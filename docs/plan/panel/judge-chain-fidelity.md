# Judge report: chain fidelity and correctness

Judge lens: correctness and chain fidelity. Does a proposal keep chain-defined behavior inside
`bittensor-core`, get transaction extensions, era, nonce, shield, proxy and multisig right, mirror v11 semantics
(plan, execute, policy, results, errors) exactly, and specify btauth/1 byte-exactly? Anything that drifts from the
chain or from the Python SDK is penalized.

Date: 2026-10-13. Inputs:
- the four proposals under `scratchpad/design/arch-{A,B,C,D}-*.md`, read end to end (D's CI YAML skimmed);
- `research/CORRECTIONS.md`, the three feasibility notes, and all research reports (B and A1 in full; A2, C1, C2, D,
  E, F, G, H and I by section).

Every claim marked **[V]** below was re-checked by me on disk this session; commands and line numbers are in §7.
**[I]** marks my inference.

Path shorthands: `UP/` = `scratchpad/upstream/subtensor/`, `CORE/` = `UP/sdk/bittensor-core/src/`,
`PY/` = `UP/sdk/python/bittensor/`, `REG/` = `~/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/`.

---

## 1. Verdict

| Proposal | Correctness / fidelity | Maintainability / upstream tracking | Performance / async | Parity completeness | Feasibility / effort | Total | Verdict |
|---|---:|---:|---:|---:|---:|---:|---|
| **A: mirror Python** | **9** | 9 | 7 | **10** | 7 | **42** | **Winner.** Highest fidelity; the only design that gets the commit-reveal version gate right and can reproduce Python's transport refusal semantics exactly. Needs grafts for the epoch predicate, the V15 requirement and B's w3f-bls evidence. |
| D: minimal custom surface | 8 | **9** | 7 | 9 | 8 | 41 | Close second. Best anti-drift *mechanisms* (extract, never hand-copy; Python oracle; staged upstream code with a deletion gate). Defects: echoes `CommitRevealWeightsVersion`, keeps the jsonrpsee header gap (acknowledged), and has a sync `NonceStore` that cannot back Redis. |
| C: operator-first | 7 | 8 | 8 | 8 | 8 | 39 | Strong operational design and the sharpest catch (finney `state_getMetadata` is V14, which has no runtime APIs). Defects: version-gate echo, an overclaimed jsonrpsee policy-refusal parity, nonce reservation on `FateUnknown`, 32-block weight era. |
| B: type-driven | 6 | 7 | **9** | 8 | 6 | 36 | Best measurements and type-level safety ideas, but it owns a second encoding and decoding path: positional codec types, a home-grown shape hash, a hand-written metagraph parser, and hashers baked into generated storage keys. It also echoes the version. Highest drift surface. |

Winner: **A**. The scores are mine, weighted toward correctness. Section 6 lists the grafts the synthesis must take
from B, C and D, and section 5 lists the unresolved problems.

---

## 2. Fidelity checkpoints across the four designs

Legend: ✔ correct and specified; ◐ correct but under-specified, or a documented deliberate divergence;
✘ wrong, or would drift from chain or Python.

| Checkpoint (ground truth) | A | B | C | D |
|---|---|---|---|---|
| Chain-defined bytes come from the core: signature payload in metadata extension order, v4 assembly, era, multisig, storage keys, `compose_call` (`CORE/codec/extrinsic.rs:38-118,260-316,407-437`; `encode.rs:528-585`) | ✔ all through core | ◐ payload and assembly through core, but typed calls are encoded as `[p,c] ++ args.encode()` with generated codec types, and typed storage keys use `hash_param` with **hashers baked at codegen** (arch-B lines 215, 746) | ✔ typed structs → `Value` → core `compose_call`; keys via `Runtime::storage_key` from the live entry | ✔ `IntoCoreValue` → core encoder; keys via core |
| Extension guard: fail closed on an unknown non-zero-sized extension, at connect (`extrinsic.rs:89-118`; CORRECTIONS #1: the 478 identifier is unchanged) | ✔ | ✔ | ✔ | ✔ (dummy `signature_payload_parts` probe) |
| Era: default 128, mortal anchored at **finalized** head, immortal at genesis (`PY/settings.py:22`; `PY/_transport/interface.py:485-499`) [V] | ✔ | ✔ | ✔ default; ◐ weight commits use era 32 (product divergence) | ✔ |
| Shield: carrier n and inner n+1, both era ≤ 8 (`UP/runtime/src/check_mortality.rs:19,94-100` [V]); `twox_128` key-hash prefix; ≤ 8192 B; inner-receipt scan; `max_fee` asymmetry | ✔ | ✔ plus type-level `ShieldPeriod{P4,P8}` | ✔ | ✔ |
| Nonce: first `account_nextIndex`, then local increment; **clear on any failure** between sign and inclusion; `prepare` always fresh (`PY/_transport/extrinsics.py:55-84` [V]; `_substrate.py:753-772`) | ✔ | ✔ (RAII lease clears on drop) | ◐ keeps the nonce *reserved* on `FateUnknown` until reconciliation; diverges from Python | ✔ |
| Retries only on `"priority is too low"`, `"transaction is outdated"`, `"stale"` (`PY/executor.py:52-55` [V]) | ✔ | ◐ typed `PoolRejection` (never retries `Stale` for shield carriers) | ✔ | ✔ (extracted table) |
| Composition order semantic → `Sudo.sudo` (root) → `Proxy.proxy` → multisig `wrap_call`; pinned prior-round bytes; sorted signatories minus signer | ✔ | ✔ | ✔ | ✔ |
| Receipt: locate by blake2 of raw bytes; 6 nested-wrapper events + MevShield failures; fee fallback (`PY/_transport/utils/receipt.py:8-16` [V]; the core checks only 3, `CORE/client.rs:1024-1043` [V]) | ✔ (port) | ✔ | ✔ | ✔ (+ upstream PR-U2) |
| Plan, policy, results: fail-closed fee cap; `UNBOUNDED`; `affects_all_subnets`; raw refusal; `execute` submits exactly `plan.call`; `"Submitted (not waiting)."` = pool acceptance (`PY/intents/plan.py:51-72` [V]) | ✔ line-for-line | ✔ (typed outputs + extras; JSON shape kept) | ✔ (+ `status`) | ✔ |
| Error taxonomy: 18 codes; 390 names; custom codes 0-27 and 255 tied to the runtime enum; needles; dispatch table; precedence | ✔ (syn-parsed code set, better than Python's hard-coded range) | ✔ | ◐ CI asserts `range(28)+{255}`, like Python | ✔ (syn + extraction) |
| **CRv3 `commit_reveal_version`**: a client-version tag that "**must** match … Used to gate runtime upgrades" (`pallets/subtensor/src/subnets/weights.rs:318-337` [V]; `dispatches.rs:1946` "The client (bittensor-drand) version" [V]). Python hard-codes `DEFAULT_COMMIT_REVEAL_VERSION = 4` (`PY/intents/weights.py:33`, used at `:411`, `:472-477` [V]) | **✔** constant 4 plus a framework guard that refuses when storage ≠ 4 (§9.4) | **✘** reads storage and echoes it (§4.3, §9.2) | **✘** echoes storage (§9.3 step 6) | **✘** echoes storage, mislabeled "rtao-stricter" (§9.2 item 4) |
| Reveal-round prediction: chain predicate `BlocksSinceLastStep > tempo` (`run_coinbase.rs:1232` [V]) vs core `> MAX_TEMPO_U64` (`epoch_schedule.rs:54` [V]) | ◐ uses core `generate_commit_v2`, exactly as Python; inherits the core bug; files U2 with a flip test | ✔ pallet-faithful port + injected clock (Python-divergent) | ✔ pallet-faithful shim + agreement test vs core | ✔ staged pallet-faithful copy + PR-U5 |
| Metadata: V15 via `Metadata_metadata_at_version(15)`; V14 only on Python's missing-method needle (`PY/_transport/runtime.py:292-312` [V]); never rely on a V14 `Runtime` for runtime APIs (`CORE/runtime/mod.rs:172-181`: V14 ⇒ `apis = []` [V]) | ✔ (fallback "when the method is missing") | ◐ fallback unqualified | ✔✔ always V15, and spells out that finney's `state_getMetadata` is V14 | ◐ "fall back on error" (the core's rule, not Python's) |
| Transport refusal parity: HTTP 403/429 handshake **with** `X-RateLimit-Policy/Reason`, `Retry-After`; `-32004/-32005`; close 1008 (`PY/_transport/rpc.py:53-59,92-124,190-241`) | ✔ tokio-tungstenite exposes `Error::Http(Box<Response>)` and `CloseCode::Policy` (`REG/tungstenite-0.28.0/src/error.rs:71`, `coding.rs:154,214` [V]) | ✔ same choice | **✘** claims `retry_after` on handshake refusals over jsonrpsee `WsClient`, which yields only `Rejected{status_code}` (`REG/jsonrpsee-client-transport-0.26.1/src/ws/mod.rs:205-210,522-524` [V]) | ◐ same library, gap acknowledged (R8) with a ~200-line soketto fix |
| Typed decode safety | ✔ `DecodeAsType` by name over the core registry, plus a typed-vs-`Value` differential test | ◐ positional codec derive gated by an rtao-defined shape hash; hand-written borrowed parser for the metagraph | ✔ `DecodeAsType` by name | ✔ `DecodeAsType` + `check-models` |
| btauth/1 byte-exactness (`PY/http_auth.py:58-343`; `signed-requests.mdx:252-289`) | ✔ | ✔ | ✔ | ✔ (sync store ◐) |
| Divergence discipline | ✔ register D-001…D-005, each with a test; D-001 flagged "verify on localnet first" | ◐ several baked in by codegen (`ZeroAccountIsNone`, `StakeWeightRao`, `PriceRao`) | ◐ 7 listed, some unverified (zero account) | ✔ `exclusions.toml`, oracle accepts only listed divergences |

### btauth/1 specifically

All four specify the eight-line `\n`-joined payload with no trailing newline:

1. `btauth/1`
2. canonical scheme
3. uppercase method
4. percent-encoded target, verbatim
5. lowercase sha256 of the raw body
6. nanosecond nonce
7. sender
8. receiver, or `-`

They also share the normative verification order (shape → receiver → freshness → signature under the declared scheme
only → replay store after the signature), the 10 s / 2 s / 60 s constants, the golden vectors and every Python
negative case. All four verify through `bittensor_core::keys::verify`, so they accept the `<Bytes>…</Bytes>` wrapper
exactly as Python does. I verified that chain myself: `PY/http_auth.py:55` → `PY/sp_core.py:16` →
`UP/sdk/bittensor-core-py/src/keys.rs:189-193` → `CORE/keys/mod.rs:457-469`.

Differences:
- A states verification order and store semantics most precisely, and mirrors Python's synchronous-signer
  requirement.
- B, C and D state the header *values* more explicitly (`0x` + lowercase hex; Crypto header only for ed25519).
- **All four move the retention check (`max_age + skew ≤ retention`) to construction.** Python raises `ValueError`
  *after* a verified signature (`PY/http_auth.py:331-339` [V]). The move is acceptable, but it is a divergence and
  must be registered.
- None of the four mentions the exact mapping of a core verify *error* (bad ss58, wrong signature length) to
  `MalformedAuth("invalid hotkey address or signature encoding")` (`PY/http_auth.py:324-328` [V]). It is implied by
  "messages verbatim", but the synthesis should spell it out.

---

## 3. Per-proposal assessment

### 3.1 A: mirror the Python layering (winner)

Why it is the most faithful:
- **Boundary.** "rtao uses from bittensor-core what the Python package uses, at the same seams." Every byte comes from
  the core, including the reveal-round function Python calls (`generate_commit_v2`). The core's blocking `Client` and
  `Executor` appear only as a `core-oracle` test lane.
- **Write path is Python's, step for step:**
  - nonce cache semantics;
  - finalized-anchored era;
  - fee probe via `state_call TransactionPaymentApi_query_info` with an immortal era and a zero signature;
  - watch state machine;
  - receipt port;
  - transport-failure mapping (`Err` for policy or exhaustion, failed `ExtrinsicResult` otherwise).
- **Transport.** A raw tokio-tungstenite session that reproduces rpc.py's refusal contract, including handshake
  headers and close code 1008. A argues correctly from the library sources why jsonrpsee cannot.
- **Commit-reveal version: the only correct design.** It keeps Python's constant 4 and adds a framework guard that
  refuses early if `CommitRevealWeightsVersion ≠ 4` (§9.4). This preserves the chain's client-upgrade lever
  (weights.rs:318-337).
- **Parity enforcement is mechanical:**
  - per-file git blob SHAs;
  - a Python-AST symbol diff;
  - catalog equality;
  - `syn` extraction of `transaction_error.rs`, `fee_filters.rs` and `SelectiveMetagraphIndex::from_index`;
  - transpiled literal tables;
  - a dockerized Python oracle at the same tag.

  This gives the smallest chance of silent drift from v11.
- **Weight conformance details are right.** A uses `f64::round_ties_even` (Python `round()` is half-to-even,
  `PY/intents/weights.py:76` [V]) and keeps the same summation order.
- **Divergences are registered with tests.** D-001 (zero-account default) is explicitly "[inferred]; verify on a
  localnet in M1 before adopting".

Defects and gaps:
1. **Epoch predicate.** A deliberately inherits the core's `> MAX_TEMPO_U64` predicate (`epoch_schedule.rs:54` [V])
   for Python parity, and files U2 with a regression test. That is principled, but it knowingly ships a
   reveal-block prediction that differs from the pallet (`run_coinbase.rs:1232` [V]) after deferred or skipped
   epochs. A late round silently loses the commit (C1 §2.3). Graft the pallet shim (§6).
2. **w3f-bls experiment premise.** "Build without the w3f-bls patch (crates.io `w3f-bls =0.1.3`) and prove ciphertexts
   still decrypt" (§2.6) cannot get past compilation. Without the patch the lockfile resolves two `w3f-bls 0.1.3`
   sources (`scratchpad/design-bench-nopatch/Cargo.lock:2377-2404` [V]), and B reports E0277 in the core. The
   realistic path is B's one-line upstream manifest change.
3. **Unmeasured performance claim.** "The typed path avoids about 2 × n ss58 renderings" suggests typed decode is
   faster. B measured `DecodeAsType` at 1,380 µs against the core `Value` path at 330 µs for 256 UIDs
   (`design-bench/bench-run-3.txt` [V]). This does not affect correctness (both are negligible next to a ~0.8 s RPC),
   but the rationale is wrong.
4. **Constants are read from the head codec** (Python parity). B, C and D read them from the snapshot's runtime,
   which is more correct across upgrades. Adopt that as a registered divergence.
5. Heavy tooling: `ruff_python_parser` is git-only, and 23k CLI lines must be ported. The effort (35-49 EW) is honest,
   which is why feasibility is 7.

Spot-checks (11, all against disk):
- ✔ `CORE/client.rs:1515-1570` `Value` helpers.
- ✔ `bittensor-core-py/src/runtime.rs:554` `runtime_api_map` and `:579` `metadata_ir`.
- ✔ `CORE/codec/decode.rs:812-817` `bytes_value`.
- ✔ `PY/_transport/extrinsics.py:55-84` `account_nextIndex`.
- ✔ `interface.py:485-499` finalized anchor.
- ✔ jsonrpsee `Rejected{status_code}`.
- ✔ tungstenite `Http(Box<Response>)` and `Policy` = 1008.
- ✔ catalogs byte-identical to bittensor.com (`cmp`).
- ✔ `v477` = `5c6e83e2…` (local `git rev-parse`).
- ✔ `runtime/src/fee_filters.rs:15-34` `call_filter_group!`.
- ✘ the no-patch experiment premise.

### 3.2 D: minimal custom surface

Strengths for fidelity:
- **"rtao code never hand-copies a Python constant."** Python tables are extracted with stdlib `ast` (literals only);
  runtime enums are extracted with `syn`. CI asserts that the Python mirrors equal the runtime sources (fee filters;
  `PROXY_TYPES` vs the metadata enum). This is the strongest anti-drift rule in any proposal.
- **The Python oracle at the pin** builds the SDK and core from the same commit and compares composed-call *trees*
  from `FakeSubstrate` (`UP/sdk/python/tests/harness/fake_substrate.py:314-318` [V] returns `ComposedCall(module, function, params)`),
  plus violations, effects, warnings and fees. Read JSON is compared as parsed values.
- **Staged upstream code.** `rtao-core-ext` has a `UPSTREAM-TARGET` header per module, a `staged --check` that fails
  once the core exports the symbol, a `core_surface` compile probe, and a nightly canary against `main`.
- **Receipt parity gap identified precisely.** The core checks 3 nested wrappers (`CORE/client.rs:1024-1043` [V]);
  Python checks 6 (`PY/_transport/utils/receipt.py:8-16` [V]). D stages Python semantics and files PR-U2.
- **Honest about the jsonrpsee gap.** It ships `retry_after = None` at handshake and has a soketto fix ready.

Defects:
1. **`commit_reveal_version` follows storage, labeled "rtao-stricter"** (§9.2). It is laxer: after a root bump it would
   tag a v4-format payload with the new version and turn Python's loud `IncorrectCommitRevealVersion` into a possible
   silent reveal failure.
2. **Sync `NonceStore::check_and_store -> bool`** (§7.1) while `RedisNonceStore` performs network I/O (§7.2). It would
   block a tokio worker. Use the async store from A, B and C.
3. **Metadata fallback.** "Fall back to `state_getMetadata` on error" follows the core's rule. Python falls back only
   on a missing-method needle (`runtime.py:296-303` [V]); on finney the V14 blob has no runtime APIs.
4. **Python oracle infrastructure in CI.** It is legitimate (Python stays out of shipped code), but it is a maintenance
   and flake source. D's own R5 accepts this.
5. **"Display matches Python's `str(Balance)` byte for byte"** cannot hold for large amounts. Python formats a
   *float* (`f"{self.amount:,.9f}"`, `PY/balance.py:146-149` [V]), while D renders exact decimals.

Spot-checks (9):
- ✔ catalogs identical, with 0 properties carrying `default`.
- ✔ the core checks 3 wrappers and Python 6.
- ✔ fake compose is structural.
- ✔ the `Balance` format string (with the float caveat).
- ✔ the jsonrpsee gap.
- ✔ `COREPY/runtime.rs:579`.
- ✔ `_TRANSIENT_SUBSTRINGS` at `executor.py:52-55`.
- ✘ the version-gate label.
- ✘ the sync store with Redis.

### 3.3 C: operator-first

Strengths:
- **The most important metadata catch in the panel.** On finney `state_getMetadata` returns **V14** (357,499 bytes,
  version byte 14, decoded from `scratchpad/meta_getMetadata.json` [V]), and a V14 `Runtime` has `apis = []`
  (`CORE/runtime/mod.rs:172-181` [V]). So the coexistence note's 3.9 ms parse was of a runtime that cannot encode
  runtime-API calls. C mandates V15 always.
- A clippy `disallowed-methods` list makes the blocking core client, `generate_commit_v2` (wall clock) and
  `save_password_to_environment` CI failures.
- Per-call fail-closed surface verification at connect and on upgrade (`SurfaceChanged` at plan time; reads fall back
  to dynamic).
- V16 cache blobs are named outside Python's `md*.bin` prune glob (`PY/_transport/runtime.py:117` [V]).
- A pallet-faithful epoch shim with an agreement test against the core.
- A clock-skew check against `Timestamp.Now`, and a reveal-miss metric.

Defects:
1. **Echoes `CommitRevealWeightsVersion`** (§9.3 step 6). This contradicts its own reasoning in open question 5 about
   not following `WeightsVersionKey` because it defeats an upgrade lever.
2. **Transport overclaim.** It says handshake 403/429 refusals surface with `retry_after` over jsonrpsee's `WsClient`.
   jsonrpsee 0.26 returns only the status code (ws/mod.rs:205-210, 522-524 [V]).
3. **`FateUnknown` keeps the nonce reserved** until reconciliation. Python clears the cache and relies on the
   pool-aware `account_nextIndex`. Reserving can strand later pipelined nonces when the unknown transaction was in fact
   dropped [I].
4. **Weight commits use era 32 and `WaitFor::InBlock`.** Product choices, but they diverge from the
   `set_weights`/`execute` defaults (128 and finalization).
5. **Extra client-side preflights** (`StakeThreshold` plus own stake weight, permit, version key) can produce false
   negatives if the client recomputes stake weight differently from the pallet [I].
6. Minor: it repeats report B's "72 + 40" e2e counts. The manifest has 73 `intent_*` + 39 `test_*` [V].

Spot-checks (8):
- ✔ the V14 and V15 sizes and version byte.
- ✔ SN2 timeouts (`sn2-chain/src/lib.rs:28-31`).
- ✔ subxt-rpcs `RpcClientT` and `LegacyRpcMethods` lines.
- ✔ core `IntentCall` accessors (`transaction.rs:201-213`).
- ✔ `TypeResolver for PortableRegistry` at `:59`, and scale-decode field-by-name.
- ✘ the jsonrpsee refusal claim.
- ✘ the e2e counts.
- ✘ the version gate.

### 3.4 B: type-driven

Strengths:
- **Real measurements, recorded in `scratchpad/design-bench/`:**
  - The `v477` tag pin builds and derives Alice (`design-tagpin/tagpin.log` [V]).
  - The no-patch lockfile has two `w3f-bls` sources [V].
  - V15 parses in about 1 ms.
  - The decode-strategy table: `DecodeAsType` is slowest, the core `Value` is 4x faster, positional codec decode
    11x faster.
- **The correct fix for the patch footgun:** a one-line upstream `w3f-bls = "=0.1.3"` in the core manifest, plus
  `rtao doctor` that checks the downstream `Cargo.lock`.
- **Safety types:**
  - `ShieldPeriod{P4,P8}`;
  - a `ShieldCiphertext` newtype with the ≤ 8192 bound;
  - spec-stamped `CallBytes` that refuse cross-spec nesting;
  - RAII nonce leases;
  - refusal of `submit_signature` on a spec *or* transaction-version change;
  - `KeySource<Role>`, so coldkey intents do not compile against a hotkey-only binary while the `dyn` path keeps
    Python's runtime check.
- **Verified details:**
  - the exact PyO3 verify chain;
  - 34 owner hyperparameter names over 33 setters (`PY/intents/hyperparameters.py:31-66` [V]);
  - the `SelectiveMetagraph` field order equals `from_index` 0..=76 (mechanically compared, metagraph.rs:121-240 vs
    569-650 [V]);
  - corrects report B's e2e count (73 + 39 [V]).

Defects:
1. **A second encoding and decoding engine for chain bytes.** Typed calls are encoded by generated codec types;
   storage keys use **hashers baked at codegen** (`StorageEntry::new(…, &[Hasher::Identity], …)`, arch-B line 746)
   instead of the live entry. That contradicts R/A2 §8 decision 1 and the core spec's "ids and hashers resolved at
   runtime per spec". A runtime hasher change, or an archive read across one, yields a silent wrong key unless the live
   shape check also covers hashers, and B does not say that it does.
2. **Positional decode guarded by a home-grown shape hash, plus a hand-written borrowed metagraph parser.** The
   mitigations are good (property tests per pinned spec, fuzzing, a dynamic fallback), but this is exactly the kind of
   chain-shape knowledge the user wanted to leave to the core.
3. **Echoes `CommitRevealWeightsVersion`** (§4.3, §9.2).
4. **Codegen overrides bake divergences:** `ZeroAccountIsNone` and `StakeWeightRao`. They are defensible, but they are
   decided without the "verify first" step.
5. The 21-35 EW estimate looks optimistic for codegen + shape hash + view parser + full parity.

Spot-checks (9):
- ✔ PyO3 verify chain.
- ✔ e2e 73 + 39.
- ✔ 34/33 hyperparameters.
- ✔ 83 / 75-8 / 74-7-2 / 116 unique wraps.
- ✔ `SelectiveMetagraph` order.
- ✔ bench and tag-pin artifacts.
- ~ `run_coinbase.rs:1231` (the predicate is at :1232).
- ✘ baked hashers.
- ✘ the version gate.

---

## 4. Why A wins under this lens, and what keeps it from a clean sweep

A is the only proposal with no fidelity *defect* that changes on-chain outcomes:
- B, C and D share the commit-reveal version flaw.
- C overclaims refusal parity.
- B owns a parallel SCALE path.

A's weaknesses are deliberate parity choices: the core's epoch predicate, and constants from the head codec. Both are
cheap to graft. D comes within one point. Its extraction-first discipline and staged-upstream gate should be merged
into A's parity tooling rather than replace it. A's file-for-file mirror makes Python diffs land on an obvious Rust
file; D's extraction keeps the *data* half of those diffs from being hand-ported at all.

---

## 5. Unresolved problems the synthesis must address

1. **Commit-reveal version gate.** The chain treats `commit_reveal_version` as the client version and requires
   equality with `CommitRevealWeightsVersion` "to gate runtime upgrades" (`pallets/subtensor/src/subnets/weights.rs:318-337`;
   `macros/dispatches.rs:1946`). Python hard-codes 4 (`PY/intents/weights.py:33,411,472-477`). B, C and D echo the
   storage value, which would tag a v4-format tlock payload with a newer version after a root bump. Keep 4 (one
   constant per supported client format) and refuse early when storage differs.
2. **Epoch predicate conflict.** The core and Python use `blocks_since_last_step > MAX_TEMPO_U64`
   (`CORE/timelock/epoch_schedule.rs:54`); the pallet uses `BlocksSinceLastStep > tempo` (`run_coinbase.rs:1232`).
   `MaxEpochsPerBlock` deferral is modeled by no one; the +3-block `SECURITY_BLOCK_OFFSET` is empirical (C1 §10.4).
   Decide pallet-faithful vs Python-faithful explicitly, register it, and push U2/U5 upstream.
3. **Metadata version.** finney `state_getMetadata` returns V14 (357,499 B, version byte 14), and a V14 `Runtime` has
   no runtime APIs (`CORE/runtime/mod.rs:172-181`). The coexistence note's 3.9 ms measurement was of that blob.
   Require V15 via `Metadata_metadata_at_version(15)`. Fall back only on Python's missing-method needle
   (`PY/_transport/runtime.py:292-312`), and disable runtime-API and typed reads on a V14 runtime.
4. **Policy-refusal parity depends on the WS library.** jsonrpsee-client-transport 0.26.1 returns only
   `Rejected{status_code}` (`src/ws/mod.rs:205-210,522-524`), so `X-RateLimit-Policy/Reason` and `Retry-After` are
   lost. Use tungstenite (A, B) or a header-preserving soketto transport. Do not claim parity on jsonrpsee.
5. **w3f-bls patch.** Without `[patch.crates-io] w3f-bls`, the graph resolves two `w3f-bls 0.1.3` sources
   (`design-bench-nopatch/Cargo.lock:2377-2404`), and B reports E0277 in the core. A's M0 and D's Q1 "drop the patch"
   experiments cannot work as written. Ship `rtao doctor` and propose B's one-line upstream manifest change. Ciphertext
   equivalence must still be proven on localnet before downstream builds drop the patch.
6. **btauth retention check.** Python raises `ValueError` for `retention < max_age + skew` only after a verified
   signature (`PY/http_auth.py:331-339`). All four move it to construction; register that. Also map core verify
   errors to `MalformedAuth("invalid hotkey address or signature encoding")` (`:324-328`).
7. **Weight quantization must match Python bit for bit:** `round()` half-to-even and Python's float operation order
   (`PY/intents/weights.py:54-114`). C and D do not specify ties. Pin with Python-oracle vectors including `.5` ties.
8. **Nonce on `FateUnknown`.** Python clears the cache and re-asks the pool-aware `account_nextIndex`
   (`extrinsics.py:55-84`; `_substrate.py:753-772`). C's reservation diverges and can strand later nonces. Follow
   Python, with reconciliation layered on top.
9. **Zero-account `Default` maps** (`Owner`, `Keys`, `SubnetOwnerHotkey` decode to 32 zero bytes, A2 §9 Q9). B and C
   bake "→ None". Verify on localnet first, then register as a divergence with an upstream fix (A's D-001 stance).
10. **Typed decode cost.** `DecodeAsType` measured 1,380 µs at 256 UIDs and 11.0 ms at 2,500 UIDs, against 330 µs and
    2.68 ms for core `Value` (`design-bench/bench-run-3.txt`). It is correct but slower; the "typed avoids ss58, so it
    is faster" rationale in A, C and D is wrong. If miners need speed, use B's borrowed view, only behind
    differential and fuzz tests.
11. **Baked hashers.** If any of B's codegen is adopted, storage keys must still be built from the live entry's hashers
    (core `Runtime::storage_key`). Never bake hashers into generated keys.
12. **Balance text parity.** Python renders `f"{amount:,.9f}"` from a float (`PY/balance.py:146-149`). Exact-decimal
    Rust output is not byte-identical for large values. Narrow the CLI "byte-for-byte" claim to JSON, or emulate the
    float formatting.
13. **Extrinsic v5** is advertised by finney (`versions [4,5]`) but unsupported by the core and by Python. All designs
    sign v4 (correct today); the drift job must alert if Python or the core move.
14. **Report-level correction.** `sdk/bittensor-core/tests/e2e-manifest.json` has 112 = 73 `intent_*` + 39 `test_*`,
    not 72 + 40 as report B states (C repeats it).

---

## 6. Grafts the synthesis must adopt into A

From D:
1. **Extraction over hand-copy.** Every Python literal table and runtime enum comes from a generator: stdlib-`ast` or
   A's transpiler for Python; `syn` for `transaction_error.rs`, `fee_filters.rs`, `SelectiveMetagraphIndex`,
   `freeze_struct` hashes. CI asserts that Python mirrors equal the runtime sources (fee filters; `PROXY_TYPES` vs the
   metadata enum).
2. **`rtao-core-ext` staging crate.** One `UPSTREAM-TARGET` per module; `xtask staged --check` fails once the pinned
   core exports the symbol; a `core_surface` compile probe; a nightly `core-canary` against monorepo `main`.
3. **Python oracle semantics.** Compare `FakeSubstrate` composed-call trees, violations, effects, warnings and fees
   per intent sample, and read JSON as parsed values. Generate spec-current fixtures with upstream's own
   `record_golden.py` and `record_shape_corpus.py`.
4. **Receipt parity** staged with Python semantics (6 nested wrappers + MevShield events + `Treasury`/`Balances.Deposit`
   fee fallback) and upstreamed as PR-U2.

From C:
5. **Always V15 for signing and runtime APIs.** Treat a V14 `Runtime` (`apis = []`) as read-only and degraded. Store
   V16 blobs outside Python's `md*.bin` prune glob.
6. **Clippy `disallowed-methods`** for the blocking core `Client`/`Executor`, `generate_commit_v2`, blocking drand
   fetches, `epoch_schedule::predict_first_reveal_block` (once shimmed) and `save_password_to_environment`.
7. **Per-call fail-closed `verify_surface`** at connect and on every runtime upgrade. Intents whose wrapped call changed
   shape return `SurfaceChanged` at plan time; typed reads fall back to dynamic.
8. **Pallet-faithful epoch shim** with a test asserting agreement with the core on every upstream vector except the
   new safety-net vector. Delete it when U2 lands; register the Python divergence. B and D propose the same.
9. **Clock-skew check** against `Timestamp.Now` before weight setting, a reveal-miss watcher metric, and operator-first
   milestone ordering (validator, then miner, then owner) inside A's parity plan.

From B:
10. **Pin hygiene.** Use B's measured no-patch evidence: drop A's no-patch experiment, propose `w3f-bls = "=0.1.3"` in
    bittensor-core's manifest upstream, and ship `rtao doctor`, which checks that `bittensor-core` and `w3f-bls`
    resolve to the same tag in downstream `Cargo.lock` files.
11. **Type-level transaction safety:**
    - `ShieldPeriod{P4,P8}`;
    - a `ShieldCiphertext` newtype with the ≤ 8192 bound;
    - spec-stamped `CallBytes` that refuse cross-spec nesting and submission;
    - an RAII `NonceLease` whose `Drop` clears the account entry (Python's semantics, made impossible to forget);
    - refusal of `submit_signature` on a `transaction_version` change as well as a `spec_version` change.
12. **Snapshot constants.** Read constants from the snapshot's runtime, not the head codec, as a registered
    divergence (also C and D).
13. **Benchmarks before decode claims.** Criterion baselines for core `Value` vs `DecodeAsType` vs the borrowed view.
    Offer B's borrowed metagraph view only as an opt-in miner hot path, behind differential and fuzz tests.
14. **Optional typed role layer.** `KeySource<Role>` as an optional layer over A's Python-shaped executor, so
    hotkey-only binaries cannot compile coldkey intents, while the `dyn` path keeps Python's runtime role check.

---

## 7. Verification log (commands run this session, read-only)

| # | Claim | Evidence |
|---|---|---|
| 1 | Core epoch predicate vs pallet | `sed -n 45,60p CORE/timelock/epoch_schedule.rs` → `if state.blocks_since_last_step > MAX_TEMPO_U64` (:54); `sed -n 1222,1240p UP/pallets/subtensor/src/coinbase/run_coinbase.rs` → `if BlocksSinceLastStep::<T>::get(netuid) > u64::from(tempo)` (:1232) |
| 2 | Core shield era 64 vs chain max 8 | `CORE/client.rs:32` `DEFAULT_ERA_PERIOD: u64 = 64`; `:622-654` `submit_shielded` passes `Some(DEFAULT_ERA_PERIOD)` for inner and carrier; `UP/runtime/src/check_mortality.rs:19` `MAX_SHIELD_ERA_PERIOD: u64 = 8`, `:94-100` → `InvalidTransaction::Stale` |
| 3 | Version gate semantics | `UP/pallets/subtensor/src/subnets/weights.rs:318-337` ("must match … Used to gate runtime upgrades"); `macros/dispatches.rs:1946` ("The client (bittensor-drand) version"); `PY/intents/weights.py:33` `DEFAULT_COMMIT_REVEAL_VERSION = 4`, used at `:411` and `:473`, `:472-477` |
| 4 | jsonrpsee handshake error carries only a status code | `REG/jsonrpsee-client-transport-0.26.1/src/ws/mod.rs:205-210`, `:522-524`; close reasons exist as `WsError::Closed(CloseReason)` (`:243-245`, `:308`) |
| 5 | tungstenite exposes the HTTP response and close 1008 | `REG/tungstenite-0.28.0/src/error.rs:71`; `src/protocol/frame/coding.rs:154,214,245` |
| 6 | Python btauth verify path and store check | `PY/http_auth.py:55`, `:324-341`; `PY/sp_core.py:5,16`; `UP/sdk/bittensor-core-py/src/keys.rs:189-193`; `CORE/keys/mod.rs:401-410,457-469` |
| 7 | Python era anchor, transient retries, defaults | `PY/_transport/interface.py:485-499`; `PY/executor.py:52-55`; `PY/settings.py:22,29` |
| 8 | Python nonce cache | `PY/_transport/extrinsics.py:55-84` (`account_nextIndex`, `use_cache=False`, `pin`, `clear`) |
| 9 | Python metadata fetch and disk cache | `PY/_transport/runtime.py:292-312` (V15, V14 only on the missing needle); `:37,81,100,117` (`STMD`, `<IB`, `md*.bin` prune) |
| 10 | V14 runtime has no APIs; finney `state_getMetadata` is V14 | `CORE/runtime/mod.rs:172-181`; `scratchpad/meta_getMetadata.json` → 357,499 bytes, magic `meta`, version 14; `meta_v15_call.json` → 400,693 bytes, Option-wrapped |
| 11 | Receipt parity gap | `CORE/client.rs:1024-1043` (3 wrappers); `PY/_transport/utils/receipt.py:8-16` (6 wrappers) |
| 12 | Weight quantization | `PY/intents/weights.py:54-114` (`round((weight / top) * U16_MAX)`; float clipping with `epsilon = 1e-7`) |
| 13 | Balance display | `PY/balance.py:146-149` (`f"{TAO_SYMBOL}{self.amount:,.9f}"`) |
| 14 | Catalog facts | `cmp` of `UP/website/apps/bittensor-website/public/catalog/{intents,reads,errors}.json` vs `scratchpad/docs/*` → identical; intents: 83; signer 75/8; origin 74/7/2; 139 wrap entries / 116 unique; 0 properties with `default` |
| 15 | e2e manifest | `sdk/bittensor-core/tests/e2e-manifest.json` → 112 = 73 `intent_*` + 39 `test_*` |
| 16 | Owner hyperparameters | `PY/intents/hyperparameters.py:31-66` → 34 names, 33 setters |
| 17 | SelectiveMetagraph order | `metagraph.rs:121-240` struct fields (77) vs `:569-650` `from_index` arms (77): no mismatch after name normalization |
| 18 | Tag pin | `git rev-parse v477` in the local clone → `5c6e83e2f4936f257234f246957f75684c35227c`; `design-tagpin/tagpin.log` → `?tag=v477#5c6e83e2…` and the Alice address |
| 19 | No-patch duplication | `design-bench-nopatch/Cargo.lock:2377-2404` → two `w3f-bls 0.1.3` (crates.io + git) |
| 20 | Decode benchmark | `design-bench/bench-run-3.txt` (A `Value` 329.8 µs; D `DecodeAsType` 1,380.2 µs; C codec 29.8 µs; E view 6.5 µs at 256 UIDs) |
| 21 | Library facts used by C and D | `REG/scale-decode-derive-0.16.2/src/lib.rs:222-236,356-371`; `REG/scale-type-resolver-0.2.0/src/portable_registry.rs:59`; `REG/subxt-rpcs-0.51.1/src/client/rpc_client_t.rs:26`, `methods/legacy.rs:144,284,309,368`, `reconnecting_rpc_client/mod.rs:9`; `CORE/transaction.rs:201-213`; `inference-labs-inc_subnet-2/crates/sn2-chain/src/lib.rs:28-31` |
| 22 | Core `coerce_account_id` and call splicing | `CORE/codec/encode.rs:488-535` (ss58, 0x-hex or 32 raw bytes; `Value::Bytes` spliced verbatim) |
| 23 | Value helpers, IR, bytes rendering | `CORE/client.rs:1515-1570`; `bittensor-core-py/src/runtime.rs:554,579`; `CORE/codec/decode.rs:812-817` |
| 24 | Fee filters | `UP/runtime/src/fee_filters.rs:15-34` (`call_filter_group!(ColdkeyPaysFeeCalls, …)`) |
| 25 | Python fee-cap semantics | `PY/intents/plan.py:51-72` (fee `None` with a cap ⇒ violation); `PY/executor.py:744-756` (plan: estimate failure ⇒ warning, then the violation); `:936-952` (shield: `PolicyError`) |
