# Judge report: performance, async ergonomics and developer experience

Judge lens: what a subnet owner experiences when writing, building, testing and running a validator, miner or owner
binary in Rust on top of rtao. Weighted most: tokio-native APIs, concurrency for fan-out, decode cost on large subnets,
compile time and dependency weight, API clarity, testing ergonomics (localnet docker, mocks), and whether the milestones
are believable. Designs that are elegant on paper but painful to use or build are penalized.

Path shorthands: `SP/` = `SP/`,
`UP/` = `SP/upstream/subtensor/`, `CORE/` = `UP/sdk/bittensor-core/src/`, `PY/` = `UP/sdk/python/bittensor/`,
`REG/` = `REG/`. Proposals: A = `SP/design/arch-A-mirror-python.md`,
B = `arch-B-type-driven.md`, C = `arch-C-operator-first.md`, D = `arch-D-minimal-custom-surface.md`.
Markers: **[V]** I checked it on disk or by running a command in this session; **[I]** my inference.

---

## 0. Verdict

| Proposal | correctness / fidelity | maintainability / upstream tracking | performance / async | parity / completeness | feasibility / effort | **total** |
|---|---:|---:|---:|---:|---:|---:|
| A — mirror Python | 8 | 6 | 5 | 9 | 4 | **32** |
| B — type-driven | 8 | 7 | 9 | 8 | 6 | **38** |
| **C — operator-first** | 8 | 7 | 8 | 8 | 8 | **39** |
| D — minimal custom surface | 9 | 8 | 6 | 8 | 7 | **38** |

**Winner: C.** It is the only proposal designed backwards from what a validator, miner and owner binary does minute by
minute, and it turns those loops into library objects: a supervised `NeuronRuntime`, a `WeightScheduler` with per-mechanism
cadence, jitter, bounded reconciliation and a reveal watcher, an idempotent `AxonPublisher`, an `ArcSwap` metagraph handle with
diffs, `/healthz` and `/readyz`, Prometheus metrics, defined shutdown semantics, role-based facade features (`validator`,
`miner`, `owner`) that also trim which intent groups compile, a token bucket for the rate-limited public endpoints, a
localnet `Scenario` builder, `rtao localnet` and `rtao doctor` commands, and a milestone order that ships a working validator
first. The margin over B and D is narrow. The synthesis needs B's typing and metagraph fast path, D's upstream-tracking
machinery, and fixes for the concerns in §7, several of which C shares with every other proposal.

B is the best *library* under a performance lens. It is the only proposal that measured anything, and its measurements
change the comparison (§2.1). But it applies positional decoding and shape hashes to every generated type to speed up a
step that its own data says is not the bottleneck, which costs robustness across runtime upgrades and adds a dynamic
fallback per read. D has the strongest maintenance story, but its framework is thin and it has two async defects. A has the
highest Python fidelity, but its API is Python-shaped, its crate is monolithic, and it ships transactions last.

---

## 1. Method

1. Read `research/CORRECTIONS.md`, the three feasibility notes, and all eleven research reports (A1, A2, B, C1, C2, D, E, F,
   G, H, I), then all four proposals end to end.
2. Spot-checked at least eight concrete claims per proposal against upstream sources, the cargo registry, the catalogs,
   B's benchmark artifacts and live-RPC captures already in the scratchpad (§3). No upstream directory was modified. The
   only files I wrote are this report and two field lists in `SP/judge-perf-dx/` (SelectiveMetagraph field order versus
   the index enum, used to verify B's claim).
3. Scored each dimension 1-10 with my lens folded in:
   - **performance / async** includes API clarity and async ergonomics;
   - **feasibility / effort** includes compile time, dependency weight, testing ergonomics and milestone realism.

---

## 2. Cross-cutting findings that change the comparison

### 2.1 Decode cost: measured, and not where three proposals assume

B decoded live finney `get_metagraph` payloads (netuids 1, 3 and 64: 48-51 KB, 256 UIDs) and a synthetic 2,500-UID payload
(408 KB) five ways. The numbers are on disk in `SP/design-bench/bench-run-3.txt` **[V]**:

| Strategy | 256 UIDs | 2,500 UIDs |
|---|---:|---:|
| core `Runtime::decode_id` → `Value` (Python shapes) | 330 µs | 2,683 µs |
| `scale-value` → `Value` | 911 µs | 7,312 µs |
| `parity-scale-codec` derive, owned, positional | 30 µs | 261 µs |
| `scale-decode` `DecodeAsType` (subxt-style, by name) | 1,380 µs | 11,001 µs |
| `DecodeAsType`, `[u8;32]` keys, hot fields only | 776 µs | 6,482 µs |
| borrowed columnar view (B's `MetagraphView`) | 6.5 µs | 57 µs |

Consequences:

- **A, C and D all decode typed models with `DecodeAsType` over the core's `PortableRegistry`.** That is the slowest path
  measured: about 4x slower than the core's own `Value` decoder. A goes further and argues the typed path is cheaper than
  `Value` because it avoids rendering ss58 strings (A §8.2). The measurement contradicts that.
- **None of this is the validator bottleneck.** B measured 0.71-0.88 s for a one-shot HTTPS `state_call` of the same
  payload **[V, B App. A]**. Even 11 ms per refresh at 2,500 UIDs is noise for a validator syncing once per tempo or a
  miner refreshing every 600 s. Decode cost matters for indexers that scan all subnets, for miners checking every request
  against the metagraph (an O(1) hotkey index matters more there than decode speed), and for allocation pressure.
- **The measured `DecodeAsType` gap is partly an artifact of the bench.** It uses `#[derive(DecodeAsType)] struct
  AccountId([u8; 32])` (`SP/design-bench/src/main.rs:44-45` **[V]**), so every key decodes element by element through
  the visitor. In addition, the named-field derive collects every composite into a `BTreeMap` keyed by field name
  (`REG/scale-decode-derive-0.16.2/src/lib.rs:229-236` **[V]**). A hand-written visitor that slices
  `Array::bytes_from_undecoded()` (`REG/scale-decode-0.16.2/src/visitor/types/array.rs:54` **[V]**) for 32-byte keys
  would probably narrow the gap **[I, not measured]**.

Recommendation for the synthesis:
- keep by-name `DecodeAsType` as the general typed path, because it tolerates appended fields;
- use B's borrowed `MetagraphView` (with its owned fallback) for the metagraph;
- add the bench payloads as criterion baselines;
- drop the claim that typed decode is cheaper than `Value`.

### 2.2 Build weight: every proposal makes every neuron binary compile libsodium and vendored OpenSSL

- All four proposals enable `bittensor-core/host` for validator and miner builds:
  - A pins `features = ["host"]`;
  - C states "`bittensor-core/host` is always on";
  - D builds the core "always with `host` (keyfiles) until PR-U10";
  - B gates `host` behind its `wallet` feature, but `neuron = [..., "wallet", ...]`.
- Cost of `host`:
  - 330 unique crates versus 207 (`research/feasibility-bittensor-core-gitdep.md`);
  - libsodium, vendored OpenSSL (needs Perl `IPC::Cmd`), `secp256k1-sys` and hidapi C builds (research B §6);
  - harder Docker, musl and cross builds for operators. B measured the no-`host` tag build at 13.2 s wall / 84.6 s CPU
    (`SP/design-tagpin/tagpin.log` **[V]**).
- **A neuron binary does not need `host`.** `keyfiles`, `pow`, `client` and `transaction` are host-gated
  (`CORE/lib.rs:10-25` **[V]**). `Keypair::from_mnemonic`, `from_seed`, `from_uri`, `from_private_key`, `sign` and
  `verify`, plus `timelock`, `mlkem`, `digest`, `codec` and `runtime`, are not (`CORE/keys/mod.rs:211-402` **[V]**).
- **The only blocker is that the plaintext hotkey JSON codec lives in the host-gated `keyfiles` module.** B identifies this
  (B §1.6 item 5) and proposes the upstream PR. No proposal offers an interim fix (§7, concern 1).

### 2.3 The downstream `w3f-bls` patch is mandatory today: measured

- The bench workspace built without `[patch.crates-io] w3f-bls`, and its lockfile has two `w3f-bls 0.1.3` sources:
  crates.io and the monorepo git copy (`SP/design-bench-nopatch/Cargo.lock:2377-2403` **[V]**).
- The cause is that bittensor-core inherits `w3f-bls = { workspace = true, features = ["std"] }`
  (`UP/sdk/bittensor-core/Cargo.toml:62` **[V]**), which resolves to the vendored path, while `tle` asks crates.io for
  `=0.1.3`. B reports the resulting E0277 failures.
- Effect on the other proposals: the "drop the patch for std builds" experiments in A (§2.6, optional M0 experiment) and D
  (Q1) will fail unless bittensor-core's manifest changes to `w3f-bls = "=0.1.3"`, B's one-line upstream PR (B §1.6
  item 6).
- Until then every subnet workspace must carry the patch at exactly rtao's tag. A template plus `rtao doctor` are
  mandatory, not nice-to-have.

### 2.4 Transport: tungstenite versus jsonrpsee is a real trade, not a style choice

- **jsonrpsee 0.26 cannot surface the gateway's refusal headers.** It reports a refused handshake only as
  `WsHandshakeError::Rejected { status_code }` (`REG/jsonrpsee-client-transport-0.26.1/src/ws/mod.rs:206-210` **[V]**).
  tungstenite 0.28 returns `Error::Http(Box<Response<Option<Vec<u8>>>>)` and models close code 1008 as `CloseCode::Policy`
  (`REG/tungstenite-0.28.0/src/error.rs:71`, `src/protocol/frame/coding.rs:154,214` **[V]**).
- **Python's policy-refusal contract needs those headers**: `Retry-After`, `X-RateLimit-Policy` and `X-RateLimit-Reason`
  (research A1 §6.1).
- **How each proposal handles it:**
  - A and B own a tokio-tungstenite session.
  - D accepts the gap and documents it.
  - C builds on jsonrpsee and claims `PolicyRefused { retry_after }` without noting the gap.
- **The `subxt_rpcs::RpcClientT` seam is valuable for DX.** C and D implement it (`REG/subxt-rpcs-0.51.1/src/client/rpc_client_t.rs:26-52`
  **[V]**). It brings typed `LegacyRpcMethods`, `MockRpcClient` and an optional `subxt::OnlineClient` over the same
  connection, which is C's `subxt` feature.
- **Best of both [I]:** run C's supervisor on tokio-tungstenite, as A and B do, and expose it through `RpcClientT`.
- **Testability:**
  - C's and D's `FakeChain` sits at the `RpcClientT` level. D's is built on `MockRpcClient`, whose builder registers
    per-method handlers (`REG/subxt-rpcs-0.51.1/src/client/mock_rpc_client.rs:50-70` **[V]**).
  - A fake at that level replaces the layer that holds the reconnect, resend and `FateUnknown` logic. D's §3.12
    transport checklist therefore cannot run as described.
  - A's `FakeNode` and B's `FakeChain`, which serve a real WebSocket on 127.0.0.1, do exercise that logic.

### 2.5 Public-endpoint rate limits make read coalescing a correctness-of-experience issue

- **The limit.** Public OTF endpoints are "rate-limited (roughly 1 request per second per IP)"
  (`UP/docs/guides/local-development.mdx:160` **[V]**). Exceeding storage work returns `-32004`, which every proposal
  treats as terminal.
- **The read volume.**
  - C's `WeightsPreflight` reads about 18 storage items plus a stake read at one block (C §9.3 step 2). C's default token
    bucket for public pools is 1 r/s with a burst of 10 (C §3.2.4).
  - `epoch_status`, the hyperparameter planner and the owner monitor have the same shape.
- **What the proposals offer.** None batches heterogeneous storage keys into one `state_queryStorageAt` at a pinned hash.
  Python's `query_batch` batches one item over many keys. A and B mention JSON-RPC batch requests. D chunks single-item
  batches into groups of 256 keys.
- **Proposed fix [I].** One coalesced call per block would turn a ~10 s preflight on a public endpoint into one or two
  requests.

### 2.6 Byte-parity details only some proposals caught

- **Weight quantization.** Python uses `round((weight / top) * U16_MAX)` (`PY/intents/weights.py:76` **[V]**), which rounds
  half to even. Rust's `f64::round` rounds half away from zero. A and B specify `round_ties_even` and Python-recorded
  vectors; C and D do not.
- **`limit_price`.** It is declared `TaoBalance` on `add_stake_limit`, `remove_stake_limit`, `swap_stake_limit`,
  `remove_stake_full_limit`, `add_collateral` and `move_stake_limit` (`SP/research/_call_index_map.txt` lines 34, 35, 36, 46,
  80, 84 **[V]**), but it is a price (rao per alpha). A, C and D map metadata idents mechanically (`TaoBalance → Rao/Tao`),
  so their generated builders would type a price as an amount. Only B overrides it (`PriceRao`).
- **Metadata version.** Live finney `state_getMetadata` returns V14 (357,499 bytes, version byte 14), and
  `Metadata_metadata_at_version(15)` returns V15 (400,688 bytes) (`SP/meta_getMetadata.json`, `SP/meta_v15_call.json`
  **[V]**). The core builds `apis = Vec::new()` from V14 (`CORE/runtime/mod.rs:172-180` **[V]**), so runtime-API calls
  are impossible on a V14 codec. All four proposals fetch V15 first, but any fallback that silently lands on V14 against
  a live chain must be a hard error. C is the only proposal that calls this out (C §1.4).

---

## 3. Spot checks (claim → evidence → result)

✔ verified · ◐ verified with a caveat · ✗ contradicted or not achievable as written.

### A — mirror Python

| # | Claim | Evidence | Result |
|---|---|---|---|
| 1 | jsonrpsee reports handshake rejection as `Rejected { status_code }` only | `REG/jsonrpsee-client-transport-0.26.1/src/ws/mod.rs:206-210` | ✔ |
| 2 | tungstenite exposes the response and `CloseCode::Policy` | `REG/tungstenite-0.28.0/src/error.rs:71`; `coding.rs:154,214` | ✔ |
| 3 | `DecodeAsType` derive looks fields up by name and ignores extras | `REG/scale-decode-derive-0.16.2/src/lib.rs:229-236,356-371` | ✔ |
| 4 | Monorepo catalogs are byte-identical to the fetched bittensor.com catalogs | `cmp UP/website/.../public/catalog/{intents,reads,errors}.json SP/docs/*.json` | ✔ |
| 5 | Python package is 60,461 lines; `cli/` 23,083; `intents/` 8,335; `_transport/` 3,941 | `wc -l` (60,461 counts `.pyi`; `.py` alone is 59,723) | ✔ |
| 6 | `Value` helpers at `CORE/client.rs:1515-1570` are pure | they exist (`field`, `variant`, `as_u128`, …) but live in host-gated `client.rs` (`CORE/lib.rs:10-11`) | ◐ |
| 7 | `metadata_ir()`/`runtime_api_map()` live in the PyO3 crate | `UP/sdk/bittensor-core-py/src/runtime.rs` (~554, ~579) | ✔ |
| 8 | The typed `DecodeAsType` path is cheaper than the `Value` path (§8.2) | B's bench: 1,380 µs vs 330 µs at 256 UIDs | ✗ |
| 9 | M0 experiment: drop the `w3f-bls` patch for std builds | two `w3f-bls` sources without it (`SP/design-bench-nopatch/Cargo.lock`) | ✗ without an upstream manifest change |
| 10 | Python `round` is half-to-even, so Rust needs `round_ties_even` | `PY/intents/weights.py:76` | ✔ |

### B — type-driven

| # | Claim | Evidence | Result |
|---|---|---|---|
| 1 | Decode table (330/911/30/1,380/776/6.5 µs; 2,683/7,312/261/11,001/6,482/57 µs) | `SP/design-bench/bench-run-3.txt` | ✔ |
| 2 | `tag = "v477"` resolves to `5c6e83e2`; release build 84.6 s CPU | `SP/design-tagpin/Cargo.lock`, `tagpin.log` (84.55 s user, 13.2 s wall) | ✔ |
| 3 | The `w3f-bls` patch is mandatory | two sources in `SP/design-bench-nopatch/Cargo.lock`; core uses `workspace = true` (`UP/sdk/bittensor-core/Cargo.toml:62`) | ✔ (mechanism; the E0277 log itself is not on disk) |
| 4 | `Executor<'a>` borrows the blocking `&'a Client` | `CORE/transaction.rs:997-1000` | ✔ |
| 5 | btauth verify path: `http_auth.py:55,325` → `sp_core.py:16` → `bittensor-core-py/src/keys.rs:188-193` → `keys::verify` | read on disk | ✔ |
| 6 | 116 distinct wrapped `(pallet, call)` pairs | `jq` over `SP/docs/intents.json`: 139 entries, 116 unique | ✔ |
| 7 | `SelectiveMetagraph` field order equals `SelectiveMetagraphIndex` 0..=76 | 77 fields vs 77 arms, no mismatch (`SP/judge-perf-dx/sel_*.txt`) | ✔ |
| 8 | `limit_price` is declared `TaoBalance` | `SP/research/_call_index_map.txt:34-36,46,80,84` | ✔ |
| 9 | `DecodeAsType` is intrinsically the slowest strategy | true as benchmarked, but the bench derives element-wise `[u8;32]` decoding (`src/main.rs:44-45`) | ◐ |

### C — operator-first

| # | Claim | Evidence | Result |
|---|---|---|---|
| 1 | `subxt_rpcs::client::RpcClientT` (`request_raw`, `subscribe_raw`) | `REG/subxt-rpcs-0.51.1/src/client/rpc_client_t.rs:26-52` | ✔ |
| 2 | Finney `state_getMetadata` returns V14 (357,499 B); the runtime API returns V15 (400,688 B) | decoded `SP/meta_getMetadata.json` and `SP/meta_v15_call.json` | ✔ |
| 3 | V14 yields `apis = []` | `CORE/runtime/mod.rs:172-180` | ✔ |
| 4 | Public endpoints allow ~1 request/s/IP | `UP/docs/guides/local-development.mdx:160` | ✔ |
| 5 | SN2 bounds: connect 30 s, ping 20 s, inactivity 60 s | `UP/../inference-labs-inc_subnet-2/crates/sn2-chain/src/lib.rs:29-32` | ✔ |
| 6 | btlightning depends on crates.io `sp-core = "34.0"` unconditionally | `.../inference-labs-inc_btlightning/crates/btlightning/Cargo.toml:30` | ✔ |
| 7 | Pallet predicate `BlocksSinceLastStep > tempo` vs core `> MAX_TEMPO_U64` | `UP/pallets/subtensor/src/coinbase/run_coinbase.rs:1232`; `CORE/timelock/epoch_schedule.rs:54` | ✔ |
| 8 | Handshake policy refusals surface `retry_after` | impossible over jsonrpsee 0.26 (§2.4); the gap is not mentioned | ✗ |
| 9 | 39,865 lines in `intents/ reads/ _transport/ cli/ evm/ vault/ extension/` | recount: 41,721 recursive (minor) | ✗ minor |
| 10 | "139 wrapped (pallet, call) pairs" | 139 entries, 116 distinct | ◐ wording |

### D — minimal custom surface

| # | Claim | Evidence | Result |
|---|---|---|---|
| 1 | Python package is 59,723 lines | `find PY -name '*.py' \| xargs cat \| wc -l` | ✔ |
| 2 | No `input_schema` property in `intents.json` has a `default` | `jq` → 0 | ✔ |
| 3 | Read hits: 81 single, 10 zero, 11 multi | `jq` over `reads.json` | ✔ |
| 4 | jsonrpsee lacks handshake headers (a known gap) | §2.4 | ✔ |
| 5 | `MockRpcClient` builder exists; `subxt-rpcs` pulls no codegen | `mock_rpc_client.rs:50-70`; `subxt-rpcs-0.51.1/Cargo.toml` dependency list | ✔ |
| 6 | Monorepo syn lints and clippy deny set | `UP/build.rs:70-76`, `UP/support/linting/src/*`, `UP/Cargo.toml:49-57` | ✔ |
| 7 | `FakeSubstrate.compose` returns a structural `ComposedCall` | `UP/sdk/python/tests/harness/fake_substrate.py:314-318` | ✔ |
| 8 | `btcli query` has no block flag | no `block` in `PY/cli/query.py` | ✔ |
| 9 | The §3.12 transport checklist runs on `FakeChain` failure injection | `FakeChain` is a `MockRpcClient`, which replaces `RpcSession`, so reconnect and resend are not exercised | ✗ (design inconsistency) |
| 10 | `NonceStore` serves an in-memory and a Redis store | `fn check_and_store(&self, …) -> bool` is synchronous (D line 1277), so a Redis store blocks a tokio worker | ✗ (async defect) |

---

## 4. Per-proposal assessment

### A — mirror the Python layering (32)

**Strengths**
- The strongest Python fidelity machinery:
  - a file map with blob SHAs;
  - a Python AST symbol diff;
  - transpiled literal tables;
  - syn-extracted chain tables;
  - an oracle in Docker.
- A tungstenite session with full refusal-header semantics, plus a `FakeNode` that injects drops, refusals, silence, close
  1008 and `State already discarded`.
- `round_ties_even` for weight quantization.
- A divergence register with pinning tests.
- `#[doc(alias = "bittensor.…")]` on every mirrored item, so Python users can find the Rust name in rustdoc search.

**Weaknesses under this lens**
- **The API is Python-shaped.** `dyn Substrate`, `dyn View` and `dyn Intent` use `async-trait`; storage keys are
  `&[Value]`; reads return a `Decoded` handle; generated call builders produce
  `Call::new("SubtensorModule", "serve_axon", [("netuid", netuid.into()), …])`. That reads naturally to a Python migrant,
  not to a Rust subnet author.
- **`rtao` is one crate.** It mirrors about 30 top-level modules, 83 intents, 102 reads, wallets, the executor and the
  metagraph. Every subnet binary compiles all of it, and incremental rebuilds are slow.
- **Value arrives last.** M1 requires all 102 reads before any transaction, so the first working validator lands around
  week 12-17. Full parity is estimated at 35-49 engineer-weeks, the most of any proposal.
- **The parity tool is a project in its own right**: a git-pinned `ruff_python_parser`, a transpiler, a maturin-built
  oracle image. The file-for-file mirror also couples Rust structure to upstream Python refactors, including the
  23,083-line Typer/Rich CLI.
- **Its performance claim is wrong.** The typed path is not cheaper than `Value` (§2.1).
- **`host` is always on** (§2.2).

### B — type-driven veneer (38)

**Strengths**
- **Evidence-based.** It is the only proposal that measured anything: decode strategies, `Runtime::parse` at 1.0 ms, the
  tag-pin build, the patch requirement.
- **Fast metagraph path.** The borrowed `MetagraphView` decodes in 6.5 µs and has an O(1) `by_hotkey` index.
- **Async without boxing.** RPITIT throughout; no `async-trait` boxing on hot paths.
- **Compile-time safety where it matters:**
  - `Executor::execute<I: Intent, K: KeySource<I::Role>>` returns `ExtrinsicResult<I::Output>`, and `HotkeyFile`
    implements only `KeySource<Hotkey>`, so a coldkey intent does not compile in a hotkey-only binary;
  - `Balance<Tao>` and `Balance<Alpha>` cannot be mixed;
  - storage keys typed as `NetUidStorageIndex`.
- **Transaction safety types:**
  - RAII `NonceLease` whose `Drop` clears the cache;
  - `ShieldPeriod` that cannot exceed the chain's 8 blocks;
  - `ShieldCiphertext` that rejects anything over 8,192 bytes;
  - a typed `PoolRejection`, so the shield's `Stale` is never retried (Python's substring bug, A1 quirk 5);
  - a `ReadMethod` enum that makes `author_submit*` structurally impossible to resend.
- **Codegen overrides** that fix semantic errors metadata cannot express: `PriceRao`, `StakeWeightRao`,
  `ZeroAccountIsNone`, `CompactBytesText`.
- **Tests that cover the right layers:** a `FakeChain` over a real WebSocket server, and a `RecordingView` that checks each
  read's declared targets.

**Weaknesses under this lens**
- **Positional decoding everywhere.** Generated `parity-scale-codec` types gated by shape hashes cover every type in scope,
  not just the hot ones. A runtime that only *appends* a field invalidates the hash, so typed user calls return
  `ReadError::Shape` where by-name `DecodeAsType` would have kept working. Each of the 102 reads also needs a
  `Fallback::Dynamic` path with a per-read `TryFrom<Value>`, roughly doubling read work. That buys speed for a step B
  itself says is not the bottleneck ("On the validator hot path, CPU decode is not the bottleneck", B §0).
- **Heavy machinery for users.** 23 crates, GAT-heavy signatures (`StorageEntry<K, V, Q>` with `Q::Out<V>`,
  `KeySource<R>` with GAT signers) and associated-type intents. That means a steeper learning curve and worse compiler
  errors for subnet authors.
- **`host` still arrives transitively**, through `neuron` → `wallet`.
- **Optimistic estimates.** 21-35 engineer-weeks, including CLI parity in 3-5 weeks for a 23k-line Typer/Rich CLI and
  about 70 intents in 4-6 weeks.

### C — operator-first veneer (39, winner)

**Strengths**
- **Designed from the operator loops** (C §0.1 tables).
- **A complete runtime around user code:**
  - `NeuronRuntime` supervises a `JoinSet` with a `CancellationToken`, plus watch and broadcast channels;
  - `StateCell` comes with the rule "never hold the write guard across an await";
  - `WeightScheduler` handles per-mechanism cadence, deterministic per-hotkey jitter, era 32 (so reconciliation always
    reaches a definite answer), a reveal-miss metric, and an activity-cutoff alarm;
  - `AxonPublisher` compares the chain record first and schedules around `ServingRateLimit`;
  - `MetagraphSync` keeps an `ArcSwap` handle and emits `MetagraphDiff` events.
- **Fan-out with built-in limits.** `BtAuthClient::fan_out` bounds concurrency and timeouts per call.
- **Production operations covered**: Prometheus metrics, `/healthz` and `/readyz`, shutdown semantics (drain the in-flight
  commit, persist, optional `reset_axon`), and Docker and systemd templates.
- **Safety rails on by default:**
  - neuron runtimes never load a coldkey;
  - a netuid-scoped, zero-spend `Policy`;
  - genesis and clock-skew checks at boot;
  - surface verification that fails closed per call.
- **Pay for what you use.** The `validator`, `miner` and `owner` facade features also select which intent groups compile.
- **Endpoint-friendly reads.** A token bucket for public pools; per-snapshot reads with no unpinned API, which removes the
  straddling "gather" reads; a lazy event visitor that decodes only the records a caller asks for.
- **An escape hatch.** The optional `subxt` feature builds an `OnlineClient` over the same RPC client.
- **The best testing ergonomics:**
  - a `Scenario` builder with per-test subnets and sudo knobs that lower the lock cost;
  - `rtao localnet up|down|fund|subnet`;
  - a `FakeChain` with a paused clock that drives the scheduler.
- **Guardrails against the blocking core.** A `clippy.toml` `disallowed-methods` list keeps the blocking client off async
  paths.
- **A credible milestone order.** Validator at M1, miner at M2, owner tooling at M3; about 42 engineer-weeks in total.

**Weaknesses under this lens**
- **Handshake headers are lost.** jsonrpsee drops them, and C does not say so (§2.4).
- **Its `FakeChain` as specified cannot test its own supervisor**, because it implements `RpcClientT` (C §11.3). C's `rtao-rpc` `test-util` "scripted transport" (C §2.2) may sit below the session, but C never says so (§2.4).
- **Role and unit checks happen at run time.** `Executor` takes `&dyn DynIntent` plus `WalletSigners`, and `Balance` is
  tagged at run time.
- **No metagraph fast path.**
- **`host` is always on.**
- **Uncoalesced preflight reads.** About 18 reads per weight preflight under a 1 r/s default bucket (§2.5).
- **Rounding unspecified.** No `round_ties_even` for weight quantization.

### D — minimal custom surface (38)

**Strengths**
- **One pin drives everything.** A single `upstream.toml` sets the core dependency, the patch, vendored artifacts,
  localnet digests and the toolchain.
- **Bumps fail precisely.** `xtask pin --tag` performs the bump; the `core_surface` probe names the broken symbol; a
  nightly `core-canary` builds against upstream `main`.
- **Staged upstream code.** `rtao-core-ext` carries `UPSTREAM-TARGET` headers, and `xtask staged --check` forces deletion
  once upstream ships the symbol.
- **No hand-copied constants.** Tables are extracted from Python and runtime sources, and extraction fails loudly when a
  table changes shape.
- **Less hand-porting.** By D's heuristic, 46 of the 83 intents are pass-through, so their `build()` can be generated. A
  generated `AnyIntent` enum makes completeness a compile error.
- **A Python oracle per pin** checks every ported intent and read semantically.
- **Upstream recorders are reused** instead of rewritten.
- **Bounded batch reads.** `state_queryStorageAt` is chunked at one pinned hash.
- **Async reads without boxing.** RPITIT `View`, no `dyn View`.
- **Licensing and upstreaming planned.** Dual MIT OR Apache-2.0 from day one, and a credible path to `sdk/bittensor-rs`.

**Weaknesses under this lens**
- **A thin framework** (~3k lines). Health is a single log line, with no readiness probe.
- **A synchronous `NonceStore`** alongside a Redis implementation (§3, D#10).
- **Stringly typed composite access.** Generated descriptors return `DynValue` for composites (for example
  `GET_METAGRAPH: RuntimeApiMethod<(NetUid,), DynValue>`), so typed access goes through
  `query_as::<T>(pallet: &str, item: &str, …)`.
- **Lints that cost speed and contributor time.** Adopting the monorepo's `RequireFreezeStruct` and clippy deny set
  (unwrap, indexing, arithmetic) forces `DecodeAsType` everywhere (the slowest decoder measured) and slows contributors.
- **Fragile CI.** Python and maturin run in CI for extraction and the oracle, and D's own R5 rates that as medium/medium.
- **`jsonschema` in the default graph** for `AnyIntent::from_tool`.
- **A `FakeChain` that cannot test `RpcSession`** (§2.4).
- **No v10 Synapse or Epistula crates.** That is defensible for v11 parity, but most live subnets still speak those
  protocols (research D §6).

---

## 5. Why C wins, and what it must change

C optimizes the experience this judge was asked to weigh: a subnet author writes about 50 lines of `Validator`/`Miner`
trait code and gets a production-shaped binary with metrics, health, shutdown, safety rails and a localnet test harness.
Its sequencing delivers that early. B's library ergonomics are better in places, but B's riskiest machinery (positional
decode for all types, plus per-read dynamic fallbacks) is effort aimed at a non-bottleneck. D's maintenance model is the
best, but it is orthogonal to the operator API and can be grafted onto C.

The synthesis should be **C's crate layout and operator runtime**, plus these grafts:
- B's type-level role and unit safety, its transaction-safety types and its metagraph view;
- D's single-pin, extraction, staged-upstream and generated-intent machinery;
- A's tungstenite transport and `FakeNode`, wrapped behind C's `RpcClientT` seam;
- fixes for the concerns in §7.

---

## 6. Grafts (the synthesis must adopt)

From **B**:
1. **Metagraph fast path.** Borrowed columnar `MetagraphView`:
   - `Columns::{Borrowed, Owned}`, gated by a structural shape hash of `get_metagraph`'s output type;
   - automatic fallback to the owned or dynamic path on mismatch;
   - O(1) `by_hotkey` via `OnceLock<HashMap<AccountId, Uid>>`.
   - Measured at 6.5 µs / 57 µs versus 1.38 ms / 11 ms for `DecodeAsType` (`SP/design-bench/bench-run-3.txt`).
   - Keep by-name `DecodeAsType` for every other model.
2. **Compile-time roles.** `Intent { type Role; type Output; }`, `Executor::execute<I, K: KeySource<I::Role>>` returning
   `ExtrinsicResult<I::Output>`, and `HotkeyFile: KeySource<Hotkey>` only, so C's "neuron runtimes never load a coldkey"
   becomes a type error. Keep `DynIntent`/`DynKeySource` for the CLI, agents and `execute_tool`.
3. **Static TAO versus alpha.** `Balance<Tao>` and `Balance<Alpha>` (or D's `Tao`/`Alpha`); no panicking `Add`/`Sub` on
   money; `to_wire_for(call_netuid)` checks the netuid.
4. **Codegen semantic overrides:**
   - `limit_price` → `PriceRao` (six calls, §2.6);
   - `Metagraph.total_stake` → `StakeWeightRao`;
   - `ZeroAccountIsNone` for `Owner`, `Keys` and `SubnetOwnerHotkey`;
   - `Vec<Compact<u8>>` name and symbol → text;
   - fixed-point types mapped from the typenum `Frac` parameter.
5. **Transaction-safety types:** RAII `NonceLease` (`Drop` clears unsettled state), `ShieldPeriod { P4, P8 }`,
   `ShieldCiphertext` (at most 8,192 bytes), typed `PoolRejection` (never retry `Stale` for a shielded carrier),
   `ReadMethod` (submits structurally excluded from the resend path), and `CallBytes` stamped with their spec version.
6. **The `w3f-bls` patch, handled end to end.** `rtao doctor` fails on more than one `w3f-bls` source in `Cargo.lock` or a
   tag mismatch, and the synthesis files the one-line upstream manifest PR (`w3f-bls = "=0.1.3"` in bittensor-core) that
   removes the downstream patch.
7. **Test fakes at the right layers.** A `FakeChain` served over a real loopback WebSocket (typed seeding via descriptors,
   fault injection), plus a `RecordingView` that asserts each read touches exactly its declared targets.
8. **Benchmark baselines.** Criterion benches seeded with B's committed payloads (finney netuids 1, 3 and 64, plus the
   synthetic 2,500-UID payload) and a gross-regression gate.

From **D**:
9. **One pin.** A single `upstream.toml` drives the core tag and patch, vendored artifacts with sha256, localnet image
   digests and the toolchain. Bumps go through `cargo xtask pin --tag vN`, with a `core_surface` compile probe and a
   nightly `core-canary` against upstream `main`.
10. **Staged upstream code.** An `rtao-core-ext` crate whose modules carry `UPSTREAM-TARGET` headers, enforced by
    `xtask staged --check`.
11. **Generated intents.** Generate intent interfaces and the pass-through `build()` bodies (46 of 83 by D's heuristic)
    with xtask (committed code, not proc-macros, to keep compile cost down). A generated `AnyIntent` enum makes
    completeness a compile error, with `pending.toml` stubs during build-out.
12. **Extraction and oracle.** Extract upstream literal tables (error map, hyperparameters, settings, fee filters,
    `RAW_ONLY`, samples, fake defaults) instead of hand-copying them, failing loudly on shape change. Run a CI-only Python
    oracle per pin over `FakeSubstrate`.
13. **Bounded batches.** Chunk `state_queryStorageAt` at one pinned hash. Add `rtao query <read> --block <n>`.
14. **Licensing.** Dual MIT OR Apache-2.0 from day one: vendored fixtures are Apache-2.0, and it keeps the
    `sdk/bittensor-rs` upstreaming path open.

From **A**:
15. **Weight rounding.** `round_ties_even` and Python's left-to-right summation order in `normalize` and
    `clip_to_max_weight`, with oracle u16 vectors that include `.5` ties.
16. **Python-name discoverability.** `#[doc(alias = "bittensor.<dotted.path>")]` on public items, and a generated v11 → rtao
    migration table.
17. **Transport.** The session on tokio-tungstenite, for handshake headers and close 1008, exposed through C's
    `subxt_rpcs::RpcClientT` seam so `LegacyRpcMethods`, `MockRpcClient` and the optional `subxt` feature still work.
18. **Transport tests.** `FakeNode` fault injection (403/429 with `Retry-After` and `X-RateLimit-*`, `-32004`/`-32005`,
    close 1008, silence, `State already discarded`) and a port of `tests/unit/test_rpc.py`.

---

## 7. Concerns the synthesis must resolve

1. **Every neuron binary builds the C toolchain.** No proposal provides a validator or miner build without
   `bittensor-core/host`, so subnet operators compile libsodium, vendored OpenSSL (Perl), `secp256k1-sys` and hidapi.
   That is 330 crates instead of 207, and painful Docker, musl and cross builds.
   - Evidence: §2.2; `CORE/lib.rs:10-25`; `CORE/keys/mod.rs:211-402`; `research/feasibility-bittensor-core-gitdep.md`.
   - Fix:
     - add a no-`host` neuron profile with a small plaintext-hotkey JSON reader, differential-tested against the core's
       `deserialize_keypair_from_keyfile_data` in a `host` dev-dependency test;
     - keep coldkey decryption (NaCl, Fernet, Ansible) in CLI and owner builds;
     - pursue the upstream PR that un-gates the JSON codec.
2. **The `w3f-bls` patch cannot be dropped downstream.** Without it the lockfile resolves two `w3f-bls 0.1.3` sources.
   A's M0 experiment and D's Q1 will fail without the upstream manifest change.
   - Evidence: §2.3; `SP/design-bench-nopatch/Cargo.lock:2377-2403`; `UP/sdk/bittensor-core/Cargo.toml:62`.
3. **Decode cost and the claims about it.** `DecodeAsType` (the typed path in A, C and D) measures about 4x slower than the
   core's `Value` decoder. A's claim that it is cheaper is wrong.
   - Evidence: §2.1.
   - Fix: use the metagraph view (graft 1), keep benchmark gates, and stop deciding hot-path codecs without measurement.
4. **Policy-refusal metadata.** C's jsonrpsee transport cannot read `Retry-After` or `X-RateLimit-*` from a refused
   handshake, but C promises `retry_after`.
   - Evidence: `REG/jsonrpsee-client-transport-0.26.1/src/ws/mod.rs:206-210`.
   - Fix: graft 17.
5. **Supervisor testability.** D's `FakeChain` (a `MockRpcClient`), and C's as specified in C §11.3, replace the layer that holds the reconnect, resend and
   `FateUnknown` logic, so those transport tests cannot run as written.
   - Evidence: §2.4; `REG/subxt-rpcs-0.51.1/src/client/mock_rpc_client.rs:50-70`.
   - Fix: a fake WebSocket server (grafts 7 and 18) for the session, and `RpcClientT` fakes above it.
6. **Rate-limited public endpoints.** Weight preflight, `epoch_status`, the hyperparameter planner and the owner monitor
   each issue 10-20 single-key reads per block. At ~1 request/s per IP that means seconds per operation, plus a risk of
   terminal `-32004` refusals.
   - Evidence: `UP/docs/guides/local-development.mdx:160`; C §9.3 step 2.
   - Fix: a "read plan" that coalesces heterogeneous storage keys into one `state_queryStorageAt` at the pinned hash (with
     runtime APIs in parallel), and documented own-node guidance.
7. **Async correctness of the nonce store.** D's `NonceStore::check_and_store` is synchronous but has a Redis
   implementation, which blocks a tokio worker on network I/O.
   - Evidence: D line 1277.
   - Fix: make the trait async (B's RPITIT form or C's `BoxFuture`).
8. **Byte parity of weight quantization.** C and D do not specify half-to-even rounding.
   - Evidence: `PY/intents/weights.py:76`.
   - Fix: graft 15.
9. **Mis-typed prices in generated signatures.** Mechanical ident mapping types `limit_price` as a TAO amount in six calls.
   - Evidence: `SP/research/_call_index_map.txt:34-36,46,80,84`.
   - Fix: graft 4.
10. **Robustness of typed reads across runtime upgrades.** If positional types are used beyond the metagraph view, an
    upgrade that only appends a field turns typed reads into `ReadError::Shape` until rtao regenerates and every subnet
    binary rebuilds. Upstream ships about 9 finalized runtimes in 20 days (research H §0.3).
    - Fix: by-name `DecodeAsType` as the default typed path; positional or borrowed decode only where measured and always
      with a fallback.
11. **No silent drop to V14.** On live finney `state_getMetadata` returns V14, and the core builds no runtime APIs from V14.
    A fallback that silently lands on V14 against a V15-capable chain breaks every runtime-API call.
    - Evidence: §2.6; `CORE/runtime/mod.rs:172-180`.
    - Fix: request V15 explicitly and treat V14 as an error unless the block's runtime predates V15.
12. **Realistic effort.** B's 21-35 engineer-weeks is optimistic (CLI parity in 3-5 weeks for 23,083 lines of Typer/Rich
    code). A's 35-49 delivers value last.
    - Fix: budget about 40-45 engineer-weeks for one engineer to full parity, following C's order (validator by about
      week 8, miner about 11, owner tooling about 16). Run the CLI, signers and the long intent tail in parallel tracks
      after M3 [I].
13. **Python in CI.** A's and D's tracking machinery (AST parsing, extraction, maturin-built oracle images) is CI-only but
    fragile and slow.
    - Fix: cache oracle images per tag, keep a Rust-only extraction fallback, and never let a Python step gate a build of
      rtao or a subnet binary.

---

## 8. Suggested shape of the synthesis (for the next author)

- **Crates.** C's layout (`rtao-btcore`, `rtao-primitives`, `rtao-runtime`, `rtao-rpc`, `rtao-signer`, `rtao-client`,
  `rtao-wallet`, `rtao-tx`, `rtao-reads`, `rtao-intents`, `rtao-metagraph`, `rtao-auth`, `rtao-http`, `rtao-neuron`,
  `rtao-cli`, `rtao-testkit`, facade `rtao`), plus D's `rtao-core-ext`.
- **Facade features.** C's `validator`, `miner` and `owner` features, plus a no-`host` profile for the first two
  (concern 1).
- **What a validator author writes:**

  ```rust
  #[tokio::main]
  async fn main() -> anyhow::Result<()> {
      let cfg = rtao::neuron::NeuronConfig::load()?;                  // flag > env > btcli.json > default
      rtao::neuron::NeuronRuntime::validator(cfg, MyValidator::default()).run().await
  }
  impl rtao::neuron::Validator for MyValidator {
      type State = rtao::neuron::EmaScores;
      async fn step(&self, cx: &NeuronCx, s: &StateCell<Self::State>) -> anyhow::Result<()> {
          let mg = cx.metagraph.load();                                 // Arc<MetagraphView>, O(1) by_hotkey
          let res = cx.http.fan_out(mg.axon_targets(), |t| make_request(t), FanOut::default()).await;
          s.write().await.update(&res);                                // never held across an await
          Ok(())
      }
      fn weights(&self, _cx: &NeuronCx, s: &Self::State, _m: MechId) -> WeightsProposal { s.to_proposal() }
  }
  // typed one-offs: cx.executor.execute(&ServeAxon { .. }, &cx.hotkey).await?  // coldkey intents do not compile here
  ```

- **Transport.** A tokio-tungstenite supervisor behind `subxt_rpcs::RpcClientT`, legacy RPC namespace only, a
  `ReadMethod` resend allowlist, a token bucket for public pools, read-plan coalescing, and an optional `subxt` feature.
- **Decoding.**
  - By-name `DecodeAsType` models for everything typed.
  - `MetagraphView` for the metagraph.
  - Core `Value` for JSON, CLI and dynamic surfaces (corpus-compatible).
  - Encoding via the core's `compose_call` (C, D), so golden-vector bytes come from one encoder.
- **Tracking.** D's single pin, extraction, staged crate and generated intents; C's runtime surface verification; B's
  drift classes; A's oracle vectors for weight conformance.

---

## Appendix — reproduce the key checks

```sh
SP=SP
cat $SP/design-bench/bench-run-3.txt                                  # decode strategies
grep -n -A3 'name = "w3f-bls"' $SP/design-bench-nopatch/Cargo.lock      # two sources without the patch
grep -n 'w3f-bls' $SP/upstream/subtensor/sdk/bittensor-core/Cargo.toml   # workspace = true -> vendored path
sed -n 200,212p ~/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/jsonrpsee-client-transport-0.26.1/src/ws/mod.rs
sed -n 10,30p $SP/upstream/subtensor/sdk/bittensor-core/src/lib.rs     # host-gated modules
jq '[.[] | .wraps[] | join(".")] | unique | length' $SP/docs/intents.json   # 116
sed -n 76p $SP/upstream/subtensor/sdk/python/bittensor/intents/weights.py   # round()
sed -n 160p $SP/upstream/subtensor/docs/guides/local-development.mdx      # ~1 req/s/IP
```
