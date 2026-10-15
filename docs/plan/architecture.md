# rtao: final architecture (synthesis)

Date: 2026-10-13, revised through 2026-10-14 with the user's decisions (§15.2). Author: the synthesis step of the rtao
design panel. Reader: an expert Rust/Substrate engineer who knows FRAME and subtensor internals.

This document is the single architecture for **rtao**, a set of Rust crates that lets Bittensor subnet owners build
validators, miners and owner tooling entirely in Rust, with full parity with the v11 Python package
(`sdk/python/bittensor` in the RaoFoundation monorepo). It merges four proposals (A mirror-Python, B type-driven,
C operator-first, D minimal-custom-surface) under the verdicts of three judges (chain fidelity, maintainability,
performance/DX), and it resolves every concern those judges raised (Appendix A) and every graft they requested
(Appendix B).

User decisions that are fixed and not re-argued here:

1. crates.io publishability is not a goal. Git dependencies (including a pinned git dependency on
   `sdk/bittensor-core` and a replicated `[patch.crates-io] w3f-bls`) are fine.
2. rtao is a veneer over `bittensor-core`: everything chain-defined comes from the official Rust core.
3. Scope is full parity with the v11 Python package layer, phased but complete.
4. The answers of 2026-10-13 to the open questions (§15.2): a dual `MIT OR Apache-2.0` license; Python only in CI
   lanes; the binary `rtao` only; btauth/1 verification that accepts plain and `<Bytes>`-wrapped signatures by default;
   the repository `github.com/sam0x17/rtao`; hardware acceptance deferred by Q30, with optional hosted emulator
   evidence (§11.9); each upstream PR and issue filed as soon as its evidence exists; per-milestone
   security passes by local cold readers; no time estimates. Three later answers of the same day (§15.2 Q20-Q22):
   until PR-U17 ships, rtao's own workspace builds `bittensor-core` through a Cargo patch that adds it (§1.11); an AI
   coding agent develops M0 to M9 as one goal loop, committing to `main` without PRs (PLAN.md "Executing this plan");
   and the Ledger app fix was filed with Zondax before the plan runs (Appendix D).
5. The 2026-10-14 prerequisite answer (§15.2 Q23): use `sam0x17` GitHub/GHCR and only free `ubuntu-latest` hosted
   CI; ship rustdoc and the book as downloadable release assets without Pages. The user offers testnet funding to
   a prepared address and has a Ledger. Q30 later removes local infrastructure, cross-platform build and
   hardware-rehearsal requirements; current execution scope below governs.

## Current execution scope (Q30, 2026-10-14)

The user has restricted this task to client development with substantial tests in free `ubuntu-latest` GitHub CI.
[PLAN.md's current execution scope](../../PLAN.md#current-execution-scope-q30-2026-10-14) governs this document and
supersedes older local validation, milestone release and infrastructure requirements. Ordinary acceptance uses
Rust units, goldens, committed oracle vectors, property tests, `FakeNode`, `FakeChain` and relevant integration tests.

Local work is source editing and review with cheap checks. Do not run local full nodes, localnets, testnet workloads,
Docker/oracle containers, soaks, hardware/Keychain rehearsals, fuzz/benchmark campaigns or release builds. Do not
create parallel Cargo jobs, duplicate target trees or extra toolchains, or use Mac Studio as a build host. Any
necessary diagnostic compilation is narrowly scoped and sequential; prefer hosted CI. Keep disposable work in one
recorded task root, audit dependencies before cleanup and preserve authored source and durable user state.

Container-based oracle, interop, emulator or localnet designs below are references for optional targeted hosted
checks with timeouts and free-runner limits. They cannot create local execution requirements. Mainnet cloning and
full-node operation are removed. Per-milestone releases, cross-platform binary builds, hardware acceptance and
upkeep/deployment setup are deferred. State unexecuted platform and interoperability coverage; retain API and safety
requirements without claiming the removed campaigns passed.

Only `.github/workflows/pr.yml` is active. Eleven older infrastructure, scheduled, oracle and release workflows are
inactive drafts in `.github/deferred-workflows/`; their triggers cannot run. Any future targeted check needs a
reviewed, bounded hosted job.

## Evidence conventions

Citations use these prefixes. Upstream paths are pinned to the monorepo commit the design was checked against, so every
`file:line` stays reproducible after upstream moves on.

| Prefix | Location |
|---|---|
| `UP/` | `RaoFoundation/subtensor` at commit `850f2755bb05a3851f948489f0ab27b0645cebce` (<https://github.com/RaoFoundation/subtensor/tree/850f2755bb05a3851f948489f0ab27b0645cebce>; spec 478 on `main`; finney and testnet ran 477 on 2026-10-13 until `850f2755` was released as `v478` later that day and both moved to 478; PLAN.md M0-06 pins `v478`) |
| `CORE/` | `UP/sdk/bittensor-core/src/` |
| `PY/` | `UP/sdk/python/bittensor/` |
| `PYT/` | `UP/sdk/python/tests/` |
| `CAT/` | `UP/website/apps/bittensor-website/public/catalog/` (byte-identical to <https://www.bittensor.com/catalog/>, [J-CF §7 #14]) |
| `REG/` | the crates.io source of the named crate version (`~/.cargo/registry/src/index.crates.io-*/<crate>-<version>/`) |
| `RES/` | [`docs/plan/research/`](research/) (reports A1, A2, B, C1, C2, D, E, F, G, H, I, `CORRECTIONS.md`, `feasibility-*.md`, and [`J-ledger-ci-shim.md`](research/J-ledger-ci-shim.md), the Ledger-in-CI research done after the user's decisions) |
| `DOTAPP/` | the Polkadot generic Ledger app, `Zondax/ledger-polkadot` at commit `644ec63851072f6eac9c3911ea23a811f3a1b15c` (tag `v100.0.29`, <https://github.com/Zondax/ledger-polkadot/tree/644ec63851072f6eac9c3911ea23a811f3a1b15c>); the Speculos emulator is cited by URL in `RES/J-ledger-ci-shim.md` |
| `P-A` … `P-D` | [`docs/plan/panel/`](panel/)`arch-{A-mirror-python,B-type-driven,C-operator-first,D-minimal-custom-surface}.md` |
| `J-CF`, `J-MA`, `J-PD` | [`docs/plan/panel/`](panel/)`judge-{chain-fidelity,maintainability,performance-dx}.md` |
| `SP/` | the design session's scratch workspace (not retained). The artifacts that matter were copied: `SP/design-bench*` and `SP/design-tagpin` → [`docs/plan/bench/`](bench/), `SP/research` → `RES/`, `SP/design` → [`docs/plan/panel/`](panel/), `SP/upstream/subtensor` → `UP/`. Raw metadata dumps (`SP/meta*`) are reproducible with `subxt metadata` against the named network and spec |

Markers: **[V]** re-verified on disk by the synthesizer in this session (Appendix F lists every check); **[J]** verified
by a judge (report and section cited); **[R]** from a research report; **[P]** a proposal's claim adopted without
re-verification; **[I]** inference or size estimate, not executed (the design carries no time estimates, §14).
`RES/CORRECTIONS.md` overrides the research reports: the spec-478 fee-extension identifier did not change on the wire,
spec 478 changes no client-visible metadata, and the "do not depend on bittensor-core" recommendations of reports
B/E/F/G are superseded (their technical points stand and are all addressed below).

---

## 0. Synthesis summary

### 0.1 Which proposal is the base, and why

The three judges picked three different winners. Their scores combine as follows:

| Proposal | Chain fidelity (J-CF) | Maintainability (J-MA) | Performance/DX (J-PD) | Sum | Mean rank |
|---|---:|---:|---:|---:|---:|
| A mirror Python | **42** (1st) | 37 (4th) | 32 (4th) | 111 | 3.00 |
| B type-driven | 36 (4th) | 37.5 (3rd) | 38 (2nd=) | 111.5 | 3.17 |
| C operator-first | 39 (3rd) | 38 (2nd) | **39** (1st) | 116 | 2.00 |
| **D minimal custom surface** | 41 (2nd) | **40** (1st) | 38 (2nd=) | **119** | **1.83** |

**D is the consensus base**: highest total and best mean rank, never below second place. From D this design keeps the
thesis "rtao owns only I/O, generators, ported behavior and staged upstream code", the single-pin discipline, extraction
instead of hand-copying, generated intent interfaces with compile-time completeness, the per-pin Python oracle, and the
`rtao-core-ext` staging crate with its deletion gate.

Each judge's lens contributes the parts where its winner was strongest:

- **From C (performance/DX winner)**: the operator runtime (`NeuronRuntime`, `WeightScheduler`, `AxonPublisher`,
  `MetagraphSync` with an `ArcSwap` handle, `OwnerOps`, health, metrics, shutdown, safety rails), the single
  anti-corruption crate that alone imports `bittensor_core`, `clippy` `disallowed-methods`, fail-closed per-call surface
  verification, the always-V15 metadata rule, `deny.toml` stack bans, the `Scenario` localnet builder, and
  operator-first milestone ordering.
- **From A (chain-fidelity winner)**: the tokio-tungstenite transport that reproduces `rpc.py`'s refusal contract, the
  Python-faithful write path (finalized-anchored era 128, immortal zero-signature fee probe, nonce semantics, receipt
  port), `commit_reveal_version = 4` with an early refusal when storage differs, `parity/map.toml` with git blob SHAs, the
  Rust-native Python AST symbol diff, `#[doc(alias = "bittensor.…")]`, and `f64::round_ties_even` weight quantization.
- **From B (best measurements and types)**: the decode benchmark and its consequences, the shape-gated borrowed
  `MetagraphView`, `KeySource<Role>` compile-time roles, `Balance<Tao>`/`Balance<Alpha>`, transaction-safety types
  (`NonceLease`, `ShieldPeriod`, `ShieldCiphertext`, spec-stamped `CallBytes`, typed `PoolRejection`, `ReadMethod`),
  codegen semantic overrides (`PriceRao`, `StakeWeight`), the measured `w3f-bls` patch requirement, `rtao doctor`, and
  the one-line upstream manifest fix.

What this design **rejects** from the proposals, with the evidence, is listed in the decision log (§16): B's positional
codec types and baked hashers for everything, C's 32-block weight-commit era and nonce reservation on `FateUnknown`,
B/C/D echoing `CommitRevealWeightsVersion`, A's inheritance of the core's epoch predicate, A's and D's "drop the
`w3f-bls` patch" experiments, jsonrpsee as the WebSocket client, and Python in the regeneration path.

### 0.2 The architecture in one page

- **Chain-defined bytes come only from `bittensor-core`**: keys and ss58, keyfiles, the metadata model and SCALE engine,
  the signature payload in metadata extension order, v4 extrinsic assembly, era math, multisig derivation, the RFC-0078
  digest and Ledger proof, the ML-KEM shield envelope, drand tlock, and the PoW seal. Two crates form the seam:
  - `rtao-core` is the only crate that imports `bittensor_core`;
  - `rtao-core-ext` holds code written as if it were already upstream, one module per upstream PR, each deleted when the
    pinned core exports it.
- **Python-defined product behavior is ported from Python, never taken from the core's product layer.** The core's
  blocking `Client`, `Executor`, `IntentCall` and `Policy` are differential oracles in tests only, and a `clippy`
  `disallowed-methods` list keeps them off runtime paths. The parity target is `sdk/python/bittensor` at `parity_tag`.
- **Two pins, one file.** `upstream.toml` records `core_tag` (what Cargo builds, and the tag every downstream
  `[patch.crates-io] w3f-bls` line must match) and `parity_tag` (what rtao mirrors: catalogs, Python sources, goldens,
  oracle, localnet image). `parity_tag` moves on every finalized mainnet release. `core_tag` moves only when the core's
  inputs change. Its sources changed in 1 of the last 6 release pairs, and a second pair touched only tests
  [J-MA §1].
- **Async I/O is rtao's**: a tokio-tungstenite JSON-RPC session (`rtao-rpc`) with `rpc.py` semantics and TLS from one
  explicitly built `ring` rustls config (§2.7), exposed through `subxt_rpcs::client::RpcClientT` so `LegacyRpcMethods`,
  mocks and an optional subxt `OnlineClient` work on the same connection. On top sit a runtime cache that requires V15 metadata, block-pinned snapshots, coalesced reads, a
  submission pipeline with RAII nonce leases, the MEV-shield path, and gap-free head streams.
- **Generated, not hand-copied**: intent and read interfaces, dispatch enums, error tables and literal tables come from
  the vendored catalogs and from Python and Rust sources. Extraction is Rust-native: `rustpython-parser` for Python,
  `syn` for Rust. Chain descriptors and typed call builders come from pinned V15 metadata, are verified per live runtime
  with structural shape hashes, and never bake hashers.
- **Ported, oracle-checked behavior**: intent `build`/`preflight` bodies, the executor, reads, the CLI command groups.
  A CI-only Python oracle at `parity_tag` produces committed vectors. PR gates run offline and need no Python.
- **Typed where it pays, dynamic where Python's shapes are the contract**:
  - by-name `DecodeAsType` models over the core's `PortableRegistry`;
  - a borrowed, shape-gated `MetagraphView` for the one hot structure;
  - the core `Value` for JSON, the CLI and agent surfaces.
- **Operator framework** (`rtao-neuron`): validator and miner runtimes with weight scheduling, reveal watching,
  idempotent axon publishing, metagraph sync, btauth/1, metrics, health and safety rails.
- **Neuron binaries build without Perl, OpenSSL, libsodium or CMake**: a no-`host` profile with a staged
  plaintext-hotkey reader. Their only native code is `ring` and `blake3`'s SIMD kernels, built by the C compiler rustc
  already uses as its linker (§1.7). Owner and CLI builds keep `host` for encrypted coldkeys and PoW.

### 0.3 Twenty-five decisions that define the design

| # | Decision | Section |
|---|---|---|
| 1 | Base = D; C's operator runtime and seams, A's transport and fidelity rules, B's types and measurements | §0.1 |
| 2 | Seam = `rtao-core` (only importer of `bittensor_core`) plus `rtao-core-ext` (staged upstream code with a deletion gate) | §1.1, §2 |
| 3 | Two tags in `upstream.toml`: `core_tag` (moves on a core-input fingerprint change) and `parity_tag` (every finalized release) | §1.2, §12.2 |
| 4 | Downstream `[patch.crates-io] w3f-bls` is mandatory and checked by `rtao doctor`; no "drop the patch" experiment; the one-line upstream manifest change is proposed, and it removes the patch only after a localnet decrypt proof | §1.3 |
| 5 | Neuron binaries build without `bittensor-core/host`; a staged plaintext keyfile reader is differential-tested against the core | §1.7, §2.4, §6.1 |
| 6 | Transport: own tokio-tungstenite session behind `RpcClientT`, with one explicitly built `ring` rustls config (never the process-default provider lookup); jsonrpsee is not in the default graph | §2.7, §3.2 |
| 7 | Reconnect resends only methods in a `ReadMethod` allowlist; `author_submit*` is structurally absent, and a queued frame whose caller has gone is never transmitted | §3.2 |
| 8 | Policy refusals are classified exactly as `rpc.py` classifies them, from extracted constants, with headers from the handshake response | §3.2 |
| 9 | V15 metadata always; V14 only on Python's missing-method needle; a V14 runtime is degraded (no signing; runtime APIs only through the ported frozen legacy table) | §3.4 |
| 10 | Generated descriptors are name-based; storage keys always come from the live entry via the core; shape hashes only verify | §3.7 |
| 11 | Exact shape hashes for calls and the positional metagraph view; projection hashes (declared fields only) for by-name models; the outer `RuntimeCall`/`RuntimeEvent`/`RuntimeError` enums are opaque; fail-closed per call | §3.7 |
| 12 | One encoder (`Runtime::compose_call`); no codec-derive encode path | §3.8 |
| 13 | Coalesced reads: heterogeneous keys in one `state_queryStorageAt` at one pinned hash, chunked at 256 keys | §3.6 |
| 14 | Nonces follow Python (clear on any failure, re-ask the pool-aware `account_nextIndex`), enforced by an RAII `NonceLease`; one `NonceManager` per `Client`, shared by every clone, executor and service; reconciliation is layered on top, never a reservation | §3.9 |
| 15 | Python's era defaults everywhere (128, anchored at the finalized head), including weight commits; `ShieldPeriod { P4, P8 }` is the only shield era type, and a shielded submission whose era the finality lag would expire before inclusion is refused (D-025) | §3.9, §3.11 |
| 16 | Receipts use Python's semantics (6 nested wrappers, MevShield failure events, fee fallback), staged as `ops::receipt` and upstreamed; the `retracted` and `finalityTimeout` watch statuses are resolved instead of reported as failures (D-024) | §3.9 |
| 17 | Intents: generated interfaces and dispatch, generated pass-through `build()` for 46 of 83 (the generator lands in M2), hand-ported bodies; `AnyIntent` plus a shrink-only `pending.toml` | §4.2, §4.3 |
| 18 | Compile-time signer roles (`KeySource<Role>`), with a `dyn` path that keeps Python's runtime check | §4.2, §6.4 |
| 19 | `commit_reveal_version` stays 4 (one constant per supported tlock payload format); the framework refuses early when storage differs; the intents are unchanged from Python | §9.3 |
| 20 | Reveal-round prediction reads the schedule at the best head, as Python does, and uses a pallet-faithful epoch predicate with an injectable clock and a clock-skew guard; the predicate is a registered Python divergence with an upstream fix | §1.6, §9.3 |
| 21 | Weight quantization uses `f64::round_ties_even` and Python's summation order, pinned by oracle vectors with `.5` ties | §9.3 |
| 22 | `Balance` text emulates Python's float formatting exactly, so read JSON is value-equal to `btcli --json` | §5.3 |
| 23 | Python appears only in CI (oracle, interop, recorders, and the Speculos of the Ledger emulator lane); extraction and regeneration are Rust-only; PR gates are offline | §11.5, §11.9, §12 |
| 24 | Parity is enforced mechanically: blob-SHA file map, symbol diff, catalog equality, extraction check, environment-variable map, coverage gate, oracle, shrink-only `pending.toml`, divergence register | §4.9 |
| 25 | Operator-first milestones: validator, then miner, then owner, then intent, read, signer and CLI parity; milestones are defined by scope, dependencies and exit checks, without time estimates | §14 |

### 0.4 What rtao owns (budget)

[I] Estimates for scale. The v11 Python package is 59,723 `.py` lines [J-PD §3 D#1].

| Kind | Where | Est. Rust lines | Who keeps it correct |
|---|---|---:|---|
| Reused unchanged | `bittensor-core` (~13.6k lines with tests, [R] A1 §1) | 0 | upstream |
| Staged for upstream | `rtao-core-ext` | ~2k | upstream style and lints; deleted as PRs merge |
| Generated | `rtao-catalog`, `rtao-runtime/generated`, `rtao-intents/generated`, `rtao-reads/generated`, docs | ~14k | diff review only |
| I/O and runtime | `rtao-rpc`, `rtao-client`, `rtao-signer`, `rtao-wallet`, `rtao-btauth` | ~13k | rtao, tested on `FakeNode` and localnet |
| Ported behavior | intent bodies, executor, read bodies, CLI groups | ~27k | Python oracle at `parity_tag` |
| Framework | `rtao-neuron`, `rtao-metagraph` | ~7k | rtao (v11 ships no framework) |
| Tooling | `xtask`, `rtao-testkit` | ~8k | rtao |

---

## 1. Boundary with bittensor-core

### 1.1 Principle: who defines the bytes

`bittensor-core` sorts its own modules into CHAIN (bytes the chain defines), COMPAT (bytes an external ecosystem
defines: btwallet, polkadot-js, Ledger) and PRODUCT (`client.rs`, `transaction.rs`) [R] B §3.14. The rule here is
A's, sharpened by D:

1. **Every CHAIN and COMPAT byte comes from the core.** rtao re-implements no byte format. The golden vectors pin the
   core, and rtao replays them *through its own wrappers* (§11.1), so the seam is tested too.
2. **Product behavior comes from Python, not the core.** The parity target is `PY/` at `parity_tag`. The core's
   product layer diverges from Python on era, anchor, shield era, fee RPC, receipts, error codes and intent coverage
   (P-A §1.4 table, [J-CF §3.1]). It is used only as a differential oracle (§11.8).
3. **Missing chain-adjacent logic is written as upstream code.** rtao does not keep private forks of code that subnets
   run; the one patch it carries, PR-U17 for its own Ledger test lane, never reaches them (§1.11). Code that belongs in
   the core lives in `rtao-core-ext`, which follows upstream style and lints, carries an `UPSTREAM-TARGET` header per
   module, and is deleted when its upstream probe passes against the pinned core (`xtask staged --check`, §1.6).

Two crates implement the rule:

| Crate | Role | May depend on |
|---|---|---|
| `rtao-core-ext` | staged upstream code: sans-IO protocol functions, receipt parity, metadata IR, pallet-faithful epoch schedule, plaintext keyfile reader, pure env-password encoder, strict verify, `Value` helpers | `bittensor-core` (no default features); `serde`, `serde_json`, `parity-scale-codec`, `hex`, `base64` and `zeroize` (portable env-password buffers, core manifest lines 25 and 49); `scale-info` and `frame-metadata` (IR; the core re-exports no `scale_info`, `CORE/runtime/mod.rs:145, 257`); `schnorrkel` and `ed25519-zebra` (`verify_strict`); `subtensor-macros` (git, `core_tag`; the upstream lint requires `#[freeze_struct]` on SCALE structs, `UP/build.rs:72`). Each at the version and source already in the core's graph; `deps-check` checks the actual staged normal/build graph and rejects a second built copy. Benchmark-only dependencies remain dev-only. Python diagnostic printability uses authenticated Unicode 15 data without an additional staged dependency. Hashing goes only through `bittensor_core::codec::storage::hash_param` and `mlkem::twox_128` (no crates.io `sp-crypto-hashing` next to the fork's git copy). **No tokio** |
| `rtao-core` | anti-corruption layer and the **only** crate that names `bittensor_core` in non-test code. It re-exports a curated surface, maps `CoreError`, hosts `offload` (spawn_blocking), the `core_surface` probe and `BITTENSOR_CORE_TAG` | `bittensor-core`, `rtao-core-ext`, `tokio` (feature `offload`, default on) |

`rtao-core` re-exports each symbol from wherever it currently lives:

```rust
// crates/rtao-core/src/epoch.rs
// Staged until PR-U5 ships in core_tag; then this becomes `pub use bittensor_core::timelock::epoch_schedule::*;`
// and `cargo xtask staged --check` fails until rtao-core-ext/src/epoch_schedule.rs is deleted.
pub use rtao_core_ext::epoch_schedule::{EpochScheduleState, predict_first_reveal_block, should_run_epoch};
```

When upstream absorbs a staged module, one `pub use` line flips and no other crate changes, provided the upstream symbol
exists without `host`: a symbol that lands only behind `host` does not retire a module the no-host profile uses (§1.6,
Appendix D). The other crates that may name `bittensor_core` are `xtask` (codegen, extraction, oracle builds) and
`rtao-testkit` (the `core-oracle` differential lane, under `#[allow(clippy::disallowed_methods)]`). `cargo xtask
deps-check` enforces this from `cargo metadata` (§2.3).

### 1.2 Pinning: `core_tag` and `parity_tag`

All four proposals bump the core git pin on every finalized release. Because the downstream `w3f-bls` patch must use
rtao's core tag exactly (§1.3), that would make every subnet repository edit its manifest 2-4 times a week, although
the core's sources changed in only 1 of the last 6 finalized release pairs; one more pair touched only its tests
[J-MA §1, §4.1]. The fix is two tags in
`upstream.toml`, the single machine-readable pin file read by `xtask` and CI:

```toml
# upstream.toml — written only by `cargo xtask pin` (and, for [core.patch], `cargo xtask core-patch`); never edited by hand
[monorepo]
url = "https://github.com/RaoFoundation/subtensor.git"

[core]                                  # what Cargo builds (and what downstream [patch] lines must match)
tag = "v477"
commit = "5c6e83e2f4936f257234f246957f75684c35227c"   # `git rev-parse v477` [J-CF §7 #18]
toolchain = "1.89"                       # UP/rust-toolchain.toml at core.tag [J-MA §3.4]
fingerprint = "b3:…"                     # core-input fingerprint at core.tag (definition below)

[core.patch]                            # from M7-09 until core.tag contains PR-U17 (§1.11); absent otherwise
repo = "https://github.com/sam0x17/subtensor.git"
branch = "rtao-core-patch/v477"
rev = "…"                               # full commit: core.tag plus the PR-U17 commits
base = "5c6e83e2f4936f257234f246957f75684c35227c"     # = core.commit
upstream_pr = "https://github.com/RaoFoundation/subtensor/pull/…"
paths = [                               # the files PR-U17 may touch (Appendix D)
  "sdk/bittensor-core/src/signers/mod.rs",
  "sdk/bittensor-core/src/signers/ledger.rs",
  "sdk/bittensor-core/src/signers/hid.rs",
]
probe = [                               # present in the unpatched core = PR-U17 shipped (§1.11)
  "bittensor_core::signers::ApduTransport",
  "bittensor_core::signers::ledger::LedgerDevice::with_transport",
]

[parity]                                # what rtao mirrors; moves on every finalized mainnet release
tag = "v477"
commit = "5c6e83e2f4936f257234f246957f75684c35227c"
spec_version = 477
python_package_version = "11.3.1.dev0"   # source version from sdk/python/pyproject.toml at parity.tag; not a PyPI release (§11.5) [P-A App. B]

[localnet]
image = "ghcr.io/raofoundation/subtensor-localnet"
pinned  = { tag = "v477",    digest = "sha256:…" }     # per-spec images exist for v432, v473, v475, v477 [R] H §1.3; digest written by `xtask pin --parity vN --check` (§12.3)
mainnet = { tag = "mainnet", digest = "sha256:…" }
testnet = { tag = "testnet", digest = "sha256:…" }
devnet  = { tag = "devnet",  digest = "sha256:…" }
main    = { tag = "main",    digest = "sha256:…" }     # optional targeted hosted comparison; no scheduled campaign

[networks.finney]
genesis = "0x2f0555cc76fc2840a25a6ea3b9637146806f1f44b090c175ffde2a7e5ab36c03"
metadata = { spec = 477, v15 = "vendor/metadata/finney/477.v15.scale", v16 = "vendor/metadata/finney/477.v16.scale", code_hash = "0x…" }
# [networks.testnet], [networks.devnet] likewise

[artifacts]                             # monorepo path at parity.tag -> vendored path + sha256 (verified in pr-gates)
"website/apps/bittensor-website/public/catalog/intents.json" = { to = "vendor/upstream/catalog/intents.json", sha256 = "…" }
"website/apps/bittensor-website/public/catalog/reads.json"   = { to = "vendor/upstream/catalog/reads.json",   sha256 = "…" }
"website/apps/bittensor-website/public/catalog/errors.json"  = { to = "vendor/upstream/catalog/errors.json",  sha256 = "…" }
"sdk/python/tests/fixtures/golden.json"                      = { to = "vendor/upstream/golden.json",          sha256 = "…" }
"sdk/python/tests/fixtures/shape_corpus/corpus.json"         = { to = "vendor/upstream/corpus.json",          sha256 = "…" }
"sdk/bittensor-core/fixtures/ledger_proof_vector.json"       = { to = "vendor/upstream/ledger_proof_vector.json", sha256 = "…" }
# … PY/vault/static/*, PY/extension/static/bridge.html, PY/evm/abi/*.json, docs/guides/signed-requests.mdx
# … vendor/upstream/tree.txt and vendor/upstream/src/<path> for every source the extraction and chain-source layers read
```

```toml
# rtao/Cargo.toml (workspace root) — the `tag` values of the first two tables are rewritten by `cargo xtask pin` from
# upstream.toml [core], and the last two tables by `cargo xtask core-patch` from [core.patch] and [core]
[workspace.dependencies]
bittensor-core = { git = "https://github.com/RaoFoundation/subtensor.git", tag = "v477", default-features = false }
# Every other monorepo git dependency is declared here too (rtao-core-ext and xtask inherit them with
# `workspace = true`), so a core bump rewrites this one manifest
subtensor-macros = { git = "https://github.com/RaoFoundation/subtensor.git", tag = "v477" }
subtensor-linting = { git = "https://github.com/RaoFoundation/subtensor.git", tag = "v477" }

[patch.crates-io]
# Cargo ignores [patch] tables in dependencies, so every final workspace must repeat this at the SAME tag
# (RES/feasibility-bittensor-core-gitdep.md). Without it Cargo locks two w3f-bls 0.1.3 sources and bittensor-core fails
# with E0277 (SP/design-bench-nopatch/Cargo.lock:2377-2404 [V]; SP/design-bench/target-nopatch/... E0277 [V]).
w3f-bls = { git = "https://github.com/RaoFoundation/subtensor.git", tag = "v477" }

# PR-U17 until core_tag contains it ([core.patch] in upstream.toml; checked by `cargo xtask pin --check`)
# (from M7-09; only this root manifest carries it, so subnet workspaces never see it, §1.11)
[patch."https://github.com/RaoFoundation/subtensor.git"]
bittensor-core = { git = "https://github.com/sam0x17/subtensor.git", rev = "<full commit>" }

# The patched core's other path dependencies, kept at core_tag: without this table the fork's copies join the graph,
# a second w3f-bls 0.1.3 (E0277, as above) and a second subtensor-macros (rtao-core-ext names it) (§1.11)
[patch."https://github.com/sam0x17/subtensor.git"]
w3f-bls = { git = "https://github.com/RaoFoundation/subtensor.git", tag = "v477" }
subtensor-macros = { git = "https://github.com/RaoFoundation/subtensor.git", tag = "v477" }
```

**Vendored upstream sources.** Besides `[artifacts]`, `xtask import-upstream` vendors, at `parity.commit`:

- `vendor/upstream/tree.txt`, the output of `git ls-tree -r <parity.commit> -- <watched roots>` (§4.9);
- every source file read by the extraction layer (`map.toml` rows of kind `extracted`, for example `PY/settings.py`,
  `PY/_transport/rpc.py`, `PY/executor.py`, `PY/error_map.py`, `PY/hyperparams.py`, `PYT/harness/fake_substrate.py`
  and `PYT/harness/samples.py`) or by the chain-source layer (`common/src/transaction_error.rs`,
  `runtime/src/fee_filters.rs`, `runtime/src/check_mortality.rs`, `pallets/subtensor/src/rpc_info/**` and the
  chain-rule files of §4.9 layer 5), under `vendor/upstream/src/<monorepo path>`, each with a sha256 row in
  `[artifacts]` that `--verify-only` checks.

On ordinary pushes the file, extraction and chain-source layers (§4.9 layers 1, 4 and 5) therefore read only `vendor/`
and need no network. The symbol layer parses old and new blobs only for rows whose `synced_blob` differs from
`tree.txt`, which happens only when `parity.commit` moves (`pin-bump`, `upstream-next`); those jobs use the cached
partial clone.

**Core-input fingerprint.** `xtask` computes, at a tag, the BLAKE3 hash of:

1. for each package in the path-dependency closure of `sdk/bittensor-core/Cargo.toml` (today `sdk/bittensor-core`,
   `support/macros` and `vendor/w3f-bls`): the git tree SHA of `src/` and the blob SHAs of `Cargo.toml` (and
   `build.rs`, if one appears), from `git ls-tree`, which is cheap on a partial clone. `tests/`, `benches/` and
   `fixtures/` are excluded: Cargo never builds them for a dependency, and fixtures are vendored from `parity_tag`
   anyway;
2. the TOML-normalized `[workspace.dependencies]` entries those packages inherit with `workspace = true` (parsed with
   `toml_edit`; today `sp-core`, `tle`, `w3f-bls`, `frame-metadata`, `scale-info`, `subtensor-macros`, `syn`, `quote`,
   `proc-macro2` and `ahash`; `codec` is declared directly, `UP/sdk/bittensor-core/Cargo.toml:55`), and the
   `[workspace.package]` keys they inherit (`edition`);
3. the root `[patch.crates-io]` entries for crates in the core's resolved graph only (today `w3f-bls`; the `zstd-sys`
   and `zstd-safe` entries of `UP/Cargo.toml:334-337` are outside it).

`xtask` derives the closure from the manifests, so a new path dependency is picked up automatically
(`subtensor-macros` resolves to `support/macros`, `UP/Cargo.toml:74`, and is built for rtao). Everything else
(pallets, runtime, Python, docs, and patches for crates outside the core's graph) cannot change what Cargo builds for
rtao and does not move `core_tag`. The monorepo's `Cargo.lock` does not reach dependents; the oracle builds with it
(see the end of this section).

**Bump procedure** (`cargo xtask pin --parity vN`, run by the agent during the goal loop and by the `pin-bump` job after
it, §12.3):

1. Refuse unless release `vN` is final (not a pre-release; v470, v471 and v476 were abandoned [R] H §1.2) **and**
   finney's finalized runtime reports `spec_version == N` with the expected `:code` hash (the RPC sequence of
   `UP/.github/scripts/mainnet-finalized-runtime.sh:19-26` [R] H §2.1) **and** the `:vN` localnet image is published
   (§12.3 step 1; upstream builds it after the mainnet promotion, 32 minutes later for v477).
2. Set `[parity]` to vN. Re-vendor `[artifacts]`, re-run extraction and regeneration, refresh oracle vectors and
   recorded fixtures (§12.3).
3. Compute `fingerprint(vN)`. If it equals `core.fingerprint`, leave `[core]` alone: rtao keeps building the
   byte-identical core at the old tag. Otherwise set `[core]` to vN and rewrite every reference generated from
   `[core]`: the `tag =` values of the root `Cargo.toml`'s `[workspace.dependencies]` (`bittensor-core`,
   `subtensor-macros`, `subtensor-linting`; no member manifest names the monorepo) and `[patch.crates-io]`,
   `templates/subnet/Cargo.toml`, and the README and book quickstart snippets. A test asserts that every monorepo git
   dependency and every `w3f-bls = { git = …, tag = "…" }` in the repository carries `[core].tag`. While the core
   patch exists, `cargo xtask core-patch rebase vN` moves it to the new tag, or the bump drops it when vN contains
   PR-U17 (§1.11). Then run `cargo update -p bittensor-core -p w3f-bls -p subtensor-macros -p subtensor-linting`, run
   the `core_surface` probe (§1.9), and mark the bump `core-bump` plus `downstream-patch-change` (PR labels after the
   goal loop). The release notes then tell subnet repositories to update their patch line, and `rtao doctor` prints
   the exact line.
4. The porting report (§4.9) plus the core diff `git diff <old core>..vN -- sdk/bittensor-core` is the commit body
   during the goal loop and the PR body after it.

The oracle (§11.5) builds `bittensor-core-py` from `parity.tag`. When the fingerprints match, its core sources are
byte-identical to `core.tag`'s, so the two-tag scheme does not invalidate the oracle.

Third-party versions are a separate matter. `bittensor-core-py` is a member of the monorepo workspace
(`UP/Cargo.toml:35`) and builds with `UP/Cargo.lock`, while rtao and every subnet workspace resolve their own locks. At
v477, 88 of the 241 packages in the core's no-host graph resolve to versions absent from `UP/Cargo.lock`, for example
ml-kem 0.2.2/0.2.3, ed25519-zebra 4.1.0/4.2.0, scale-decode 0.16.0/0.16.2 and serde_json 1.0.145/1.0.154 (aligning the
whole closure is infeasible: `UP/Cargo.lock`'s tracing 0.1.41 is below what tracing-subscriber 0.3.23 needs).
`xtask pin` therefore records `UP/Cargo.lock`'s versions of the byte-defining crates in the core's graph in
`vendor/oracle/lock.json`: `parity-scale-codec`, `scale-info`, `scale-decode`, `frame-metadata`,
`merkleized-metadata`, `schnorrkel`, `ed25519-zebra`, `bip39`, `sha2`, `sha3`, `blake2`, `twox-hash`, `ml-kem`,
`chacha20poly1305`, `tle`, `ark-*` and `serde_json`. `deps-check` prints rtao's differences from that set as a
warning. The oracle comparison runs offline in `pr-gates` (§4.9 layer 7), so a dependency bump that changes behavior
fails CI, and such a mismatch is checked against `lock.json` before anything is ported.

### 1.3 The `w3f-bls` patch, end to end

The patch is mandatory today. Two judges flagged A's M0 and D's Q1 experiments ("build without the patch") as unable
to compile [J-CF §5.5, J-MA §4.1, J-PD §2.3]:

- the core declares `w3f-bls = { workspace = true, features = ["std"] }` (`UP/sdk/bittensor-core/Cargo.toml:62` [V]),
  which resolves to the vendored path;
- `tle` asks crates.io for `=0.1.3`;
- without the patch the lockfile holds two `w3f-bls 0.1.3` sources (`SP/design-bench-nopatch/Cargo.lock:2377-2404` [V]),
  and E0277 follows (`SP/design-bench/target-nopatch/release/.fingerprint/bittensor-core-*/output-lib-bittensor_core`
  [V]).

So:

1. **rtao's own lock test** (`crates/rtao-core/tests/lock.rs`): require exactly one built `w3f-bls` in the actual
   all-feature/all-target normal/build graph, whose source is `git+…?tag=<core.tag>#<core.commit>`, identical to the source `bittensor-core` declares,
   and equal to `rtao_core::BITTENSOR_CORE_TAG`. While the core patch exists, the lock takes `bittensor-core` itself
   from `[core.patch]`, which `xtask pin --check` verifies, and the patch's second table keeps `w3f-bls` at
   `core_tag` (§1.11). The lock allows one pinned 0.1.3 and at most the recorded registry 0.1.9 with its exact checksum; activation or any unexpected identity fails (PLAN.md Q26).
2. **`rtao doctor`** (CLI, §10.4) runs the equivalent check on a downstream workspace's `Cargo.lock`, taking the
   expected reference from that workspace's own `bittensor-core` entry (the reference kind must match too). It prints
   the exact `[patch.crates-io]` line to add or fix, so the E0277 symptom becomes a one-line diagnosis.
3. **Template.** The `cargo generate` template `templates/subnet/` (§2.1) ships the patch line once, in its workspace
   root (§2.6), and never the core patch (§1.11).
4. **Upstream fix (PR-U4, Appendix D).** Change the core's manifest to `w3f-bls = { version = "=0.1.3", features =
   ["std"] }`. Inside the monorepo the root `[patch.crates-io]` (`UP/Cargo.toml:335` [V]) still redirects it to
   `vendor/w3f-bls`; downstream, `tle` and the core resolve one crates.io copy and no patch is needed [P] B §1.6 item 6.
   The core manifest's own comment explains why the vendored copy is used "so ciphertexts never drift from what
   pallet-drand can decrypt" (`UP/sdk/bittensor-core/Cargo.toml:51-53` [V]). So PR-U4 ships with evidence: commits
   produced by a crates.io-`w3f-bls` std build that rtao's patched build decrypts offline and pallet-drand decrypts on
   localnet (`ci/pr-u4-probe/`, §11.4 scenario 16). Until that proof exists and the PR merges, the patch stays. This
   is the open question carried by both B and D, and it is answered by a test, not assumed.

   crates.io `w3f-bls` 0.1.3 and `vendor/w3f-bls` differ in four files (`diff -r REG/w3f-bls-0.1.3/src
   UP/vendor/w3f-bls/src`): `double_pop.rs` is a no-std rewrite with the same `BLS_POP_` tag and seed order; `lib.rs`
   drops the serde `extern crate`; `verifiers.rs:271` changes ownership only; and `SecretKey::check` (`single.rs:384`)
   indexes `key[0], key[1]` instead of `key[1], key[2]`. Neither `tle` nor `CORE/timelock` touches `SecretKey`, so the
   two builds should produce byte-identical ciphertexts for the same ephemeral key and RNG seed, and each should
   decrypt the other's output. The probe confirms this (`vendor/w3f-bls/Cargo.toml:1-6` describes only one of the four
   changes).

### 1.4 Used as-is

Line numbers are definition sites in `CORE/` at `850f2755`. This tree is byte-identical to `v477` for `sdk/`
[J-MA §1].

| Area | Items | Host-gated? | rtao consumer |
|---|---|---|---|
| Metadata model | `runtime::Runtime::parse` (`runtime/mod.rs:163`; V14/V15 only, V16 rejected), `pallet` :230, `pallet_at` :234, `storage_entry` :238, `constant` :242, `type_id_of` :249, `resolve` :257, `type_ident` :272, `module_error` :340; pub fields `types: PortableRegistry`, `pallets`, `extrinsic`, `apis`, `metadata_bytes` | no | `rtao_runtime::RuntimeHandle` (cached by `rtao_client::RuntimeCache`), `rtao-runtime` verification, xtask codegen |
| Encode | `Runtime::compose_call` (`codec/encode.rs:532`; nested calls splice as `Value::Bytes`), `encode_id` :176, `encode_era_value` :445, `coerce_account_id` :491 (accepts ss58, 0x-hex or 32 raw bytes) | no | the single encoder for every call, storage key argument and runtime-API argument |
| Decode | `decode_id` (`codec/decode.rs:296`), `decode_call_value` :658, `batch::{decode_batch :55, decode_map_page :86, decode_map_changes :111}` (rayon above 64 items with `host`, serial without, same API: `codec/batch.rs:14-45` [V]), `value::Value` (`value.rs:12`), `to_corpus_json` :58 | no | dynamic reads, receipts, CLI/agent JSON, corpus tests |
| Storage keys | `storage::{hash_param :21, concat_hash_len :48, storage_prefix :60}`, `Runtime::storage_key` :69, `decode_storage_key_params` :101 | no | every storage key rtao builds (**always from the live entry**, §3.7) |
| Extrinsics | `extrinsic::TxParams` (:19-30), `signature_payload_parts` :217, `signature_payload` :240, `encode_signed_extrinsic` :260 (v4 only, `:265-273` [V]), `decode_extrinsic` :321, `era_birth` :407, `multisig_account_id` :419, `multisig_ss58` :440; fail-closed on unknown non-zero-sized extensions (:89-118) | no | the submission pipeline (§3.9), offline prepare and attach |
| Keys | `keys::Keypair` (`from_mnemonic` :211, `from_seed` :243, `from_uri` :264, `from_private_key` :287, `generate_mnemonic` :322, `sign` :391, `verify` :402), `public_key_from_ss58` :48, `ss58_from_public` :57, free `verify` :457 (tries raw, then `<Bytes>`-wrapped, `:457-469` [V]), `CRYPTO_ED25519/SR25519` :28-29, `DEFAULT_SS58_FORMAT` :31 | no (`from_encrypted_json`, `encrypt`, `decrypt`, `encrypt_for` are `host`, `keys/mod.rs:337-445` [V]) | `AccountId`, signers, btauth/1 |
| Keyfiles | format detectors (`keyfiles/mod.rs:29-47`), `encrypt_keyfile_data` :86, `get_password_from_environment` :111, `decrypt_keyfile_data` :153, `serialized_keypair_to_keyfile_data` :179, `deserialize_keypair_from_keyfile_data` :287 | **yes**, whole module (`lib.rs:15-16` [V]) | `rtao-wallet` (feature `encrypted`) |
| Timelock | `WeightsTlockPayload` (`timelock/mod.rs:53`), `UserData` :62, `reveal_round` :87, `inner_ciphertext` :97, `encrypt_and_compress` :104, `decrypt_and_decompress` :143, `encrypt_at_round` :230, `decrypt_with_signature` :319, `constants::*` (`GENESIS_TIME`, `DRAND_PERIOD`, `SECURITY_BLOCK_OFFSET = 3`, `COMMIT_INCLUSION_BLOCK_OFFSET`, `DRAND_ENDPOINTS`) | crypto no; drand fetches `:241-315` yes | weights commits, timelock UX, timelocked commitments, the round math of `rtao_primitives::drand` |
| MEV shield | `mlkem::seal(pk, plaintext, include_key_hash)` (`mlkem/mod.rs:53`), `twox_128` :37 | no | `rtao-client::shield` |
| RFC-0078 | `digest::ChainInfo` (`digest/mod.rs:34`), `metadata_digest` :102, `generate_extrinsic_proof` :117 | no | `CheckMetadataHash = Enabled`, Ledger |
| PoW | `pow::{seal :15, mine_cpu :51, PREFIX_LEN :11}`, `pow::opencl::{gpu_devices, GpuMiner}` | **yes** (`lib.rs:19-20` [V]) | `rtao-intents::pow` (owner/CLI builds) |
| Ledger | `signers::ledger::LedgerDevice::{open, app_version, address, sign}` | feature `ledger` (`signers/mod.rs:6-8` [V]) | `rtao-signer::LedgerSigner`, behind the `LedgerBackend` seam (§6.5); the transport is a private concrete HID type with no injection point (G16), so rtao's test lane takes PR-U17's seam from the core patch (§1.11) |
| Errors | `error::CoreError` | no | mapped once in `rtao-core` |

### 1.5 Wrapped (thin adapters; the bytes stay the core's)

| Core item | Wrapper | Why |
|---|---|---|
| `Runtime` | `rtao_runtime::RuntimeHandle(Arc<RuntimeEntry>)` (re-exported by `rtao-client`, cached by `rtao_client::RuntimeCache`): parsed V15, `MetadataLevel`, the surface report, the shape cache, name→index caches and the RFC-0078 digest computed once per spec (V16 is decoded only by `xtask`, §3.4) | caching per `(genesis, spec)`, V15 enforcement, surface verification (§3.4, §3.7) |
| `Keypair::sign` | `rtao_signer::KeypairSigner: Signer` | the async signer seam (§6.3); signing runs inline (about 0.03 ms, [R] A2 §7) |
| `LedgerDevice::sign` | `LedgerSigner` over the `LedgerBackend` seam: one device mutex held across whole operations, plus `spawn_blocking` (§6.5) | HID blocks until the user approves, and the app aborts or refuses interleaved APDUs |
| `decrypt_keyfile_data` | `Wallet::unlock_coldkey` (`spawn_blocking`, once per process, `Zeroizing`) | Argon2i with 512 MiB and t = 8 ([R] F §3.3) |
| `pow::mine_cpu` / `GpuMiner` | `PowSolver` (dedicated threads, `CancellationToken`, refreshes the work block before it ages past 5 blocks) | long-running CPU or GPU work |
| timelock crypto | `TimelockCommitter` (weights) and `rtao_tx::timelock::{seal, open}` (UX) with an async drand client; the pure round math (`current_round`, `round_at`, `reveal_time`) in `rtao_primitives::drand` over the core's `GENESIS_TIME` and `DRAND_PERIOD` | the injectable clock and pallet-faithful schedule (§1.6); async drand instead of blocking reqwest; commitments need `reveal_time` below `rtao-tx` (§8.2, §9.8) |
| `Value` | `rtao_core::DynValue` newtype: `Display` through `to_corpus_json`, typed accessors from the staged `value_ext` | keeps core types out of typed public APIs |

### 1.6 Gaps and divergences in the core, and the staged code that closes each

Each `rtao-core-ext` module starts with three machine-read lines: `//! UPSTREAM-TARGET: <path> (PR-Ux)`,
`//! UPSTREAM-PROBE: symbol <path>` or `//! UPSTREAM-PROBE: vectors <suite>`, and `//! UPSTREAM-FEATURES: <core
features>` (default none). A bare "the pinned core exports `<symbol>`" trigger is unsound: G4's names already exist
today with the buggy predicate (`CORE/timelock/epoch_schedule.rs:17, 45, 108`, ungated via `CORE/lib.rs:23`), and G9's
symbols exist behind `host`, which the core enables by default (`UP/sdk/bittensor-core/Cargo.toml:113`).

`cargo xtask staged --check` runs in `pr-gates`. It generates one scratch crate per probe under `target/staged-probe/`.
Each crate is its own workspace and depends on the pinned core with `default-features = false` plus exactly the listed
features, which is the no-host profile rtao ships. It carries rtao's `[patch.crates-io] w3f-bls` line and a copy of
rtao's `Cargo.lock`, because without the patch the core never compiles (E0277, §1.3). While the core patch exists
(§1.11), the copied lock names the fork's `bittensor-core`; a probe carries no core patch, so Cargo re-locks that one
package to `core_tag`, and probes run without `--locked`.

- A `symbol` probe names a path that does not exist at the current `core_tag`. A `vectors` probe runs the module's own
  test vectors against the core's implementation.
- Only three outcomes count as "not upstream yet": an unresolved-name error (E0405, E0412, E0425, E0432, E0433 or
  E0599), a missing core feature, or failing vectors. Any other failure (E0277, resolution or network errors) fails
  `staged --check` with the compiler output, so the gate cannot go silently inert.
- `staged --check` fails when a probe passes, which forces the staged copy's deletion.
- A probe that compiles only with `host`, for a module the no-host profile uses, is reported as "upstream but
  host-gated" and does not trigger deletion.

Probes: G1-G3 `symbol` at the ungated Appendix D targets; G4 `vectors` (the skip and tempo-reset safety-net vectors run
against `bittensor_core::timelock::epoch_schedule::{should_run_epoch, predict_first_reveal_block}`, which already exist
with the `MAX_TEMPO_U64` predicate); G5 `symbol timelock::generate_commit_v2_at`; G8 `symbol
runtime::Runtime::metadata_ir`; G9 `symbol keyfiles::deserialize_keypair_from_keyfile_data` with
`UPSTREAM-FEATURES: keyfiles-json` (today it exists only under `host`); G10 `symbol
keyfiles::encode_password_for_environment` (features `host`); G11 `symbol keys::verify_strict`; G12 `symbol` at the path
PR-U15 introduces, recorded when it is filed (today the helpers are free functions in `client.rs:1515-1570`).

G16 has no staged module. Staged code adds functions that rtao can host in `rtao-core-ext`; PR-U17 changes a private
type of the core (`LedgerDevice` holds the concrete HID `Transport`), which only a patched core can expose, and because
Cargo ignores `[patch]` in dependencies, a patch suits only rtao's own test lane, never code that subnets run. rtao
therefore carries PR-U17 as a core patch, whose obsolescence probe `xtask pin --check` runs (§1.11).

| # | Core gap or divergence (evidence) | `rtao-core-ext` module | PR |
|---|---|---|---|
| G1 | Protocol logic is private inside the blocking client: `fetch_metadata` (`client.rs:1234`), `decode_storage` :1352, runtime-API encoding (:469-499, :1334), `find_extrinsic` :942, `decode_outcome` :993, `dispatch_error` :1365 [P] D §1.4 | `ops::{metadata, storage, runtime_api, extrinsic_lookup, dispatch_error}` (sans-IO) | PR-U1 |
| G2 | Fee estimation uses the deprecated `payment_queryInfo` (`client.rs:551-576`). Python probes `TransactionPaymentApi_query_info` via `state_call` with a zero signature and an immortal era (`PY/_transport/interface.py:714-731`) [J-CF §3.1] | `ops::fee` (probe-extrinsic builder and `RuntimeDispatchInfo` decode) | PR-U1 |
| G3 | Receipts check only 3 nested wrappers (`client.rs:1018-1048` [V]) and look up `Sudo.Sudid`'s outcome under a field named `result`, so a failed `Sudid` reports success (`client.rs:1025-1037`, §3.9.4); Python checks 6 (`PY/_transport/utils/receipt.py:8-16` [V]) plus MevShield failure events and the `Treasury`/`Balances.Deposit` fee fallback (`receipt.py:75-96`) | `ops::receipt` (Python semantics) | PR-U2 |
| G4 | Epoch predicate `blocks_since_last_step > MAX_TEMPO_U64` (`timelock/epoch_schedule.rs:54` [V]) vs the pallet's `BlocksSinceLastStep > tempo` (`pallets/subtensor/src/coinbase/run_coinbase.rs:1232` [V]); the pallet also caps the counter at `tempo + 1` (`run_coinbase.rs:413-415` [V]) | `epoch_schedule` (pallet-faithful copy, upstream vectors plus safety-net vectors) | PR-U5 |
| G5 | `generate_commit_v2` reads the wall clock internally (`timelock/mod.rs:160-195`) | `timelock_commit::{reveal_round_at, commit_payload}` with `now: f64` as a parameter | PR-U5 |
| G6 | The blocking client signs shield carrier and inner extrinsic with era 64 (`client.rs:32, 622-654`); the runtime rejects `submit_encrypted` with era > 8 as `Stale` (`UP/runtime/src/check_mortality.rs:19, 94-100` [V]) | none: rtao never calls `Client::submit_shielded`. The runtime bound is extracted into `rtao_catalog::rt::MAX_SHIELD_ERA_PERIOD` (§4.9 layer 5), and `ShieldPeriod` (§3.11) is tested against it | PR-U7 (fixes the core's own users; retires nothing in rtao) |
| G7 | `Policy::check` takes only `IntentCall`, whose trusted constructors are private (`transaction.rs:55-90, 103-248, 929-976`) | none: rtao ports `PY/intents/plan.py:21-85` and differential-tests it against `Policy::check` on 21 of the 22 constructors (`set_root_claim_type` is retired; §4.6) | PR-U8 |
| G8 | Metadata IR exists only in the PyO3 binding (`UP/sdk/bittensor-core-py/src/runtime.rs:554,579` [J-CF §7 #23]) | `ir::MetadataIr` (the same IR plus storage hashers and key idents, event field idents, runtime-API signatures and versions) | PR-U9 |
| G9 | The plaintext keyfile JSON codec is gated behind `host` with libsodium, OpenSSL, reqwest and rayon, although it uses only non-host `Keypair` constructors and `serde_json` (`keyfiles/mod.rs:179-370` [V]) | `keyfile_json::{deserialize_keypair_from_keyfile_data, serialized_keypair_to_keyfile_data, is_encrypted}`, a line-for-line copy | PR-U6 |
| G10 | `save_password_to_environment` calls `std::env::set_var` (`keyfiles/mod.rs:126-138`), which is unsound with threads and `unsafe` in edition 2024 | `keyfile_env::encode_password_for_environment(name, pw) -> String` (pure) | PR-U10 |
| G11 | `keys::verify` always tries the `<Bytes>` wrapper; the strict variant is private (`verify_with_crypto`, `keys/mod.rs:78-100`) | `verify_strict` (schnorrkel `b"substrate"` context and ed25519, already in the core's graph) | PR-U10 |
| G12 | `Value` navigation helpers (`field`, `variant`, `as_u128`, …) live in host-gated `client.rs:1515-1570` [J-PD §3 A#6] | `value_ext::ValueExt` | PR-U15 |
| G13 | `Runtime::parse` rejects V16 (`runtime/mod.rs:193-198` [V]) and yields `apis = []` for V14 (`:172-181` [V]) | none: rtao requires V15 at run time and uses `frame-metadata` 23 for V16 in tooling (§3.4) | none (V15 is what RFC-0078 digests use) |
| G14 | The core's extension guard fails closed only at signing time (`codec/extrinsic.rs:89-118`) | none: rtao calls `signature_payload_parts` once at connect with dummy params and disables signing with a precise error (§3.4) | none |
| G15 | 83 intents and 102 reads vs 22 constructors and about 9 read helpers; no plan, preflight or result model; out of scope by upstream's spec (`UP/sdk/bittensor-core-spec.md:85-97`) | none: `rtao-tx`, `rtao-intents`, `rtao-reads` port Python | none |
| G16 | The Ledger APDU transport has no injection point: `mod hid` is private, `LedgerDevice { transport: Transport }` has only `open()`, which takes the first HID Ledger, and APDU building is private (`CORE/signers/mod.rs:6-9`, `signers/ledger.rs:51-65, 158-219`, `signers/hid.rs:47-72` [V]). Speculos, Ledger's emulator, has no USB HID, so the core's Ledger path cannot be tested against it (`RES/J-ledger-ci-shim.md` §3) | none in shipped code: until PR-U17 ships in `core_tag`, rtao's own workspace builds the core through a Cargo `[patch]` that adds it, and only `rtao-testkit::ledger_emu` names its items (§1.11, §11.9) | PR-U17 |

The staged epoch predictor also distinguishes the processing phases within a
block. Reveal processing reads the pre-coinbase counter, while a commit
extrinsic reads the post-coinbase epoch. A counter-threshold fire can occur
only after coinbase increments the counter, so prediction derives the commit
epoch from the simulated post-coinbase state. The pinned regression at head
105 (last epoch 90, tempo 20, step counter 20) predicts reveal block 126 for
inclusion 106 and a one-epoch reveal period. This is part of D-006 and PR-U5;
all existing upstream schedule vectors remain passing. Future inconsistent
inputs and capacity deferrals remain outside the predictor's model.

### 1.7 `host` and the no-host neuron profile

Every proposal enables `bittensor-core/host` for neuron binaries. That means libsodium, vendored OpenSSL (which needs
Perl `IPC::Cmd` and `make`) and `secp256k1-sys` (through `sp-core/std`), 330 crates instead of 207, and painful Docker,
musl and cross builds [J-PD §2.2, concern 1]. hidapi is gated by the separate `ledger` feature, not by `host`, and uses
the pure-Rust hidraw backend on Linux (`UP/sdk/bittensor-core/Cargo.toml:86-94, 126`). A validator or miner needs
only:

- the hotkey (a plaintext keyfile in the btcli layout);
- signing, codec, runtime, extrinsic assembly and timelock encryption.

None of these is host-gated (`lib.rs:10-25`, `keys/mod.rs` [V]). The only blocker is the plaintext keyfile codec, so
this design adds a **no-host profile**:

- `rtao-core` features: `host` (off by default), which forwards to `bittensor-core/host`; `ledger`.
- `rtao-wallet` reads plaintext hotkeys through `rtao_core::keyfile_json` (staged G9) without `host`. Encrypted coldkeys
  need feature `encrypted`, which turns on `rtao-core/host`.
- The facade features `validator` and `miner` do not enable `host`. `owner`, `wallet-encrypted`, `pow` and `full`
  (which the CLI uses) do (§2.4).
- **Differential test**: `crates/rtao-core/tests/keyfile_json_diff.rs` (dev-dependency on `bittensor-core` with
  `host`) runs the staged reader and `bittensor_core::keyfiles::deserialize_keypair_from_keyfile_data` over every
  keyfile vector: upstream `PYT/unit/test_keyfiles_golden.py` inputs, btcli-created files from the interop lane, legacy
  `" ss58Address"` files, numeric-string `cryptoType`, bare mnemonics and seeds, and polkadot-js exports for the error
  path. It asserts identical keypairs or identical error classes.
- **CI enforcement**: the `no-host` job builds `-p rtao --no-default-features --features validator,prometheus` and,
  separately, `--features miner,redis`. `xtask deps-check --profile no-host` asserts with `cargo tree -e normal,build`
  that none of `sodiumoxide`, `libsodium-sys`, `openssl`, `openssl-sys`, `openssl-src`, `native-tls`, `fernet`,
  `ansible-vault`, `secp256k1-sys`, `aws-lc-rs`, `aws-lc-sys`, `cmake`, `hidapi`, `libloading` or `rayon` is present.
  Build edges are included because `cargo tree -e normal` hides C builders such as `cmake` and `openssl-src`. It also
  asserts that `reqwest` lacks the `blocking` feature (the core's blocking copy) and that `sp-core` is built without
  `std` (`cargo tree -e features -i sp-core`). The `aws-lc-*` entries rely on the ring-only TLS decision (§2.7).
- **Native code that remains**: `ring` (TLS) and `blake3`'s SIMD kernels (through `merkleized-metadata`,
  `REG/blake3-1.8.7/build.rs:343-360`), both built by the C compiler rustc already uses as its linker. Neuron images
  need no Perl, OpenSSL, libsodium or CMake; they do need the compiler that ships with every Rust build image.
- **Feature unification caveat**: Cargo unifies features across the packages selected for a build. A subnet workspace
  that also contains an owner tool must build its neuron binaries with `cargo build -p <validator>` to keep `host` out
  [I]. The template's Dockerfile does exactly that.
- **Upstream**: PR-U6 splits `host` into `keyfiles-json` (pure), `keyfiles-crypto` (NaCl, Fernet, Ansible),
  `drand-http`, `rpc-client`, `parallel` and `pow`. The staged reader is deleted when it lands.

### 1.8 Deliberately not used on runtime paths

| Item | Why | Instead |
|---|---|---|
| `client::Client` (blocking reqwest HTTP; `ws://` rewritten to `http://`; polling receipts; no subscriptions, nonce cache or pools; `client.rs:1-8,150-179,338-355,897-991`) | blocking I/O on tokio workers; Python-divergent era, anchor, fee and receipts | `rtao-rpc` + `rtao-client`; the core client runs only in the `core-oracle` test lane |
| `Client::submit_shielded` | era 64 (rejected as `Stale`), no inner-receipt resolution | `rtao-client::shield` (§3.11) |
| `transaction::{Executor, IntentCall, Plan, Policy}` | `Executor<'a>` borrows `&Client` (`transaction.rs:997-1000`); 22 constructors; private semantics | `rtao-tx` (Python port), with a `Policy` equivalence test |
| `timelock::generate_commit_v2` | wall clock inside; core epoch predicate | `TimelockCommitter` over staged `timelock_commit` |
| `timelock::{decrypt, get_round_info, get_reveal_round_signature}` | blocking reqwest drand fetch | async `DrandClient` + `decrypt_with_signature` |
| `timelock::epoch_schedule::predict_first_reveal_block` | divergent predicate | staged `epoch_schedule` |
| `keyfiles::save_password_to_environment` | `set_var` | staged `keyfile_env` (prints `export …`) |
| a V14 `Runtime` for signing or modern runtime APIs | `apis = []` | V15 required (§3.4) |

Enforced by `clippy.toml` (from C §1.7, extended per [J-MA G8]):

```toml
# clippy.toml (workspace) — `cargo clippy -- -D warnings` turns these into CI failures.
# allow-invalid = true marks the seven paths that resolve only when the core is built with `host` (see below), and the
# two PR-U17 items, which resolve only in the patched core (§1.11; added in M7-09).
disallowed-methods = [
  { path = "bittensor_core::client::Client::connect", reason = "blocking reqwest client; use rtao_client::Client", allow-invalid = true },
  { path = "bittensor_core::transaction::Executor::new", reason = "bound to the blocking Client; use rtao_tx::Executor", allow-invalid = true },
  { path = "bittensor_core::transaction::Executor::with_policy", reason = "bound to the blocking Client", allow-invalid = true },
  { path = "bittensor_core::timelock::generate_commit_v2", reason = "reads the wall clock and uses the core epoch predicate; use rtao_intents::weights::TimelockCommitter" },
  { path = "bittensor_core::timelock::decrypt", reason = "blocking drand fetch; use rtao_tx::timelock::open", allow-invalid = true },
  { path = "bittensor_core::timelock::get_round_info", reason = "blocking drand fetch", allow-invalid = true },
  { path = "bittensor_core::timelock::get_reveal_round_signature", reason = "blocking drand fetch", allow-invalid = true },
  { path = "bittensor_core::timelock::epoch_schedule::predict_first_reveal_block", reason = "safety-net predicate differs from the pallet; use rtao_core::epoch" },
  { path = "bittensor_core::timelock::epoch_schedule::should_run_epoch", reason = "use rtao_core::epoch" },
  { path = "bittensor_core::keyfiles::save_password_to_environment", reason = "std::env::set_var is not thread-safe", allow-invalid = true },
  { path = "bittensor_core::signers::ledger::LedgerDevice::with_transport", reason = "PR-U17 through the core patch: rtao-testkit::ledger_emu only", allow-invalid = true },
]
disallowed-types = [
  { path = "bittensor_core::signers::ApduTransport", reason = "PR-U17 through the core patch: rtao-testkit::ledger_emu only", allow-invalid = true },
]
```

The `core-oracle` tests in `rtao-testkit` opt out per item with `#[allow(clippy::disallowed_methods)]`, and
`rtao-testkit::ledger_emu` opts out of the two PR-U17 entries, the only module allowed to name PR-U17's items (§1.11).
`xtask pin --check` keeps those two paths equal to `[core.patch] probe`, so they follow PR-U17 as it merges. Checked on
clippy 1.89 against a prototype of the patched core (`RES/J-ledger-ci-shim.md` §8.5): the two entries flag `use`
imports, `impl` and `dyn` type positions, generic bounds, `where` clauses and `with_transport` calls, whether the path
is imported or written in full, and a module-level `allow` exempts `ledger_emu`; a trait `impl` header written by full
path is not flagged, which `core-unpatched` catches (§1.11). Against the unpatched core, `allow-invalid` keeps them
silent.

Seven of the ten paths are host-gated and resolve only when the core is built with `host`: `client::Client::connect`,
`transaction::Executor::{new, with_policy}`, `timelock::{decrypt, get_round_info, get_reveal_round_signature}` and
`keyfiles::save_password_to_environment` (`CORE/lib.rs:10-29`; `CORE/timelock/mod.rs:241, 275, 297`). Measured on clippy
1.89 and 1.99, a configured path that does not resolve produces a non-lint warning ("does not refer to a reachable
function") that `-D warnings` does not turn into an error, so an upstream rename would silently disable its ban. Hence:

- the seven host-gated entries carry `allow-invalid = true` (accepted by clippy 1.89 and 1.99), which silences the
  warning in no-host runs and still enforces the entry wherever its path exists;
- the enforcing run is `cargo clippy --workspace --all-targets --all-features -- -D warnings`, where all ten paths
  resolve; the `no-host` job adds a no-host clippy run (Appendix E); clippy runs only on the development toolchain (the
  MSRV job runs `cargo check`);
- `xtask core-surface` emits, for each of the ten paths, `#[allow(clippy::disallowed_methods)] const _: <signature> =
  <path>;` into `crates/rtao-core/tests/core_surface.rs` (the host-gated ones under `#[cfg(feature = "host")]`), so a
  renamed or removed path fails the build instead of disabling its ban (§1.9).

### 1.9 Keeping up with an unversioned core

`bittensor-core` is `publish = false` at version `0.1.0` and never stamped. Its API is shaped by the Python binding
([R] B §7). Three mechanisms catch API changes:

1. **`core_surface` probe** (`crates/rtao-core/tests/core_surface.rs`, generated by `xtask core-surface`). It is a
   compile test that names every core item `rtao-core` and `rtao-core-ext` use, with its exact signature, for example:

   ```rust
   const _: fn(&Runtime, &str, &str, &Value) -> Result<Vec<u8>, CoreError> = Runtime::compose_call;
   ```

   A bump that renames or reshapes an item fails here with the symbol's name. The probe also names the ten
   `disallowed-methods` paths of §1.8, so a renamed banned path fails here instead of silently disabling its ban.
2. **Optional hosted core-source check.** For a relevant dependency change, compile the client against a reviewed
   upstream `main` commit with a bounded job. This does not start a node or install a scheduled canary campaign.
3. **Golden replay through the wrappers** in `pr-gates` (§11.1).

### 1.10 Where the dynamic `Value` stays

- **Encoding input.** Every typed call, storage key and runtime-API argument becomes a core `Value` (trait
  `IntoCoreValue`, §5.1) and goes through `compose_call`, `storage_key` or `encode_id`. The golden vectors pin a single
  encoder. A typed `Value` costs one small allocation per argument; the core spec measures `compose_call` under 50 µs
  ([R] A2 §7).
- **Python-shaped output.** Dynamic reads (`storage_dyn`, `call_dyn`), event records consumed by the receipt logic,
  `rtao call`, agent tools and reads that fall back after a surface mismatch return `Value`, whose shapes are the
  shape corpus contract (`CORE/codec/value.rs:1-4` [V]).
- **Not used for typed models.** `Value` renders `AccountId32` as an ss58 `String` and byte vectors as utf8-or-hex, which
  is ambiguous. Typed models decode from raw bytes with `DecodeAsType` against the same `PortableRegistry` (§8.1).

### 1.11 The PR-U17 core patch

Decided by the user (§15.2 Q20): until a `core_tag` contains PR-U17 (`ApduTransport` and
`LedgerDevice::with_transport`, Appendix D), the rtao workspace builds `bittensor-core` through a Cargo `[patch]` that
adds exactly PR-U17 on top of `core_tag`. It serves one consumer, the Ledger emulator lane (§11.9), and replaces the
test-only transcription of `CORE/signers/ledger.rs` that plan v2 had put in `rtao-testkit`; the transcription survives
only as a fallback. Why a patch and not a staged module: §1.6. The mechanics below were prototyped in Docker while
planning, with a local bare repository standing in for the fork (`RES/J-ledger-ci-shim.md` §8): PR-U17 itself, the
root manifest, the git and lock checks, the obsolescence probe, the clippy entries, the unpatched build, a cherry-pick
rebase, and an emulator run of the core's own `ledger.rs` through `with_transport`.

- **Branches.** PR-U17 is written and filed in M0 (its evidence predates the plan) from the user's fork of the monorepo
  (default `github.com/sam0x17/subtensor`, created when filing); its review branch targets monorepo `main`. For rtao,
  the patch branch `rtao-core-patch/<core_tag>` in the same fork is `core_tag` plus the PR-U17 commits, cherry-picked,
  and new commits refresh it when the PR changes in review. Patch branches are never force-pushed or deleted while an
  rtao commit pins them: once a deleted branch's commits are pruned, a fresh resolve of the pinned `rev` fails
  (`revspec '<rev>' not found`). Each new `core_tag` gets a new branch.
- **Root manifest only**, two tables (the last two of the root `Cargo.toml`, §1.2):

  ```toml
  # PR-U17 until core_tag contains it ([core.patch] in upstream.toml; checked by `cargo xtask pin --check`)
  [patch."https://github.com/RaoFoundation/subtensor.git"]
  bittensor-core = { git = "https://github.com/sam0x17/subtensor.git", rev = "<full commit>" }

  # The patched core's other path dependencies, kept at core_tag (one w3f-bls, one subtensor-macros)
  [patch."https://github.com/sam0x17/subtensor.git"]
  w3f-bls = { git = "https://github.com/RaoFoundation/subtensor.git", tag = "v477" }
  subtensor-macros = { git = "https://github.com/RaoFoundation/subtensor.git", tag = "v477" }
  ```

  The `[patch.crates-io] w3f-bls` line (§1.3) stays as it is, but the first table alone does not build. A path
  dependency inside a git package takes that package's source, so the patched core brings the fork's `vendor/w3f-bls`
  and `support/macros`: the lock then holds a second `w3f-bls 0.1.3` next to `tle`'s and the core fails with E0277,
  the failure of §1.3, and `rtao-core-ext`'s `subtensor-macros` (§1.1) gets a second copy. The second table sends every
  other package of the core's path-dependency closure (§1.2) back to `core_tag`, so the lock keeps one `w3f-bls` and
  one `subtensor-macros`, both at the tag, and `bittensor-core` is the only package from the fork, locked as
  `git+https://github.com/sam0x17/subtensor.git?rev=<full commit>#<full commit>`. Cargo prints no warning for either
  table. Features stay on the `[workspace.dependencies]` entry: Cargo ignores `features` on a `[patch]` entry and
  warns. A `[patch]` table matches its URL whatever the reference (checked with `tag` and `rev`), so a build of
  monorepo `main` must drop both tables (`core-canary`, below).
- **`[core.patch]`** in `upstream.toml` (§1.2): `repo`, `branch`, `rev`, `base` (equal to `core.commit`),
  `upstream_pr` (the PR's link), `paths` (the files PR-U17 may touch: `signers/{mod.rs, ledger.rs, hid.rs}`, the three
  files the prototype's diff touched) and `probe`, the two items whose presence in the unpatched core proves that
  PR-U17 shipped (`bittensor_core::signers::ApduTransport` and
  `bittensor_core::signers::ledger::LedgerDevice::with_transport`).
- **`cargo xtask pin --check`** (in `pr-gates`) verifies, while `[core.patch]` exists:
  1. `base` equals `core.commit`, and `rev` descends from `base` (`git merge-base --is-ancestor`; a shallow fetch of
     `branch` suffices);
  2. `git diff --name-only base rev` lists only `paths`;
  3. the lockfile's `bittensor-core` source is `repo` at `rev`, and no other package comes from `repo` (a new path
     dependency of the core that the second table misses fails here);
  4. the two `clippy.toml` entries of §1.8 name the `probe` items;
  5. the patch is still needed. A generated probe crate outside the rtao workspace (its own `[workspace]` table and
     `w3f-bls` line, like the probes of §1.6) names both `probe` items in one generic function,
     `fn probe<T: ApduTransport + 'static>(t: T) -> LedgerDevice { LedgerDevice::with_transport(t) }`, so it needs no
     transport type. It must compile against `repo` at `rev`, carrying the second table above (without it `w3f-bls`
     splits as with the first table alone), and fail against the unpatched `core_tag` with an unresolved-name error
     (E0405, E0412, E0425, E0432, E0433 or E0599) in its own source, read with `--message-format=json`; any other
     failure (a network error, an E0277 from a `w3f-bls` mistake) fails the check with the compiler output, as in §1.6.
     It builds in its own target directory (`target/core-patch-probe/`): probe packages of the same name in a shared one
     reuse each other's fingerprints, which gave a false "compiles" in the prototype. Once the probe compiles against
     `core_tag`, or the PR's merge commit is an ancestor of `core.commit`, the check fails with "PR-U17 shipped: drop
     the core patch".
- **`cargo xtask core-patch`.** `rebase <tag>` cherry-picks the PR-U17 commits (`base..rev`; on the first run, in M7-09,
  the commits of PR-U17's review branch) onto the tag, pushes `rtao-core-patch/<tag>` to the fork (not with `--no-push`,
  which a `pin-bump` dry run uses), and rewrites `[core.patch]` and both tables, the second from the core's
  path-dependency closure at the new tag; a conflict stops it with `core-patch-conflict`, which the agent resolves like
  any upstream change and logs. A pin bump that moves `core_tag` (§12.3) runs it while `[core.patch]` exists and the new
  tag lacks PR-U17; when the new tag contains PR-U17, the bump deletes both tables and `[core.patch]`, adapts
  `rtao-testkit` to the API as merged, and logs it. `strip` deletes both tables in a disposable CI checkout
  (`core-unpatched`); `xtask pin --core-branch` drops them too, so `core-canary` builds monorepo `main` itself.
- **Downstream.** Cargo applies `[patch]` only from the root manifest of the workspace being built, and ignores a
  dependency's tables without a warning, so subnet projects never see the core patch and keep building the upstream
  tag: their only patch line stays `w3f-bls`, and the `cargo generate` template must not copy the core patch. Shipped
  crates must therefore compile against the unpatched core, and only `rtao-testkit` (feature `ledger-emu`) may name
  PR-U17's items: in a downstream build, a crate that names them fails with E0432. Three checks enforce it:
  - the `clippy.toml` entries for `LedgerDevice::with_transport` (`disallowed-methods`) and `ApduTransport`
    (`disallowed-types`), both `allow-invalid = true`, which only `rtao-testkit::ledger_emu` opts out of (§1.8); they
    miss only a trait `impl` header written by full path;
  - the `core-unpatched` job (on relevant source/pin changes;
    Appendix E), which deletes both tables in its disposable checkout and runs
    `cargo check --workspace --exclude rtao-testkit --exclude xtask --all-features` without `--locked` (Cargo then
    re-locks only `bittensor-core`), proving what subnets build. `--all-features` still builds an excluded crate that
    a feature of an included one reaches, so the job also fails if a shipped feature pulls in `rtao-testkit` (§2.3
    rule 8);
  - the `template` job (§11.4, M3), which stays the end-to-end downstream proof.
- **Coverage.** From its first run the emulator lane drives the core's own `ledger.rs` (APDU building, 250-byte
  chunking, status-word mapping) against the real app, through `LedgerDevice::with_transport` over `rtao-testkit`'s
  Speculos TCP transport (Speculos's length framing); `ledger_replay` in `pr-gates` uses a replay transport the same
  way (§11.9). The prototype did both: the address, a confirmed address, the core's proof vector (16 APDUs, the golden
  signature) and a rejection, then a replay of the 35 recorded exchanges. Deferred hardware coverage includes: the HID transport and OS device access (`hid.rs`, `LedgerDevice::open`), the app binary users install, real
  firmware and the review on a real screen. The rtao workspace, and with it the device-run build and the release
  binaries, uses the patched core, whose `open()` path differs from upstream only by the seam (it boxes the HID
  transport); `[core.patch] paths` bounds the difference (§15.1 R21).
- **Cost.** While the patch exists, rtao's own builds, never a subnet's, also clone the fork's git history, a second
  database the size of the monorepo's (§2.8).
- **Fallbacks.** If the PR changes in review, the patch branch follows it. If upstream declines PR-U17, the agent stops
  and asks the user (PLAN.md "Executing this plan", rule 6): keep carrying the patch, fall back to the test-only
  transcription (`RES/J-ledger-ci-shim.md` §3, option D), or build a uhid bridge, which otherwise stays a non-goal
  (§17 #24).

---

## 2. Workspace and crate layout

### 2.1 Repository tree

```
rtao/
├─ Cargo.toml              # [workspace] (resolver = "3"; exclude = ["ci/pr-u4-probe"]); [workspace.dependencies] incl. bittensor-core (core_tag) and rustls (ring only, §2.7); [patch.crates-io] w3f-bls; from M7-09 until PR-U17 ships, the core patch (§1.11)
├─ upstream.toml           # the single pin file: [core], [parity], [localnet], [networks.*], [artifacts], [core.patch] while it exists (§1.2)
├─ rust-toolchain.toml     # dev/CI toolchain (current stable; Renovate-bumped after the goal loop, §12.4); MSRV is checked separately (§2.5)
├─ clippy.toml             # disallowed-methods and disallowed-types (§1.8)
├─ deny.toml               # licenses, advisories, SCALE-stack duplicate bans (§12.4); bans aws-lc-rs, aws-lc-sys, native-tls (§2.7)
├─ .config/nextest.toml    # profiles: ci (excludes the e2e, interop, core_oracle and ledger_emu test targets), e2e-fast, e2e-slow, interop, core-oracle, ledger-emu (§11.1)
├─ release-plz.toml        # deferred distribution/upkeep configuration; disabled for current development (Q30)
├─ crates/
│  ├─ rtao-core-ext/       # staged upstream code (no tokio); every module has an UPSTREAM-TARGET header
│  ├─ rtao-core/           # the only importer of bittensor_core; re-exports; offload; core_surface probe; lock test
│  ├─ rtao-primitives/     # ids, money, fixed point, ratios, AccountId, DefaultAccount, Python formatting (pyfmt), drand round math, axon targets
│  ├─ rtao-catalog/        # GENERATED data: intent/read/error specs, ErrorCode, extracted Python + runtime tables
│  ├─ rtao-runtime/        # GENERATED chain descriptors + typed call builders; hand models; shape hashes; verify_surface; RuntimeHandle, CallBytes (no I/O)
│  ├─ rtao-rpc/            # tokio-tungstenite JSON-RPC session implementing subxt_rpcs::client::RpcClientT
│  ├─ rtao-signer/         # Signer trait, roles, KeySource<R>, Keypair/Ledger/Vault/Extension signers (Remote in M10); extension bridge server and client; Vault page server and QR
│  ├─ rtao-wallet/         # btcli wallet layout, keyfiles (plaintext always; encrypted with feature), passwords
│  ├─ rtao-client/         # Client, Snapshot, View, RuntimeCache, ReadPlan, HeadTracker, submission, shield, compose, waits, config (btcli files), ClientError
│  ├─ rtao-tx/             # Intent traits, Executor, Plan, Policy, ExtrinsicResult, ChainError/TxError, classify, multisig account object, timelock
│  ├─ rtao-intents/        # the 83 intents (generated interfaces + pass-through bodies + ported bodies), AnyIntent
│  ├─ rtao-reads/          # the 102 reads (generated specs/dispatch/namespaces + ported bodies), read models, commitments, ReadError
│  ├─ rtao-metagraph/      # Metagraph (v11 projection), MetagraphView (borrowed, shape-gated), SubnetsMetagraphExt, MetagraphSync, persistence
│  ├─ rtao-btauth/         # btauth/1 core (no I/O) + features: server (axum/tower), client (reqwest), redis, legacy-v10
│  ├─ rtao-neuron/         # NeuronRuntime, Validator/Miner, WeightScheduler, AxonPublisher, OwnerOps, telemetry
│  ├─ rtao-evm/            # v11 PY/evm parity: keystores, precompile ABIs, eth JSON-RPC, association proofs
│  ├─ rtao-cli/            # binary `rtao` (btcli-equivalent); its [[bin]] sets doc = false (§12.8: it would overwrite the facade's docs)
│  ├─ rtao-testkit/        # FakeNode (socket), FakeChain (RpcClientT), RecordingView, Localnet, Scenario, fixtures, oracle loaders, the Ledger emulator harness (§11.9)
│  ├─ rtao/                # facade: re-exports + role profiles (the one dependency subnet code needs); rtao::blocking
│  └─ (post-parity, optional) rtao-compat-synapse/, rtao-compat-epistula/, rtao-transport-quic/
├─ xtask/                  # pin, import-upstream, extract, regen, parity, staged, core-surface, deps-check, drift, oracle,
│                          # record, docs, compat, localnet-image, localnet (bootstrap, run, smoke, up/down), probe, release-notes, pr-u4-proof,
│                          # fuzz, bench (optional hosted), template (check), upstream (parity-tag), ci (issue), lint-upstream,
│                          # ledger-emu (build-app, up, record, speculos-image), gates (hosted pr-gates, Q30), core-patch (rebase, strip)
├─ codegen/                # types.toml, overrides.toml, models.toml, scope.toml, intent_types.toml, intent_callmap.toml
├─ vendor/
│  ├─ upstream/            # artifacts copied from parity_tag (sha256 in upstream.toml); src/<path> + tree.txt (extraction inputs, §1.2); extracted/*.json (Rust extractor output)
│  ├─ metadata/<net>/      # <spec>.v15.scale, <spec>.v16.scale, index.toml (live networks; localnet-main/ holds only the :main drift baseline, never a codegen input)
│  ├─ oracle/              # committed Python-oracle outputs at parity_tag (intents.json, reads.json, pure/*.json, cli_tree.json, seeds/, samples_extra.json, lock.json)
│  ├─ recorded/<spec>/     # spec-current goldens/corpus recorded by upstream's recorders on :v<spec> localnet
│  ├─ recorded/ledger/     # Ledger APDU transcripts recorded on Speculos, replayed by ledger_replay (§11.9)
│  └─ bench/               # committed finney get_metagraph payloads (netuids 1, 3, 64) + synthetic 2,500-UID payload
├─ parity/                 # map.toml, pending.toml, divergences.toml, raw_only_extra.toml, e2e_map.toml, env.toml, read_hits_extra.toml, exports_extra.toml, matrix.json (generated)
├─ ci/                     # oracle Dockerfile and record.py; interop compose, pyproject.toml + uv.lock, generated recipe scripts; pr-u4-probe/ (own workspace); build image; ledger/pins.toml (§11.9)
├─ book/                   # mdBook; reference/ is generated
└─ templates/subnet/       # cargo-generate template: validator, miner, owner tool, Dockerfile, compose, CI
```

### 2.2 Crates

| Crate | Purpose | Main public items | Features | Depends on (rtao crates) |
|---|---|---|---|---|
| `rtao-core-ext` | staged upstream code (§1.6) | `ops::{metadata, storage, runtime_api, extrinsic_lookup, receipt, dispatch_error, fee}`, `ir::MetadataIr`, `epoch_schedule`, `timelock_commit`, `keyfile_json`, `keyfile_env`, `verify_strict`, `value_ext::ValueExt` | none | (`bittensor-core` only; third-party list in §1.1) |
| `rtao-core` | seam (§1.1) | curated re-exports (`keys`, `codec`, `runtime`, `extrinsic`, `digest`, `mlkem`, `timelock`, `epoch`, `keyfile_json`; `keyfiles`/`pow` under `host`; `ledger` under `ledger`), `CoreError` mapping, `DynValue`, `offload`, `BITTENSOR_CORE_TAG`, `BITTENSOR_CORE_COMMIT` | `host`, `ledger`, `offload` (default) | `rtao-core-ext` |
| `rtao-primitives` | vocabulary (§5) | `AccountId`, `DefaultAccount`, `H256`, `NetUid`, `MechId`, `Uid`, `NetUidStorageIndex`, `BlockNumber`, `TaoBalance`, `AlphaBalance`, `Balance<Tao>`, `Balance<Alpha>`, `DynBalance`, `Money`, `Amount`, `ResolvedAmount`, `Spend`, `PriceRao`, `StakeWeight`, `PerU16`, `Permill`, `Perbill`, `Percent`, `PerMillion`, `U64Normalized`, `U64F64`, `U96F32`, `I96F32`, `I64F64`, `I32F32`, `Ratio`, `CryptoType`, `IntoCoreValue`, `pyfmt`, `ratio::{ratio_fraction, RATIO_TYPE_DENOMINATORS}`, `hyperparams::{Hyperparam, HyperparamTable, proportion_to_raw}` (§5.5), `drand::{current_round, round_at, reveal_time, parse_duration, format_duration}` (§9.8), `axon::{AxonTarget, axon_base_url}` (§7.3), `evm::{h160_to_ss58, …}`, `json::{from_slice, from_str}` (`Value`-first parsing, §2.7 JSON row) | `serde` (default), `schemars` | `rtao-core` |
| `rtao-catalog` | GENERATED, data only (§4.1) | `INTENTS: [IntentSpec; 83]`, `READS: [ReadSpec; 102]`, `ErrorCode`, `CHAIN_ERRORS`, `py::*` (extracted Python tables), `rt::*` (extracted runtime tables: custom codes, fee filters, `MAX_SHIELD_ERA_PERIOD`, `rate_limits`, chain rules), `hyperparams::TABLE` (a generated `HyperparamTable`, §5.5) | none | `rtao-primitives` |
| `rtao-runtime` | chain surface (§3.7, §8.1) | GENERATED `storage::*`, `calls::*` (typed builders + `CallDescriptor`s), `runtime_apis::*`, `constants::*`, `events::*`, `errors::MODULE_ERRORS`, `extensions::EXPECTED`, `SelectiveMetagraphIndex`, `shapes`; hand-written `models::*`; `ShapeHash`, `verify_surface`, `SurfaceReport`; `RuntimeHandle`, `RuntimeEntry`, `MetadataLevel`, `CallBytes`, `ComposeError` (no I/O, §3.7) | none | `rtao-core`, `rtao-primitives` |
| `rtao-rpc` | transport (§3.2) | `Session`, `SessionConfig`, `EndpointPool`, `ReadMethod`, `RpcError`, `PolicyRefusal`, `RefusalKind`, `SubmitError`, `SessionEvent`, `SessionState`, `RateLimit`; `impl subxt_rpcs::client::RpcClientT for Session` | `rate-limit` (default), `metrics` | none (third-party only; generated `py_rpc.rs` constants) |
| `rtao-signer` | signing seam (§6.3) | `Signer`, `SignerCaps`, `SigningRequest`, `SigningContext`, `MultiSignature`, `Role`, `Coldkey`, `Hotkey`, `SignerRole`, `KeySource<R>`, `DynKeySource`, `KeypairSigner`, `LedgerSigner`, `VaultSigner`, `ExtensionSigner`, `extension::{BridgeServer, BridgeClient, connect_extension_signer, ensure_bridge, run_bridge, stop_bridge_daemon}`, `vault::{VaultSigner, VaultSessionServer, VaultError, VaultPageError, scan_vault_address}`, `RemoteSigner` (M10) | `ledger`, `vault` (the one-shot page server on axum with a hyper WebSocket upgrade, and the QR encoder, §6.5), `extension` (bridge client on `tokio-tungstenite`, bridge server on axum with a hyper upgrade, §6.5), `remote` (M10, outside `full`), `test-util` (makes the `LedgerBackend` seam and `LedgerSigner::with_backend` public for `rtao-testkit`, §6.5) | `rtao-core`, `rtao-primitives` |
| `rtao-wallet` | btcli-compatible wallets (§6.1-6.2) | `Wallet`, `WalletRef`, `HotkeyFile`, `ColdkeyFile`, `PasswordSource`, `PasswordSources`, `PromptPolicy`, `resolve_wallet_password`, create/regen | `encrypted` (→ `rtao-core/host`), `keychain` (macOS), `keyring` (Linux, rtao extra), `prompt` | `rtao-signer`, `rtao-core` |
| `rtao-client` | chain access (§3) | `Client` (`connect`, `from_rpc`, `nonces()`, `decode_scale`, `estimate_weight`, `estimate_shielded_carrier_fee`, `block_info`), `ClientConfig`, `ClientError`, `Network`, `HeadPolicy`, `Snapshot`, `View`, `DynView`, `RuntimeCache` (and re-exports of `rtao_runtime::{RuntimeHandle, MetadataLevel, CallBytes, ComposeError}`), `ReadPlan`, `HeadTracker`, `BlockHeader`, `BlockInfo`, `EpochEvent`, `Events`, `EventRecord`, `SubmitOptions`, `SubmitFailure`, `Wait`, `Mortality`, `ShieldPeriod`, `ShieldCiphertext`, `NonceManager`, `NonceLease`, `ExtrinsicReport`, `PoolRejection`, `UnsignedExtrinsic`, `compose::*`, `config` (btcli files and caches, §10.2), `EpochClock`, `EpochStatus`, `wait_for_*`, `blocking::BlockingRuntime` | `blocking` (the runtime thread only; the facades are in `rtao::blocking`), `subxt` (escape hatch), `metrics`, `test-util` | `rtao-rpc`, `rtao-runtime`, `rtao-catalog`, `rtao-signer`, `rtao-core` |
| `rtao-tx` | product layer for writes (§4.2-4.7) | `IntentInterface`, `Intent`, `DynIntent`, `BuildCx`, `PreflightCx`, `Built`, `IntentPreflight`, `Spend` (re-exported from `rtao-primitives`), `Executor`, `ExecuteOptions`, `PlanOptions`, `ShieldOptions`, `Plan`, `Policy`, `ExtrinsicResult<O>`, `ChainError`, `PolicyError`, `TxError`, `classify`, `multisig::{Multisig, ApproveOptions}` and the multisig funds preflight (§3.12, §4.6), `timelock::{seal, open, Timelocked, Reveal, OpenOptions, TimelockError, DrandClient}` (§9.8) | none | `rtao-client`, `rtao-catalog`, `rtao-signer` |
| `rtao-intents` | the 83 intents (§4.2-4.5) | one struct per op (`AddStake`, `SetWeights`, …), `AnyIntent`, `weights::{conform, TimelockCommitter}`, `pow::PowSolver`, `hyperparameters::{Hyperparameter, to_raw}` (`to_raw` delegates to `rtao_catalog::hyperparams::TABLE`, §5.5), `MultisigAdapter`, `MultisigThreshold1Adapter` | one feature per Python group (`staking`, `weights`, `registration`, `transfer`, `evm`, `children`, `coldkey`, `collateral`, `lock`, `governance`, `hyperparameters`, `identity`, `root`, `serving`, `proxy`, `multisig`, `crowdloan`, `leasing`, `association`, `batch`), `all`, `pow` (→ core `host`), `pow-gpu` | `rtao-tx`, `rtao-reads` |
| `rtao-reads` | the 102 reads (§4.8) | `ReadName`, `ReadSpec`, `ParamSpec`, `dispatch`, `ReadError`, read fns, 14 namespace types (`Balances`, `Chain`, `Delegation`, `Epochs`, `Hyperparameters`, `Identity`, `Leasing`, `Locks`, `Collateral`, `Neurons`, `Prices`, `Staking`, `Subnets`, `Weights`), read output models, `commitments::{NeuronCommitment, CommitmentMap, fetch_commitments, fetch_commitment_map}` (§8.2) | none | `rtao-client`, `rtao-catalog`, `rtao-runtime` |
| `rtao-metagraph` | metagraph (§8.2-8.5) | `Metagraph`, `MetagraphNeuron`, `MetagraphView`, `NeuronRef`, `SubnetsMetagraphExt` (`metagraph`, `metagraph_with`), `MetagraphOptions`, `Preset`, `MetagraphSync`, `MetagraphHandle`, `MetagraphDiff`, `SyncPolicy`, `persist::{save, load}` | `persist` | `rtao-reads`, `rtao-runtime` |
| `rtao-btauth` | btauth/1 (§7) | `build_payload`, `sign`, `Verifier` (`authenticate`, `Authenticated::commit`, `verify`), `VerifyConfig`, `WrappedPolicy`, `Caller`, `AuthError`, `NonceStore`, `InMemoryNonceStore`; `server::{BtAuthLayer, VerifiedCaller, CallerPolicy, MetagraphPolicy, PerHotkeyLimiter, Json}` (`Json<T>`: the `Value`-first extractor, §7.2); `client::{SignedClient, FanOut}`; `RedisNonceStore`; `legacy::verify_v10` | `server`, `client`, `redis`, `metagraph-policy`, `legacy-v10` | `rtao-signer`, `rtao-core`, `rtao-primitives` (`AxonTarget`); `rtao-metagraph` (opt) |
| `rtao-neuron` | framework (§9) | `NeuronConfig`, `NeuronRuntime`, `NeuronCx`, `StateCell`, `Validator`, `Miner`, `StepPolicy`, `WeightsProposal`, `WeightScheduler`, `WeightPolicy`, `AxonPublisher`, `Registration`, `OwnerOps`, `HyperparamPlanner`, `OwnerMonitor`, `StateStore`, `EmaScores`, `telemetry`, `anyhow` (re-export, §2.5) | `validator`, `miner`, `owner`, `prometheus` (default), `auto-update` | `rtao-intents`, `rtao-metagraph`, `rtao-btauth`, `rtao-wallet` |
| `rtao-evm` | `PY/evm/*` parity (§6.6) | `EvmKeystore`, `Precompiles` (ABIs from `PY/evm/abi/*.json`), `EvmRpc`, `EvmNetwork`, `association_proof`, wei/rao conversions | none | `rtao-client`, `rtao-wallet` |
| `rtao-cli` | `rtao` binary (§10) | — | `ledger`, `vault`, `extension`, `evm` (all default on) | `rtao` (`full`) |
| `rtao-testkit` | test infrastructure (§11) | `FakeNode`, `FakeChain`, `FakeState`, `Fault`, `RecordingView`, `Localnet`, `LocalnetProfile`, `Scenario`, `PinProxy` (block-pinning JSON-RPC proxy for interop lane 9, §11.6), `fixtures::*`, `oracle::*`, `ledger_emu::*` (the Speculos harness, with the Speculos TCP and replay transports that `LedgerDevice::with_transport` takes from the core patch, §1.11, §11.9) | `docker`, `interop`, `core-oracle`, `ledger-emu` (each gates its own `[[test]]` target, §11.1) | `rtao-client`, `rtao-rpc`, `rtao-tx`, `rtao-wallet` (plus `subxt-rpcs` for `FakeChain`), `rtao-signer` (feature `ledger-emu` enables its `test-util`, §2.3 rule 8) |
| `rtao` | facade (§2.4) | `prelude` (contents per feature, §2.4), re-exports per feature, `blocking::{SyncClient, SyncSnapshot, Subtensor, set_weights, close_shared_clients}` (ports of `PY/sync.py` and `PY/_subtensor.py`, driven by `rtao_client::blocking::BlockingRuntime`; shared clients keyed by canonical endpoint, `PY/_subtensor.py:129-150`; `SyncClient` exposes all 14 namespaces, where Python's has 13, D-027) | §2.4 | all, optional per feature |

Two crates need a word of justification:

- `rtao-runtime` is separate from `rtao-catalog`. A runtime bump regenerates descriptors; a Python bump regenerates
  catalog data. Each recompiles only its own dependents.
- `rtao-rpc` is separate from `rtao-client`. The transport has no chain knowledge, so a WebSocket-library change
  touches one crate, and `FakeNode` tests it in isolation.

### 2.3 Dependency layers and enforced rules

```
L0  bittensor-core (git, core_tag) · tokio · tokio-tungstenite · subxt-rpcs (default-features = false) · scale-* (shared)
L1  rtao-core-ext → bittensor-core
L1' rtao-core     → bittensor-core, rtao-core-ext
L2  rtao-primitives → rtao-core
L3  rtao-catalog → primitives · rtao-runtime → core, primitives · rtao-rpc (third-party only) · rtao-signer → core, primitives
L4  rtao-wallet → signer, core · rtao-client → rpc, runtime, catalog, signer, core
L5  rtao-tx → client, catalog, signer · rtao-reads → client, catalog, runtime · rtao-evm → client, wallet
L6  rtao-intents → tx, reads · rtao-metagraph → reads, runtime · rtao-btauth → signer, core, primitives (+ metagraph, optional)
L7  rtao-neuron → intents, metagraph, btauth, wallet
L8  rtao (facade, incl. rtao::blocking) · rtao-cli → rtao · rtao-testkit (dev) · xtask (tooling)
```

`cargo xtask deps-check` runs in `pr-gates` over `cargo metadata` and `cargo tree`:

1. Only `rtao-core`, `rtao-core-ext`, `rtao-testkit` and `xtask` may depend on `bittensor-core`.
2. Only `rtao-rpc`, `rtao-testkit` and `rtao-signer` with feature `extension` or `vault` (the extension bridge client,
   and the WebSocket upgrades of the extension bridge server and the Vault page server, §6.5) may depend on
   `tokio-tungstenite`, all at the single workspace version. axum's
   own `ws` feature is not used: axum 0.8.9 requires `tokio-tungstenite ^0.29` (crates.io index), which would put a
   second `tungstenite` next to 0.30 and trip the `deny.toml` duplicate ban. Only `rtao-rpc`, `rtao-client` and
   `rtao-testkit` (`FakeChain`, §11.3) may depend on `subxt-rpcs`. `jsonrpsee` and `subxt` must be absent from the
   default graph and may appear only under the facade feature `subxt`. `deps-check` asserts the `extension` and `vault`
   gates, and that neither `jsonrpsee` nor `subxt` nor `rtao-signer/extension` nor `rtao-signer/vault` is in the no-host
   profile.
3. `rtao-catalog` and the `generated/` modules are 100% generated: `xtask regen --check` must be a no-op.
4. `rtao-btauth` without features has no tokio, axum, reqwest or hyper. Every intra-workspace dependency is declared
   `default-features = false` and enables exactly what it uses (`rtao-core/offload` only from `rtao-client`,
   `rtao-wallet` and `rtao-signer/ledger`; the facade depends on `rtao-neuron` without `prometheus`), so a feature
   turned off in the facade stays off.
5. Profile `no-host` (`validator,prometheus` and, separately, `miner,redis`, both `--no-default-features`) has, in
   `cargo tree -e normal,build`, none of the crates listed in §1.7 (libsodium, OpenSSL, `secp256k1-sys`, `aws-lc-*`,
   `cmake`, `hidapi`, `libloading`, `rayon`, …), no blocking `reqwest`, and `sp-core` without `std`.
6. Exactly one built `w3f-bls` uses `core_tag`, with only the recorded dormant lock entry allowed (§1.3, PLAN.md Q26).
7. Exactly one rustls crypto provider: `aws-lc-rs`, `aws-lc-sys` and `native-tls` are absent from every profile
   (`deny.toml` bans them too; §2.7).
8. Test seams stay out of shipped graphs: `rtao-signer/test-util` and `rtao-client/test-util` are enabled only by
   `rtao-testkit` and dev-dependencies, never by a facade feature or by `rtao-cli`.

PR-U17's items, which only the core patch provides, stay out of shipped crates through `clippy.toml` and the
`core-unpatched` job rather than `deps-check` (§1.11).

### 2.4 Facade features and role profiles

```toml
# crates/rtao/Cargo.toml [features]
default         = ["client", "reads", "tx"]                       # library default; no host
client          = ["dep:rtao-client"]
reads           = ["client", "dep:rtao-reads"]
tx              = ["client", "dep:rtao-tx", "dep:rtao-signer"]
intents         = ["tx", "reads", "dep:rtao-intents"]             # rtao::intents; the group features pick AnyIntent variants
metagraph       = ["reads", "dep:rtao-metagraph"]
wallet          = ["dep:rtao-wallet"]                              # plaintext hotkeys via staged keyfile_json; no host
wallet-encrypted = ["wallet", "rtao-wallet/encrypted"]             # NaCl/Fernet/Ansible coldkeys: enables bittensor-core/host
btauth          = ["dep:rtao-btauth"]
btauth-server   = ["btauth", "rtao-btauth/server", "rtao-btauth/metagraph-policy", "metagraph"]
btauth-client   = ["btauth", "rtao-btauth/client"]
redis           = ["rtao-btauth/redis"]
neuron          = ["intents", "metagraph", "wallet", "btauth-client", "dep:rtao-neuron"]
validator       = ["neuron", "rtao-intents/weights", "rtao-intents/serving", "rtao-neuron/validator"]
miner           = ["neuron", "btauth-server", "rtao-intents/serving", "rtao-neuron/miner"]
owner           = ["neuron", "wallet-encrypted", "rtao-neuron/owner",
                   "rtao-intents/registration", "rtao-intents/governance", "rtao-intents/hyperparameters",
                   "rtao-intents/identity", "rtao-intents/proxy", "rtao-intents/multisig", "rtao-intents/collateral"]
intents-all     = ["intents", "rtao-intents/all"]
pow             = ["intents", "rtao-intents/pow"]                  # enables bittensor-core/host (CPU PoW)
pow-gpu         = ["pow", "rtao-intents/pow-gpu"]                  # OpenCL via libloading (no link-time dependency)
ledger          = ["rtao-signer/ledger"]
vault           = ["rtao-signer/vault"]
extension       = ["rtao-signer/extension"]
remote-signer   = ["rtao-signer/remote"]                          # lands with RemoteSigner in M10; not part of `full`
keychain        = ["rtao-wallet/keychain"]
evm             = ["dep:rtao-evm"]
blocking        = ["intents-all", "metagraph", "wallet", "rtao-client/blocking"]  # rtao::blocking: SyncClient, Subtensor, set_weights (PY/sync.py, PY/_subtensor.py)
prometheus      = ["rtao-neuron?/prometheus", "rtao-rpc/metrics"]
subxt           = ["rtao-client/subxt"]                            # OnlineClient over the same Session (LegacyBackend)
full            = ["intents-all", "metagraph", "wallet-encrypted", "btauth-server", "btauth-client", "redis",
                   "neuron", "rtao-neuron/validator", "rtao-neuron/miner", "rtao-neuron/owner",
                   "pow", "ledger", "vault", "extension", "keychain", "evm", "blocking", "prometheus"]
```

| Profile | Features | `host` | Typical binary |
|---|---|---|---|
| validator | `validator` (+ `prometheus`) | no | weight-setting validator, hotkey only |
| miner | `miner` (+ `redis`) | no | axum miner behind btauth/1 |
| owner | `owner` (+ `ledger`) | yes | owner tooling, coldkey operations |
| CLI / agents | `full` | yes | `rtao` binary, agents using `execute_tool` |

The intent group features also trim compile time: `AnyIntent` variants are `#[cfg(feature = "<group>")]`, and the
parity gate builds with `all`.

**What each feature exposes.** With `dep:` syntax there is no implicit feature to `cfg` on, so the facade gates its
modules and prelude items on its own features:

| Feature | Module | Prelude additions |
|---|---|---|
| always | `rtao::{core, primitives}` | `NetUid`, `MechId`, `Uid`, `AccountId`, `Balance`, `Tao`, `Alpha` |
| `client` | `rtao::client` | `Client`, `ClientConfig`, `View`, `ViewExt`, `Snapshot`, `BlockRef` |
| `reads` | `rtao::reads` | `Namespaces` (catalog-derived accessors on every `View`) |
| `tx` | `rtao::{tx, signer}` | `Executor`, `ExecuteOptions`, `PlanOptions`, `Policy`, `ExtrinsicResult`, `TxError` |
| `intents` | `rtao::intents` | `AnyIntent` |
| `metagraph` | `rtao::metagraph` | `SubnetsMetagraphExt` (so `client.subnets().metagraph(netuid)` resolves, §8.2) |
| `wallet` | `rtao::wallet` | `Wallet`, `HotkeyFile` |
| `btauth` | `rtao::btauth` | — |
| `neuron` | `rtao::neuron` | `NeuronConfig`, `NeuronRuntime`, `NeuronCx`, `Validator`, `Miner`, `StateCell`, `WeightsProposal` |
| `evm` | `rtao::evm` | — |
| `blocking` | `rtao::blocking` | — |

`pr-gates` compiles the plan's three examples as `crates/rtao/examples/{validator,miner,owner}.rs`, each with
`required-features` set to its profile, so a profile that stops exposing what its example uses fails the build. Each
example, and its line in the `features` job, lands with its profile: `validator` in M2, `miner` in M3, `owner` in M4
(§14).

### 2.5 Edition, MSRV, toolchain, lints, license

- **Edition 2024** for every rtao crate. The core stays on 2021, which Cargo handles per crate.
- **MSRV 1.89**: the maximum of the monorepo toolchain at `core_tag` (`UP/rust-toolchain.toml` channel 1.89
  [J-MA §3.4]) and the dependency floors (tokio-tungstenite 0.30 needs 1.85, proptest 1.12 needs 1.88; [R] F §0.1).
  `xtask pin` raises it when the monorepo does. The MSRV covers the facade, its dependency closure and `rtao-cli`; CI
  checks them with `cargo +1.89` (the `rust-toolchain.toml` pin outranks `rustup default`, so a plain `cargo` would
  build on stable). `xtask` (using the RustPython parser fallback; see PLAN.md M0-12) and
  `rtao-testkit` build only with `rust-toolchain.toml`. Development uses current stable (1.99).
- **Async traits** use return-position `impl Future + Send` (stable since 1.75) on generic paths. Object-safe twins
  (`DynView`, `DynIntent`, `DynKeySource`, `Signer`, `NonceStore`) box their futures. Those are
  heterogeneous-runtime-choice seams, called at most a few times per request or block. Every trait used behind
  `&dyn`/`Arc<dyn>` or captured by an RPITIT `+ Send` future is `Send + Sync` (`IntentInterface: Send + Sync + 'static`,
  `DynIntent`, `Clock`, `PolicySubject`, as `View`, `Signer`, `KeyView`, `NonceStore` and `CallerPolicy` already are).
  Without it a provided `+ Send` method that captures `&self` does not compile ("future cannot be sent between
  threads safely … `Self` is not `Sync`", reproduced with rustc 1.99).
- **Lints.**
  - Workspace: `missing_docs = "deny"` on library crates, `unsafe_code = "forbid"`, clippy `-D warnings` plus
    `clippy.toml` (§1.8), and `unwrap_used`/`expect_used`/`panic` denied in non-test library code.
  - `rtao-primitives` (money) and `rtao-core-ext` (code destined for upstream) also deny `arithmetic_side_effects` and
    `indexing_slicing`, the monorepo's deny set (`UP/Cargo.toml` `[workspace.lints.clippy]` [J-PD §3 D#6]).
  - `cargo xtask lint-upstream` runs the monorepo's `subtensor-linting` (git dependency at `core_tag`) over
    `rtao-core-ext` only. This is narrower than D's whole-workspace adoption, which would force `#[freeze_struct]` on
    codec derives and slow contributors for no benefit outside the staged crate [J-PD §4 D].
- **Errors**: `thiserror` enums per crate; `anyhow` only in binaries, examples and tests, with one exception: the
  framework's user-callback traits (`Validator::{on_start, step}`, `Miner::on_start`) and `NeuronRuntime::run` return
  `anyhow::Result<()>`, because user code produces arbitrary errors. `rtao-neuron` re-exports it as
  `rtao::neuron::anyhow`, so subnet crates and rtao share one version, and `cargo semver-checks` treats it as part of
  `rtao-neuron`'s public API.
- **License**: dual `MIT OR Apache-2.0`, decided by the user (§15.2 Q1; D §2.6), replacing the skeleton's MIT, because
  rtao vendors and ports Apache-2.0 upstream material (fixtures, transpiled tables, the epoch-schedule copy, static Vault
  and extension assets, ported Python logic) and because dual licensing keeps an `sdk/bittensor-rs` move open. M0's
  first steps relicense the skeleton: `[workspace.package] license = "MIT OR Apache-2.0"`, `LICENSE-MIT`,
  `LICENSE-APACHE` (the Apache-2.0 text, which also meets §4(a) for derived material) and a `NOTICE` file that
  attributes vendored and derived material (upstream ships none). Every Rust file copied or derived from upstream
  (staged, ported, generated from extracted tables) starts with `// Derived from RaoFoundation/subtensor <path> @
  <commit> (Apache-2.0); modified by rtao.` (§4(b), §4(c)), while vendored assets stay byte for byte and are listed in
  `NOTICE`. Crates without derived files declare `MIT OR Apache-2.0`; crates that contain derived files declare
  `license = "(MIT OR Apache-2.0) AND Apache-2.0"`, and `xtask parity check` derives that set from the `rs` column of
  `parity/map.toml` and from the files that carry the attribution header.

### 2.6 What a subnet owner's manifests look like

The repository URL is the decided one (§15.2 Q5); the tags are illustrative.

```toml
# validator/Cargo.toml — hotkey only; no Perl, OpenSSL or libsodium
[package]
name = "sn42-validator"
edition = "2024"
rust-version = "1.89"

[dependencies]
rtao   = { git = "https://github.com/sam0x17/rtao", tag = "rtao-v1.0.0", default-features = false, features = ["validator", "prometheus"] }
tokio  = { version = "1.53", features = ["rt-multi-thread", "macros", "signal"] }
serde  = { version = "1", features = ["derive"] }
anyhow = "1"

# Required until upstream PR-U4 lands: Cargo ignores [patch] in dependencies. The tag MUST equal
# rtao::core::BITTENSOR_CORE_TAG (the facade re-exports rtao-core as `rtao::core`); `rtao doctor` checks Cargo.lock
# and prints the exact line.
[patch.crates-io]
w3f-bls = { git = "https://github.com/RaoFoundation/subtensor.git", tag = "v477" }
```

```toml
# miner/Cargo.toml — btauth/1 server; no Perl, OpenSSL or libsodium
[dependencies]
rtao  = { git = "https://github.com/sam0x17/rtao", tag = "rtao-v1.0.0", default-features = false, features = ["miner", "redis"] }
axum  = "0.8"
tokio = { version = "1.53", features = ["rt-multi-thread", "macros", "signal"] }

[patch.crates-io]
w3f-bls = { git = "https://github.com/RaoFoundation/subtensor.git", tag = "v477" }
```

```toml
# owner-tool/Cargo.toml — coldkey operations; builds the core with `host` (needs Perl and make)
[dependencies]
rtao = { git = "https://github.com/sam0x17/rtao", tag = "rtao-v1.0.0", default-features = false, features = ["owner", "ledger"] }

[patch.crates-io]
w3f-bls = { git = "https://github.com/RaoFoundation/subtensor.git", tag = "v477" }   # = rtao::core::BITTENSOR_CORE_TAG
```

In a workspace holding all three crates, the patch line appears once, in the workspace root: Cargo ignores `[patch]`
in member manifests.

The template's `Dockerfile` builds neuron binaries with `RUN --mount=type=cache,target=/usr/local/cargo/git
--mount=type=cache,target=/usr/local/cargo/registry cargo build --release --locked -p sn42-validator`. Building with
`-p` keeps the owner tool's `host` feature out of neuron images (§1.7), and the cache mounts pay the cold git fetch
(§2.8) once. Neuron images build in `rust:1.89-slim` as shipped (its C compiler links the binary and builds `ring` and
`blake3`); only the owner image adds `perl` and `make`. The template CI caches `~/.cargo/git/db`.

### 2.7 Third-party stack

Versions are taken from [R] F §0.1 and the coexistence build, with registry checks [V] where noted.

| Area | Crate | Notes |
|---|---|---|
| runtime | `tokio 1.53` | `rt-multi-thread, macros, sync, time, signal`; `test-util` in dev |
| WebSocket | `tokio-tungstenite 0.30` (`rustls-tls-native-roots`; the connector is supplied by `rtao-rpc`, see TLS) | handshake `Error::Http(Box<Response<Option<Vec<u8>>>>)` and `CloseCode::Policy` = 1008 verified on 0.28.0 (`REG/tungstenite-0.28.0/src/error.rs:71`, `protocol/frame/coding.rs:154,214` [V]); 0.30 assumed API-stable [I], pinned by a `FakeNode` test; `connect_async_tls_with_config` and `WebSocketStream::from_raw_socket` exist in 0.30.0 (`src/connect.rs:61`, `src/lib.rs:218`) |
| RPC seam | `subxt-rpcs 0.51.1`, `default-features = false` | `RpcClientT` (`src/client/rpc_client_t.rs:26-52` [V]), `LegacyRpcMethods`, `Error::Client(Box<dyn Error>)` for typed errors (`src/lib.rs:69-94` [V]); no jsonrpsee without the default `jsonrpsee` feature (`Cargo.toml [features]` [V]) |
| SCALE | `parity-scale-codec 3.7.5`, `scale-info 2.11.6`, `scale-decode 0.16.2`, `scale-encode 0.10.1`, `scale-type-resolver 0.2` (feature `scale-info`, enabled explicitly; [J-MA §3.3]), `frame-metadata 23.0.1` (tooling, V16) | one copy shared with the core ([R] feasibility-veneer-coexistence) |
| HTTP | `axum 0.8.9` (without its `ws` feature, §2.3 rule 2), `tower 0.5.3`, `tower-http 0.7.1`, `http 1`, `reqwest 0.13.5` (`default-features = false`, features `rustls-no-provider`, `json`, `http2`, `charset`; preconfigured ring `ClientConfig`, see TLS) | btauth server and client, async drand, EVM RPC |
| TLS | `rustls 0.23`, ring provider only: `[workspace.dependencies] rustls = { version = "0.23", default-features = false, features = ["ring", "std", "tls12", "logging"] }`, plus `rustls-native-certs` | One provider for the whole graph, chosen explicitly. The core's `host` enables `rustls/ring` through its blocking reqwest 0.12 (`UP/sdk/bittensor-core/Cargo.toml:60`), and reqwest 0.13's default `rustls` feature adds `aws-lc-rs`. With both (owner, CLI) or neither (`client`-only builds), rustls finds no process default and `ClientConfig::builder()` panics (`REG/rustls-0.23.45/src/crypto/mod.rs:243-286`, reproduced); tokio-tungstenite 0.30's built-in connector calls exactly that (`REG/tokio-tungstenite-0.30.0/src/tls.rs:126`). FakeNode, localnet, e2e and interop all use `ws://`, so only a TLS test catches it. Rules: (1) library crates never consult or install the process default. `rtao-rpc` builds `ClientConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider())).with_safe_default_protocol_versions()` with the native roots plus `SessionConfig::extra_roots`, ALPN `http/1.1` only (WebSockets need HTTP/1.1), and connects with `connect_async_tls_with_config(request, Some(ws_config), false, Some(Connector::Rustls(cfg)))`. (2) Every rtao `reqwest` 0.13 client (`SignedClient`, `DrandClient`, `EvmRpc`, and `rtao-cli`'s GitHub-release fetcher for `upgrade` and indexer client for `wallet history`, `PY/cli/upgrade_helpers.py:103-129`, `PY/cli/commands/wallet.py:1335-1352`) is built with `ClientBuilder::tls_backend_preconfigured(config)`, where `config` is an **owned** `rustls::ClientConfig` with ALPN `h2` and `http/1.1`; an `Arc` is not recognized and `build()` fails (`REG/reqwest-0.13.5/src/async_impl/client.rs:2209-2228`). (3) Applications install ring as the process default, as a backstop for third-party code: `rtao-cli`'s `main` and the template binaries first call `let _ = rustls::crypto::ring::default_provider().install_default();`. With `rustls-no-provider`, a reqwest client built without a preconfigured config panics when no default exists (`client.rs:2499-2507`). (4) `deny.toml` and `deps-check` ban `aws-lc-rs`, `aws-lc-sys` and `native-tls` (§2.3 rule 7). `ring` matches the core's reqwest 0.12, so `host` builds keep one provider, and it needs only `cc`. Tests: `rtao-rpc` and `SignedClient` connect to a loopback `wss://`/`https://` `FakeNode` with a self-signed certificate (added through `extra_roots`), in an `--all-features` build, in the no-host profile, and in a build that also enables `rustls/aws-lc-rs` (third-party feature unification); the template's test asserts `CryptoProvider::get_default().is_some()` after `main` starts |
| rate limits | `governor` [I: crate choice] | token buckets in `rtao-rpc` and `PerHotkeyLimiter` |
| concurrency | `arc-swap 1.9`, `dashmap 6`, `tokio-util` (`CancellationToken`) | |
| CLI | `clap 4.6.7` (builder API for generated commands), `clap_complete`, `rpassword`, `comfy-table` | |
| JSON | `serde_json 1` with `arbitrary_precision`: `[workspace.dependencies] serde_json = { version = "1.0.132", features = ["arbitrary_precision"] }` | The core enables the feature non-optionally (`UP/sdk/bittensor-core/Cargo.toml:40-42`) and needs it: `CORE/codec/value.rs:58-77` renders u128 and i128 values beyond 64 bits and U256 through it and substitutes 0 without it. Cargo unifies it into every rtao and subnet build, where non-integer numbers parsed from text fail inside `#[serde(untagged)]` enums and `#[serde(flatten)]` structs and `serde_json::Value` equality compares number spelling (`1e-05 != 1e-5`), measured on serde_json 1.0.154 (`REG/serde_json-1.0.154/src/de.rs:118-129, 937-953`, `src/number.rs:20-24, 72-73`). Rules: rtao depends on the feature itself, so its JSON stays digit-exact whatever upstream does; rtao's number-or-string and untagged input types (`WeightsArg`, `Amount`, `Ratio`) deserialize through `serde_json::Value` and read numbers from `Number::as_str()`; every text entry point (CLI `--args` and `call`, `execute_tool` from text, `UnsignedExtrinsic` import, `btcli.json` and the books, oracle vectors) parses through `rtao_primitives::json::{from_slice, from_str}`; subnet authors use `rtao::btauth::server::Json<T>` (§7.2); the oracle comparator never uses `Value` equality (§4.8). An M0 test pins direct versus `Value`-first parsing of untagged and flatten floats, a u128/U256 `to_corpus_json` round trip, and the comparator vectors of §4.8 |
| QR (Vault) | a QR encoder with SVG output [I: crate chosen at M7] | `rtao-signer` feature `vault` only. Python renders each UOS frame with `qrcode` in byte mode without mode optimization (`optimize=0`), error correction M and a 4-module quiet zone, as an SVG data URI (`PY/vault/qr.py:15-28`). The encoder must emit one byte-mode segment; tests decode the rendered symbol back to the frame bytes, because module patterns may legitimately differ between encoders (mask choice) while the payload may not |
| observability | `tracing 0.1.44`, `tracing-subscriber 0.3.23`, `metrics 0.24.6`, `metrics-exporter-prometheus 0.18.3` | |
| secrets | `zeroize`, `secrecy 0.10` | |
| EVM | `alloy-primitives`, `alloy-sol-types`, `alloy-json-abi`, `alloy-signer-local` [I: versions chosen at M7] | `rtao-evm` only |
| testing | `proptest 1.12`, `insta 1.49`, `criterion 0.8.2`, `cargo-nextest`, `cargo-fuzz` | plain `docker` CLI for localnet (no testcontainers, as upstream, [R] H §1.3) |
| docs | `docify 0.4.1`, `mdbook 0.5.4` | |
| tooling (xtask only) | `syn 2`, `toml_edit`, `rustpython-parser 0.4.0` (the planned fallback, selected in M0 because Ruff conflicts with the core Unicode dependency), `blake3` | never compiled into subnet binaries |

The M0 implementation of `json::{from_slice, from_str}` adds a recursive adapter to Value-first parsing:
plain `serde_json::from_value` still rejects untagged `1e-05`. The adapter uses Serde's content-buffering hook
(serde >=1.0.228) for ordinary nested untagged/flattened floats while preserving original number tokens for
custom `Money`, `Amount`, `Ratio` and corpus `Value` fields. Serde's buffered content stores decimal scalars
as f64; exact decimal domain values therefore use ordinary custom fields rather than enclosing derived
untagged/flattened content. The adapter rejects non-finite numeric conversions and preserves full-width
integers at the exact domain/chain JSON boundaries. M0 tests pin these boundaries.

### 2.8 Build weight

- Core without `host` at `tag = "v477"`: 241 lockfile packages, 12.9 s wall / 84.6 s CPU clean release
  (`SP/design-tagpin/tagpin.log` [V]). With `host`: 330 crates ([R] feasibility-bittensor-core-gitdep).
- The neuron profiles stay near the no-`host` figure plus rtao's own stack (tokio, tungstenite, axum, reqwest, clap,
  tracing): an estimated +70-100 crates [I]. Relevant hosted dependency checks can record this in `COMPAT.md`; repeated build measurements are optional.
- The current M1 preparation measured 225 normal/build package identities for no-host `rtao-core`, 318 for each
  validator/miner profile and 394 for owner on `aarch64-apple-darwin`, Rust 1.99. Validator and miner leave
  `bittensor-core/host` disabled; owner enables it through the encrypted wallet. These are target-specific package
  counts, with no build-time claim. [Measurement and commands](../execution/m1-build-weight.md) must be refreshed
  and may be included in `COMPAT.md` from actual hosted dependency evidence as profiles grow.
- **Cold fetch.** The 12.9 s figure is a warm build. A cold build (a neuron Docker image or subnet CI without a cache)
  first clones the git history of `RaoFoundation/subtensor` (about 293 MB) and `RaoFoundation/polkadot-sdk` (about
  480 MB) (measured with `du -sh ~/.cargo/git/db/*` after the v477 builds; `SP/design-tagpin/tagpin.log` shows "Updating git
  repository" for all three sources). The template Dockerfile uses BuildKit cache mounts for `/usr/local/cargo/git` and
  `/usr/local/cargo/registry` (§2.6), and the template CI caches `~/.cargo/git/db`. While the core patch exists (§1.11),
  rtao's own builds, never a subnet's, also clone the fork's history, a second database of about the same size.
- No subxt codegen in the default graph. subxt's static finney module is 60,420 lines ([R] F §1.3); rtao's generated
  descriptors cover only the scope in `codegen/scope.toml`.

---

## 3. Chain client (`rtao-rpc`, `rtao-client`, `rtao-runtime`)

The behavioral reference is the Python transport: `PY/_transport/*` (3,941 lines) and `PY/_substrate.py` (851 lines)
([R] A1 §1, [J-PD §3 A#5]). Every chain-defined step is delegated to the core.

### 3.1 Async model

- tokio multi-thread runtime, owned by the application. rtao spawns tasks but never builds a runtime, except
  `rtao_client::blocking::BlockingRuntime`, which mirrors Python's private loop thread (`PY/sync.py` `_Loop`, `:64-98`)
  and which `rtao::blocking` drives (§2.2). `Client` is `Clone + Send + Sync` (an `Arc` inside) and holds no global
  state.
- **Background tasks per `Client`**, all cancelled when the last clone drops:
  - the `rtao-rpc` session supervisor (one task owns the socket);
  - the `HeadTracker` (best and finalized subscriptions with gap backfill);
  - the runtime-version watcher (`state_subscribeRuntimeVersion`, with a 12 s TTL poll as fallback, which is Python's
    only mechanism, `PY/_transport/runtime.py:142-176`).

  The mechanism: `Client { inner: Arc<ClientInner>, _alive: Arc<DropGuard> }`. Background tasks hold only
  `Arc<ClientInner>` and a child `CancellationToken`, never the user-facing `Client`; `_alive` wraps
  `tokio_util::sync::DropGuard` for that token. Dropping the last user-facing clone (including the ones inside
  `Snapshot`, `Executor` and `MetagraphSync`) therefore cancels the tasks and closes the socket. A `FakeNode` test
  observes the close within 1 s of dropping the last clone (§3.15).
- **Off the reactor** (`rtao_core::offload` = `spawn_blocking`):
  - `Runtime::parse` (about 1 ms warm for V15, `SP/design-bench/bench-run-3.txt` [V]; once per spec);
  - keyfile unlock (Argon2i 512 MiB);
  - Ledger HID (including `generate_extrinsic_proof`), PoW;
  - any decode of a payload above 64 KiB, on every path: dynamic `Value`, `DecodeAsType` models (the `MetagraphWire`
    fallback takes 11.0 ms for a 2,500-UID, 408 KB payload, `bench/decode-bench/bench-run-3.txt`), `from_dynamic` and
    `Metagraph::to_json`;
  - `batch::decode_map_page` above 64 items;
  - timelock encryption (about 1.4-1.6 ms per weight commit with core v477 without `host`, release build);
  - the RFC-0078 `metadata_digest` (about 2.5 ms), computed once per spec and cached on the `RuntimeHandle` (§3.9.2);
  - every fsync'd atomic file write (`StateStore`, the runtime disk cache, `persist::save`, keyfiles, the btcli files).
- **Inline**: signing, call composition, key hashing, typed decodes below 64 KiB, and the `MetagraphView` parse
  (6.5 µs at 256 UIDs, [V] bench; it borrows instead of decoding, so it stays inline at any size).
- **Cancellation and deadlines**: every waiting API takes an optional `Duration`. Python's watch has no overall
  deadline apart from the 60 s silence watchdog ([R] A1 §4.11), so rtao's library defaults are `None`. The framework
  sets explicit deadlines (§9.3).

### 3.2 Transport: `rtao-rpc::Session`

**Library choice.** The session runs on `tokio-tungstenite` and implements `subxt_rpcs::client::RpcClientT`.

- *Why not jsonrpsee.* A policy-refusal contract identical to Python's needs the HTTP response of a refused handshake.
  jsonrpsee 0.26 reduces it to `WsHandshakeError::Rejected { status_code }`
  (`REG/jsonrpsee-client-transport-0.26.1/src/ws/mod.rs:207, 522-524` [V]), which loses `X-RateLimit-Policy`,
  `X-RateLimit-Reason` and `Retry-After`. tungstenite returns the full response in `Error::Http(Box<Response<…>>)` and
  models close 1008 as `CloseCode::Policy` [V].
- *Why not `ReconnectingRpcClient`.* It "doesn't retain subscriptions and pending method calls"
  (`REG/subxt-rpcs-0.51.1/src/client/reconnecting_rpc_client/mod.rs:5-12` [J-MA §3.2]), so the per-method resend policy
  has to be owned anyway.
- *Why still `RpcClientT`.* With `subxt-rpcs` at `default-features = false`, the trait comes without jsonrpsee
  (`Cargo.toml [features]` [V]). That gives typed `LegacyRpcMethods`, `MockRpcClient` for unit tests, and an optional
  subxt `OnlineClient` on the same connection, as [J-PD graft 17] asks.

```rust
pub struct SessionConfig {
    pub pool: EndpointPool,                 // [primary] + fallbacks; archive pool opened lazily (StateDiscarded)
    pub handshake_timeout: Duration,        // 10 s         PY/_transport/rpc.py `_HANDSHAKE_TIMEOUT`
    pub silence_timeout: Duration,          // 60 s with requests pending → connection presumed dead (rpc.py:323)
    pub max_retries: u32,                   // 5 consecutive failures per endpoint before rotating (rpc.py:322)
    pub retry_forever: bool,                // false → give up after max_retries × endpoints
    pub subscription_buffer: usize,         // 2048, oldest dropped   `_SUBSCRIPTION_QUEUE_MAXSIZE`
    pub early_notification_ids: usize,      // 64: notifications racing the subscribe response (rpc.py:297-300)
    pub max_message_bytes: usize,           // 256 MiB [I] (Python: ws max_size = 2**32); sets tungstenite's max_message_size
                                            // AND max_frame_size (default 16 MiB): Substrate sends each response as one frame
    pub extra_roots: Vec<CertificateDer<'static>>,  // added to the native roots (private nodes, the TLS FakeNode; §2.7)
    pub max_in_flight: usize,               // 64 [I]; per-connection semaphore
    pub rate_limit: Option<RateLimit>,      // §3.2.5; preset per network
    pub headers: http::HeaderMap,           // e.g. Authorization for private nodes (marked sensitive in logs)
    pub plaintext_ws: PlaintextPolicy,      // AllowWithWarning (Python parity) | LoopbackOnly | Refuse
}
pub struct EndpointPool { pub urls: Vec<Url>, pub archive: Vec<Url> }   // empty fallbacks = pinned endpoint (Python)

pub struct Session { /* Arc<Inner>: supervisor task, pending map, subscriptions, state watch */ }
impl Session {
    pub async fn connect(cfg: SessionConfig) -> Result<Self, RpcError>;
    pub async fn request<R: DeserializeOwned>(&self, m: ReadMethod, params: impl Serialize + Send) -> Result<R, RpcError>;
    pub async fn request_many(&self, calls: Vec<(ReadMethod, serde_json::Value)>)          // JSON-RPC batch frame
        -> Result<Vec<Result<Box<RawValue>, RpcError>>, RpcError>;
    pub async fn subscribe<T: DeserializeOwned>(&self, sub: SubscriptionKind) -> Result<Subscription<T>, RpcError>;
    pub async fn submit(&self, xt: &[u8]) -> Result<H256, SubmitError>;                   // author_submitExtrinsic
    pub async fn submit_and_watch(&self, xt: &[u8]) -> Result<Subscription<TxStatus>, SubmitError>;
    pub fn state(&self) -> tokio::sync::watch::Receiver<SessionState>;   // Connected{url} | Reconnecting{attempt} | Failed
    pub fn events(&self) -> tokio::sync::broadcast::Receiver<SessionEvent>;
    pub fn archive(&self) -> &ArchivePool;
}
impl subxt_rpcs::client::RpcClientT for Session { /* request_raw / subscribe_raw; errors as Error::Client(Box<RpcError>), and
                                                  Box<SubmitError> for author_submit* (recovered with downcast_ref, §3.3) */ }
```

`subxt-rpcs` 0.51.1 uses `default-features = false, features = ["native"]`: the crate requires a target
feature, and `native` leaves optional jsonrpsee disabled. RPC's `H256` aliases third-party
`primitive-types::H256` and `TxStatus` uses its external transaction status carrier. Client adapters convert
the hash bytes explicitly into `rtao-primitives::H256`; RPC has no internal rtao dependencies.

**TLS.** `wss://` endpoints use the ring-only `rustls::ClientConfig` of §2.7, built once per `Session` and passed to
`connect_async_tls_with_config` as `Connector::Rustls`. The process-default `CryptoProvider` is never consulted, so an
unrelated dependency that enables a second rustls provider cannot make `Client::connect` panic.

**Frame size.** tungstenite enforces `max_frame_size` (default 16 MiB) separately from `max_message_size`
(`REG/tungstenite-0.28.0/src/protocol/mod.rs:81-102`), and Substrate's RPC server sends each response as one frame
(`REG/soketto-0.8.1/src/connection.rs:464-534`). Default nodes cap responses at 15 MiB, but archive or indexer nodes
started with a larger `--rpc-max-response-size` exceed 16 MiB, which Python accepts (`max_size=2**32`,
`PY/_transport/rpc.py:89`). `max_message_bytes` therefore sets both limits.

#### 3.2.1 The resend allowlist (`ReadMethod`)

Python never re-sends `author_submitExtrinsic` or `author_submitAndWatchExtrinsic` after a reconnect
(`_NON_IDEMPOTENT_METHODS`, `PY/_transport/rpc.py:66`). A duplicate can hit the pool as "already imported" while
the original is live, and the error path would then clear the nonce cache under a live transaction. B's graft makes
this structural:

```rust
/// Methods that may be transparently re-sent after a reconnect. `author_submit*` is deliberately absent, so the
/// resend path cannot express it. A unit test asserts both author methods fail `ReadMethod::from_str`.
#[non_exhaustive]
pub enum ReadMethod {
    StateCall, StateGetStorage, StateGetStorageAt, StateGetStorageHash, StateQueryStorageAt, StateGetKeysPaged,
    StateGetMetadata, StateGetRuntimeVersion, ChainGetHead, ChainGetFinalizedHead, ChainGetBlockHash, ChainGetHeader,
    ChainGetBlock, AccountNextIndex /* "account_nextIndex", Python's spelling (PY/_transport/extrinsics.py:69-77 [V]) */,
    SystemAccountNextIndex, SystemProperties, SystemChain, SystemHealth, RpcMethods,
}
```

`RpcClientT::request_raw` parses the method string. Anything that does not parse into a `ReadMethod` is non-resendable.
`author_submit*` that is in flight across a drop fails with `SubmitError::FateUnknown { hash }`; any other unknown
method fails with `RpcError::ConnectionLost { resent: false }`. Every method Python uses is in the allowlist, so this is
Python parity for Python's surface and fail-closed for everything else. The rule applies to frames that were
already **transmitted**. A submission queued while the socket was down and never sent goes out normally on the
new connection, as in Python's `_resubmit_pending` (`PY/_transport/rpc.py:686-708` [V]), unless its caller has gone
away. Each pending entry therefore carries a `sent` flag and the caller's `oneshot::Sender`. Before any frame is
transmitted (the first send after the connected gate, a resend after a reconnect, a release from the rate-limit
queue), the supervisor drops entries whose `Sender::is_closed()` is true. For `author_submit*` this is mandatory:
cancelling a submission drops its future and its `NonceLease`, which clears the nonce cache, so a late transmission
would reuse a nonce the next lease re-fetched. Python never transmits a frame nobody awaits
(`PY/_transport/rpc.py:495-504, 516-526, 549, 713-717`). The remaining race between the check and the write is the
window Python also accepts as fate unknown.

#### 3.2.2 Reconnect semantics (port of `rpc.py`, each row a `FakeNode` test, §3.15)

| Situation | Behavior |
|---|---|
| idempotent request in flight when the socket dies | re-sent on the next socket **before** the connected gate reopens (`rpc.py:686-725`) |
| `author_submit*` already transmitted | never re-sent; `FateUnknown { extrinsic_hash }`; the transaction layer clears the nonce (§3.9). A submit queued but never transmitted is sent on the new socket |
| subscriptions at the session level | each receives `Err(SubscriptionLost)`, with Python's guidance "verify balances before retrying" (`rpc.py:582-602`); `HeadTracker` resubscribes one layer up (§3.13) |
| extrinsic watch subscription | terminal `FateUnknown` (never re-subscribed) |
| connect failure, or `max_retries` consecutive timeouts | rotate endpoint; backoff `min(0.25·n, 5) s`; whole-pool cycles `min(2·cycle, 30) s` with `retry_forever` (`rpc.py:648, 676-678`); give up after `max_retries × endpoints` otherwise (`MaxRetriesExceeded`) |
| policy refusal (§3.2.3) | **terminal**: never retried, never rotated (rotation would amplify traffic against the gateway) |
| `"State already discarded for <hash>"` in `message` or string `data` | `RpcError::StateDiscarded { block }`; the read is retried once on the archive pool (`classify_rpc_error`, `rpc.py:202-209` [V]; `PY/_substrate.py:366-400`) |

#### 3.2.3 Policy refusals, exactly as Python classifies them

The classifier is a port of `_default_connect`, `_is_policy_rpc_error`, `classify_rpc_error` and `_policy_close_error`
(`PY/_transport/rpc.py:84-135, 190-241` [V]). The constants are **extracted, not hand-copied** (§4.1) into the
generated `rtao-rpc/src/generated/py_rpc.rs`:

- `_POLICY_RPC_ERROR_CODES = {-32004, -32005}`;
- `_POLICY_RPC_METADATA_KEYS = {"policy", "reason", "retry_after", "retryAfter", "retry_after_ms", "retryAfterMs"}`;
- `_POLICY_RPC_ERROR_MESSAGES = {-32004: {"storage work rate limit exceeded"}}`;
- `_STATE_DISCARDED_NEEDLE`;
- the diagnostic length limits (500 and 200).

```rust
pub struct PolicyRefusal {
    pub kind: RefusalKind,                  // Handshake { status: 403 | 429 } | JsonRpc { code: -32004 | -32005 } | Close1008
    pub message: String,                    // Python's wording, e.g. "RPC endpoint rate limited this source (HTTP 429): … (policy=…, reason=…)"
    pub status_code: Option<u16>,           // 403/429, or 1008 for closes (Python sets status_code=1008)
    pub retry_after: Option<String>,        // raw, sanitized (≤200 chars), as Python exposes it
    pub policy: Option<String>,
    pub reason: Option<String>,
}
impl PolicyRefusal {
    /// Parsed convenience (rtao addition): delta-seconds, *_ms keys, or HTTP-date.
    pub fn retry_after_duration(&self) -> Option<Duration>;
}
```

- **Handshake 403/429.** `policy` comes from `X-RateLimit-Policy`, `reason` from `X-RateLimit-Reason`, and
  `retry_after` from `Retry-After`, else `X-RateLimit-Reset`. The response body (sanitized, ≤500 chars) is appended to the
  message. Other handshake statuses are a non-terminal `RpcError::Handshake { status, detail }`, so the endpoint
  rotates, as Python's `SubstrateRequestException` path does.
- **JSON-RPC.** The error is a policy refusal only when the code is in the set **and** either the case-folded message
  equals a listed message, or `data` is an object with at least one metadata key. Any other `-32004`/`-32005` is an
  ordinary RPC error, as in Python.
- **Close 1008.** `status_code = 1008` and `reason = close reason`.
- **Public surface.** Reads surface a refusal as `ClientError::Rpc(RpcError::Policy(PolicyRefusal))`
  (`ClientError::policy()` returns it, so a neuron can honor `retry_after_duration()`), and the executor as
  `TxError::RpcPolicy(RpcPolicyError { status_code, retry_after, policy, reason })` (`PY/result.py:45-73`; §4.7). The
  `PolicyRefused` session event carries the same data for metrics.

#### 3.2.4 Pools, archive, backend selection

- **Pool presets** come from the extracted `PY/settings.py` tables (`NETWORKS`, `FALLBACK_ENDPOINTS`,
  `ARCHIVE_ENDPOINTS`, [R] A1 §7). An empty fallback list pins the endpoint, as in Python. URLs are sanitized:
  userinfo stripped, control characters removed, length bounded (`rpc.py:136-187`).
- **Legacy JSON-RPC only**: `state_*`, `chain_*`, `author_*`, `account_nextIndex`, `system_*` ([R] C2 §7). There is no
  `chainHead_v1`, `archive_v1` or `transaction_v1`: subxt's `CombinedBackend` fails on the finney WS endpoint with
  `-32601` ([R] feasibility-subxt), and pruned nodes advertise `archive_v1_*` but refuse it ([R] E §2.6-2.8). The custom
  `subnetInfo_*`/`neuronInfo_*`/`swap_*` RPCs lag the runtime APIs and are never used ([R] C2 §0.1).
- **`rpc_methods`** is called once per connection and logged at debug level. Feature `subxt` builds
  `subxt::OnlineClient` from `subxt_rpcs::RpcClient::new(session.clone())` with an explicit `LegacyBackend`. Whether
  subxt 0.51 builds with `default-features = false` without pulling jsonrpsee is checked at M7 [I].

#### 3.2.5 Rate limits (rtao addition) and request volume

Public OTF endpoints allow "roughly 1 request per second per IP" (`UP/docs/guides/local-development.mdx:160` [V]).
Exceeding storage work returns a terminal `-32004`. Three mechanisms keep operators under the limit:

1. **Coalesced reads** (§3.6): a weights preflight that reads about 18 storage items becomes one
   `state_queryStorageAt` plus one or two `state_call`s, where a naive client issues about 20 requests [J-PD §2.5].
2. **Client-side token bucket** `RateLimit { per_second, burst }` in `request_raw`. It is on by default for the
   public presets (`finney`, `test`, `devnet`, `archive`: 1 r/s, burst 1, based on the bounded observations in
   [`m1-endpoint-quotas.md`](../execution/m1-endpoint-quotas.md), §15.2 Q6) and off for
   `local` and custom URLs. Submissions take priority in the queue. Waiting is logged at debug level and exported as a
   metric, never an error.
   This bucket belongs to one session; operators sharing a source IP across sessions or processes must coordinate
   their combined traffic. The observed operating rate is not a provider quota ceiling or a guarantee for expensive
   runtime/storage work, and successful upgrade headers do not disclose such a ceiling.
3. **Page sizes**: 100 keys per `state_getKeysPaged` page on public endpoints (Python's choice,
   `PY/_transport/interface.py:304`), 1,000 on custom endpoints (the core's, `CORE/client.rs:33`).
   `state_queryStorageAt` is chunked at ≤256 keys per call at one pinned hash. Python sends all keys in one call, and
   the results are identical.

The docs tell operators to run their own lite node for production (`UP/docs/guides/local-development.mdx:158-163` [V]).

#### 3.2.6 Telemetry

Metrics: `rtao_rpc_requests_total{method,endpoint,outcome}`, `rtao_rpc_request_seconds{method}`,
`rtao_rpc_reconnects_total{endpoint}`, `rtao_rpc_policy_refusals_total{kind}`, `rtao_rpc_inflight`,
`rtao_rpc_ratelimit_wait_seconds`.

Logging targets are `rtao::rpc` and `rtao::rpc::raw` (frame dumps, `-vvv` in the CLI), mirroring Python's
`bittensor.transport` and `bittensor.transport.raw_websocket` loggers (`rpc.py:48-49` [V]).

### 3.3 Networks and the connect sequence

| Name | Endpoints (extracted from `PY/settings.py`) | Genesis (pinned in `upstream.toml`) |
|---|---|---|
| `finney` (default) | `wss://entrypoint-finney.opentensor.ai:443` + fallbacks; archive pool | `0x2f0555cc76fc2840a25a6ea3b9637146806f1f44b090c175ffde2a7e5ab36c03` |
| `test` | `wss://test.finney.opentensor.ai:443` | `0x8f9cf856bf558a14440e75569c9e58594757048d7b3a84b5d25f6bd978263105` |
| `devnet` | `wss://dev.chain.opentensor.ai:443` | `0x077899043eb684c5277b6814a39161f4ce072b45e782e12c81a521c63fb4f3e5` |
| `archive` | archive pool | finney's |
| `local` | `$BT_CHAIN_ENDPOINT` or `ws://127.0.0.1:9944` | not pinned (regenerated per localnet container, [R] H §1.3) |
| raw `ws(s)://` | as given | not pinned |

(Genesis hashes were checked against the public nodes with `cargo xtask pin --networks`.
This command reads `chain_getBlockHash(0)` twice per extracted preset, refuses changes to an existing
identity, and writes the pinned values and the derived client module. Its `--check` mode is offline.)

**What a `Client` holds.** `Client` holds a `subxt_rpcs::RpcClient` (the type-erased `Arc<dyn RpcClientT>`,
`REG/subxt-rpcs-0.51.1/src/client/rpc_client.rs:15-46`) plus an optional `SessionHandle` (state watch, events,
archive pool) that exists only for a real `Session`. Every RPC goes through the `RpcClient`, including
`author_submitExtrinsic` (`request_raw`) and `author_submitAndWatchExtrinsic` (`subscribe_raw`, unsubscribed with
`author_unwatchExtrinsic`). rtao's typed errors cross the trait as `subxt_rpcs::Error::Client(Box<RpcError |
SubmitError>)` (`src/lib.rs:69-77`) and are recovered with `downcast_ref`, so `FateUnknown`, `PolicyRefusal` and
`StateDiscarded` keep their types. There are two constructors:

- `Client::connect(ClientConfig)` over a real `Session` (the steps below);
- `Client::from_rpc(rpc: RpcClient, cfg: ClientConfig) -> Result<Client, ClientError>` for `FakeChain` (§11.3) and
  custom transports. It runs steps 2-7 over the given client. `FakeChain` scripts `FateUnknown` by returning
  `Error::Client(Box::new(SubmitError::FateUnknown { hash }))`.

`Client::connect(ClientConfig)` performs these steps:

1. Session connect, with a 30 s startup deadline (SN2's bound, [P] C §3.3).
2. `chain_getBlockHash(0)`. For a named network, a genesis mismatch **disables signing** (divergence D-004) and the
   error names both hashes.
3. `system_properties`: the ss58 format must be 42.
4. `chain_getFinalizedHead`, then `state_getRuntimeVersion(at)`. A missing `transactionVersion` is a hard error
   (`PY/_transport/runtime.py:42-54`).
5. Resolve the `RuntimeHandle` (§3.4), run the extension guard, and run `verify_surface` (§3.7).
6. Start `HeadTracker` and the runtime-version watcher. Load token symbols through the 7-day disk cache shared with
   btcli (`rtao_client::config`, §10.2; `PY/config.py:22-27, 415-468`, `PY/client.py:613-633`).
7. Compare `(spec_version, metadata hash)` with the compiled-in `compat.json` range (§12.7). Outside it, warn. Reads
   stay metadata-driven, and signing stays on, guarded per call by the surface check.

### 3.4 Runtime metadata: V15 required, V14 only where Python uses it

The core parses V14 and V15 and rejects V16 (`CORE/runtime/mod.rs:163-199` [V]). For V14 it builds `apis = Vec::new()`
(`:172-181` [V]). On finney today `state_getMetadata` returns **V14** (357,499 bytes, version byte 14) and
`Metadata_metadata_at_version(15)` returns V15 (400,688 bytes after unwrapping) (`SP/meta_getMetadata.json`,
`SP/meta_v15_call.json` [J-CF §7 #10]). The coexistence note's 3.9 ms parse was of that V14 blob [J-CF §5.3]. So:

- **Fetch** (port of `_download_metadata`, `PY/_transport/runtime.py:292-312` [V]):
  1. Call `state_call("Metadata_metadata_at_version", "0x0f000000", at)` and strip the `Option<OpaqueMetadata>`
     wrapper.
  2. Fall back to `state_getMetadata(at)` **only** if the RPC error contains the extracted needle
     `"Exported method Metadata_metadata_at_version is not found"` (`runtime.py:34` [V]), or if the call returns
     `None`. Any other error propagates.
  D's "fall back on any error" (the core's rule) is rejected [J-CF §5.3].
- **`MetadataLevel`** on every `RuntimeHandle`:
  - `V15`: everything.
  - `V14Degraded`: reached only through the needle, which means a runtime that predates V15 (deep archive reads).
    Storage reads, constants and events work. **Signing is refused** (divergence D-010; on a live chain the needle
    never fires). Runtime-API calls go only through the frozen legacy table, a port of `LEGACY_RUNTIME_APIS` and its
    decoders (`PY/_transport/runtime_api.py:95-171` [V]), as Python does. Any other runtime API returns
    `ClientError::RuntimeApiUnavailable`, never a silent mis-encode.
- **V15 runtimes whose API returns `Vec<u8>`** (old pre-encoded results) also route through the legacy table, as in
  `_is_legacy_method` (`runtime_api.py:167`). None does at spec 477 ([R] C2 §0.2); the path exists for archive parity.
- **Memory cache**: an LRU of 4 `RuntimeHandle`s keyed by `(genesis, spec_version, transaction_version)`, plus 512
  `block_hash` entries pointing to that runtime identity. A block's runtime is resolved through its **parent's** runtime version
  (`runtime.py:178-207`). Concurrent fetches of one spec share one future (`futures::future::Shared`).
  A head refresh with the same spec but a new transaction version creates a distinct handle and digest cache without
  changing previously pinned handles. In-flight fetches for different transaction versions remain separate. The disk
  format and filenames remain Python-compatible. Cached metadata receives the observed chain transaction version;
  a transaction-version-only change preserves the existing Python cache bytes and header.
- **Disk cache shared with Python**, byte-compatible (`runtime.py:36-121` [V]):
  - directory `$BITTENSOR_RUNTIME_CACHE_DIR` or `~/.bittensor/runtime-cache/`;
  - file `md1-<genesis hex[..16]>-<spec>.bin`;
  - content `b"STMD" ‖ u32 LE transaction_version ‖ u8 is_v15 ‖ raw MetadataVersioned`;
  - atomic temp file plus rename; the 8 newest `md*.bin` files kept.
  rtao's own V16 blobs (tooling only) are named `rtao-md16-<genesis16>-<spec>.scale`, outside Python's `md*.bin` prune
  glob (`runtime.py:117` [V]) [J-CF graft 5].
- **Upgrades**:
  1. `state_subscribeRuntimeVersion` plus a check on each finalized head detects a new spec.
  2. Fetch and parse it (off the reactor), run the extension guard and `verify_surface`, then swap the head handle.
  3. Unsigned and unsubmitted `CallBytes` from the old spec are refused (§3.8).
- **Extension guard** (connect and every new runtime; [J-CF §2] "fail closed on unknown non-zero-sized extension"):
  - Call `runtime.signature_payload_parts(&probe_params())`, where `probe_params()` is an rtao helper (immortal era,
    zero nonce and tip, zero hashes). If the core refuses (`CORE/codec/extrinsic.rs:89-118`),
    signing is disabled for that runtime with an error naming the extension.
  - Separately compare the identifier list with the generated `extensions::EXPECTED`, which holds the 13 identifiers at
    477. Spec 478 keeps `ChargeTransactionPayment` on the wire (CORRECTIONS #1). A difference is a drift signal, not a
    refusal, because the core decides encodability.
- **V16** is never parsed at run time. `xtask` reads it with `frame-metadata` 23 for `deprecation_info` (generated
  `#[deprecated]`) and `transaction_extensions_by_version` (drift diff, §12.5).

### 3.5 Views, snapshots and pinning

```rust
pub enum BlockRef { Best, Finalized, Number(u64), Hash(H256) }
pub enum HeadPolicy { Best /* Python parity for head reads */, Finalized /* neuron runtimes; but the weight preflight reads at Best, §9.3 step 2 */ }

pub trait View: Send + Sync {
    /// None = head (Client: the node chooses, like Python's block_hash=None); Some = pinned (Snapshot).
    fn pinned(&self) -> Option<&PinnedBlock>;
    /// Client → Snapshot at the current head (HeadPolicy); Snapshot → itself.
    fn pin(&self) -> impl Future<Output = Result<Snapshot, ClientError>> + Send;
    fn runtime(&self) -> impl Future<Output = Result<RuntimeHandle, ClientError>> + Send;
    fn storage_raw(&self, key: &[u8]) -> impl Future<Output = Result<Option<Bytes>, ClientError>> + Send;
    fn storage_raw_many(&self, keys: &[Vec<u8>]) -> impl Future<Output = Result<Vec<Option<Bytes>>, ClientError>> + Send;
    fn keys_paged(&self, prefix: &[u8], count: u32, start: Option<&[u8]>)
        -> impl Future<Output = Result<Vec<Vec<u8>>, ClientError>> + Send;
    fn state_call(&self, method: &str, args: &[u8]) -> impl Future<Output = Result<Bytes, ClientError>> + Send;
}
pub struct PinnedBlock { pub number: u64, pub hash: H256, pub runtime: RuntimeHandle }
pub struct Snapshot { client: Client, block: PinnedBlock }        // implements View
/// Object-safe twin with boxed futures, for the CLI, agents and the read dispatcher.
pub trait DynView: Send + Sync { /* same methods, BoxFuture */ }
impl<V: View> DynView for V { /* blanket */ }

/// The error of every rtao-client API: View, ViewExt, Client, Snapshot, NonceManager. The Python-shaped ChainError
/// stays in rtao-tx (§4.7), and TxError implements From<ClientError>.
#[non_exhaustive]
pub enum ClientError {
    Rpc(RpcError),     // incl. RpcError::Policy(PolicyRefusal), StateDiscarded, ConnectionLost, MaxRetriesExceeded
    Decode { what: Cow<'static, str>, source: rtao_core::CoreError },
    RuntimeApiUnavailable { api: String, method: String },
    ApiVersion { api: String, need: u32, have: u32 },
    SurfaceChanged { descriptor: &'static str, detail: String },
    SigningDisabled { reason: String },
}
impl ClientError {
    pub fn code(&self) -> rtao_catalog::ErrorCode;
    pub fn policy(&self) -> Option<&PolicyRefusal>;   // so a neuron can honor retry_after on -32004/-32005
}
```

- `client.at(BlockRef)` resolves `n → hash` once. It uses the finalized-only hash cache, re-probing the finalized
  height at most every 12 s (Python's reorg safety, `PY/_transport/interface.py:187-230`).
- **Every multi-RPC read pins** before its first RPC. Python's ported bodies do `view = await view.at()`. Four Python
  reads gather at head without pinning (`delegate_take`, `coldkey_swap_announcement`, `hotkey_identities`,
  `miner_collateral`; [R] A2 §1.3); rtao pins them too (divergence D-002). `RecordingView` asserts in tests that every
  RPC of a read used one block hash (§11.3).
- **Constants come from the view's own runtime**, the snapshot's spec, never an RPC. Python reads them from the head
  codec even under a snapshot (`PY/snapshot.py:66-68`). rtao's choice is more correct across upgrades and registered as
  D-007 [J-CF graft 12].

### 3.6 Reading state

```rust
pub trait ViewExt: View {   // provided methods, generic over View (RPITIT; shown as async fn for brevity)
    async fn storage<K: KeyArgs, V: FromStorage>(&self, d: &StorageDescriptor<K, V>, key: K) -> Result<V::Out, ClientError>;
    async fn fetch<F: Fetch>(&self, plan: F) -> Result<F::Output, ClientError>;        // coalesced (below)
    fn storage_iter<'a, K: KeyArgs, V: FromStorage>(&'a self, d: &'a StorageDescriptor<K, V>, prefix: K::Prefix)
        -> impl Stream<Item = Result<(K::Decoded, V::Value), ClientError>> + 'a;       // pinned to one hash
    async fn call<A: IntoCoreValues, R: FromRuntimeApi>(&self, m: &RuntimeApiDescriptor<A, R>, args: A) -> Result<R, ClientError>;
    async fn constant<V: FromConstant>(&self, c: &ConstantDescriptor<V>) -> Result<V, ClientError>;
    async fn events(&self) -> Result<Events, ClientError>;
    async fn timestamp(&self) -> Result<SystemTime, ClientError>;
    async fn block_time(&self) -> Result<Duration, ClientError>;                      // Aura.SlotDuration, never hard-coded
    // dynamic (Python shapes)
    async fn storage_dyn(&self, pallet: &str, item: &str, keys: &[Value]) -> Result<DynValue, ClientError>;
    async fn call_dyn(&self, api: &str, method: &str, args: &[Value]) -> Result<DynValue, ClientError>;
}
```

- **Keys** always come from `Runtime::storage_key(live_entry, &values)`. Hashers and key types are read from the live
  runtime per spec, never from generated code [J-CF concern 11], which keeps archive reads across hasher changes
  correct.
- **Miss semantics** follow the live `StorageInfo`: `Default` decodes `default_bytes`, `Optional` gives `None`
  (`PY/_transport/storage.py:8-27`).
- **Typed values** decode by `DecodeAsType` against the entry's value type id. Primitives and newtypes decode through
  rtao visitors.
- **Map iteration**: `state_getKeysPaged` plus `state_queryStorageAt` per page, all at one hash (Python pins
  `query_map` to one head, `interface.py:347-351`). Free key components come from `decode_storage_key_params` (reversible
  hashers only). Pages above 64 items decode through `batch::decode_map_page` under `offload`.

**Coalesced reads (`ReadPlan`)** address [J-PD concern 6]. `fetch` takes a tuple of up to 16 `StorageQuery` values
(macro-generated impls). It builds every key from the live runtime, issues **one** `state_queryStorageAt(keys, hash)` at
the view's pinned hash (pinning first if needed), chunked at ≤256 keys, and decodes each value with its own miss
semantics:

```rust
let s = client.pin().await?;
let (tempo, reveal, last_epoch, pending, epoch_idx, since_step, cr_on, cr_ver, rate, last_update) = s.fetch((
    st::TEMPO.at((netuid,)), st::REVEAL_PERIOD_EPOCHS.at((netuid,)), st::LAST_EPOCH_BLOCK.at((netuid,)),
    st::PENDING_EPOCH_AT.at((netuid,)), st::SUBNET_EPOCH_INDEX.at((netuid,)), st::BLOCKS_SINCE_LAST_STEP.at((netuid,)),
    st::COMMIT_REVEAL_WEIGHTS_ENABLED.at((netuid,)), st::COMMIT_REVEAL_WEIGHTS_VERSION.at(()),
    st::WEIGHTS_SET_RATE_LIMIT.at((netuid,)), st::LAST_UPDATE.at((NetUidStorageIndex::new(netuid, mech)?,)),
)).await?;   // 1 RPC instead of 10, all at one block
```

Runtime-API calls in the same logical read run concurrently with `tokio::join!` at the same hash. Python sends single
items or same-item batches (`query_batch`). Coalescing heterogeneous items at one hash returns identical values,
because a block's state is immutable, so this is not a divergence. Ported read bodies use `fetch` wherever Python uses
`asyncio.gather` over storage queries at one block.

**Runtime APIs**:

- `state_call("{Api}_{method}", args)`. Arguments are encoded from the V15 `RuntimeApiMethodInfo.inputs` type ids through
  the staged `ops::runtime_api` (mirroring `CORE/client.rs:469-499`). The output is decoded by its `output` type id.
- Methods gated by `api_version` (`SubnetInfoRuntimeApi` v2, `BetaBasketRuntimeApi` up to v5; [R] C2 §1.0) are
  checked against the runtime's advertised versions before the call (`ClientError::ApiVersion { api, need, have }`).

**Events**: `Events` decodes `System.Events` in two ways:

1. *records*: the core `decode_id` gives Python-shaped `{phase, extrinsic_idx, event_index, module_id, event_id,
   attributes, topics}` (`CORE/codec/decode.rs:723-737`). This is what the receipt logic pattern-matches, as Python
   does.
2. *lazy typed*: a scale-decode visitor walks the outer enum once, recording `(phase, pallet name, variant name, field
   byte range)` and skipping field decoding ([P] C §3.5). `RawEvent::decode_as::<E: EventDescriptor>()` then decodes
   only the requested records.

Events are always matched by `(pallet, name)`, never by index. Event indices are not stable: SubtensorModule 149
changed meaning between 461 and 477 ([R] H §0 item 4).

### 3.7 Generated chain descriptors, shape hashes, surface verification (`rtao-runtime`)

**Ownership.** `rtao-runtime` owns `RuntimeHandle(Arc<RuntimeEntry>)`, `RuntimeEntry` (the parsed V15 `Runtime`,
`MetadataLevel`, `SurfaceReport`, the shape cache, the name→index caches and the cached RFC-0078 digest),
`MetadataLevel`, `CallBytes` and `ComposeError`. None of them does I/O. `rtao-client` keeps `RuntimeCache` (V15 fetch,
needle-only V14, the LRU of 4, parent-runtime resolution, the Python-compatible disk cache, the upgrade watcher; §3.4)
and re-exports the five types. This is what lets the generated `Call::compose(&RuntimeHandle)` and
`verify_surface(&RuntimeHandle)` live in `rtao-runtime` (L3), below `rtao-client` (L4), without a crate cycle.

`cargo xtask regen` reads `vendor/metadata/finney/<spec>.v15.scale` through `bittensor_core::runtime::Runtime` and the
staged `ir::MetadataIr`, plus V16 through `frame-metadata` for deprecations. It emits
`crates/rtao-runtime/src/generated/{storage,calls,runtime_apis,constants,events,errors,extensions,selective,shapes}.rs`.
The rules are copied from Python's generator ([R] A2 §5):

- The output is deterministic and sorted. **No `spec_version` appears in headers**; the header hash excludes the
  `System.Version` constant, so "spec bumped, surface unchanged", the common case (CORRECTIONS #2), regenerates zero
  lines (`UP/sdk/python/codegen/emit_python.py:22-30` [J-MA §1]).
- **Scope** (`codegen/scope.toml`): typed call builders for Python's 13 `COVERED_PALLETS` (extracted from
  `UP/sdk/python/codegen/check.py:58-72`) plus `System`, `Timestamp`, `Aura`, `Drand`, `Sudo`; descriptors for every
  storage item, constant, event and runtime API of those pallets plus `TransactionPaymentApi`, `AccountNonceApi`,
  `Metadata`; name tables for all pallets.
- **Descriptors are name-based**: names and type idents only. A storage descriptor records its hashers *for drift
  detection only*. Keys are always built from the live entry (§3.6). This rejects B's baked hashers
  (`StorageEntry::new(…, &[Hasher::Identity], …)`, P-B line 746) [J-CF concern 11].

```rust
// @generated by `cargo xtask regen` — do not edit. Surface hash b3:… (System.Version excluded).
pub mod subtensor_module {
    /// `SubtensorModule.Tempo`: `Map<Identity>(NetUid) -> u16`, Default.
    pub const TEMPO: StorageDescriptor<(NetUid,), u16> = StorageDescriptor::new(
        "SubtensorModule", "Tempo", &["NetUid"], "u16", Shape::exact(hex!("…")));
    pub const OWNER: StorageDescriptor<(AccountId,), DefaultAccount> = StorageDescriptor::new(   // override, §5.1
        "SubtensorModule", "Owner", &["AccountId32"], "AccountId32", Shape::exact(hex!("…")));
    /// `SubtensorModule::add_stake_limit` (call index 88 at generation time; resolved by name at run time).
    pub const ADD_STAKE_LIMIT: CallDescriptor = CallDescriptor::new("SubtensorModule", "add_stake_limit",
        &[("hotkey", "AccountId32"), ("netuid", "NetUid"), ("amount_staked", "TaoBalance"),
          ("limit_price", "TaoBalance"), ("allow_partial", "bool")], Shape::exact(hex!("…")));
}
pub mod subnet_info_runtime_api {
    pub const GET_METAGRAPH: RuntimeApiDescriptor<(NetUid,), Option<MetagraphWire>> = RuntimeApiDescriptor::new(
        "SubnetInfoRuntimeApi", "get_metagraph", &[("netuid", "NetUid")], 2 /* api_version */,
        Shape::projection(hex!("…")));   // MetagraphWire is a by-name model (§8.1); MetagraphView carries its own
                                         // EXACT hash of this output type and checks it separately (§8.2)
}
```

**Type mapping** (`codegen/types.toml`): metadata last-path-segment idents (`CORE/runtime/mod.rs:272`) map to rtao
types:

- `u8…u128`, `bool` map to Rust primitives;
- `NetUid`, `MechId`, `NetUidStorageIndex`, `PerU16`, `Percent`, `Permill`, `Perbill` map to `rtao-primitives` newtypes;
- `TaoBalance` and `AlphaBalance` map to the wire newtypes of the same names (§5.2);
- `AccountId32` maps to `AccountId`;
- `substrate_fixed::FixedU128<Frac>`, `FixedI128<Frac>` and `FixedI64<Frac>` map to `U64F64`, `U96F32`, `I96F32`, `I64F64` and
  `I32F32` by **decoding the typenum `Frac` parameter**; an unknown `Frac` is a codegen error ([P] B §3.5);
- `BoundedVec<T, _>` maps to `Vec<T>` and `Compact<T>` to `T`;
- anything else maps to `DynValue` unless `codegen/models.toml` maps its type path to a hand model.

**Semantic overrides** (`codegen/overrides.toml`) fix what metadata cannot express [J-PD graft 4]:

| Target | Declared | Generated as | Why |
|---|---|---|---|
| `limit_price` of `add_stake_limit` (88), `remove_stake_limit` (89), `swap_stake_limit` (90), `remove_stake_full_limit` (103, `Option`), `add_collateral` (144), `move_stake_limit` (149) | `TaoBalance` | `PriceRao` / `Option<PriceRao>` | a price in rao per alpha, not an amount (`SP/research/_call_index_map.txt:34-36,46,80,84` [V]) |
| `limit` of `add_stake_burn` (132) | `Option<TaoBalance>` | `Option<PriceRao>` | passed as the limit price to `do_add_stake_limit` (`UP/pallets/subtensor/src/staking/recycle_alpha.rs:131-150`); Python's `stake_burn` sends rao per alpha (`PY/intents/governance.py:118-153`) |
| `Metagraph.total_stake` (and selective variant) | `Vec<Compact<TaoBalance>>` | `Vec<StakeWeight>` | alpha-equivalent stake weight `alpha + tao·tao_weight` ([R] A2 §4.1) |
| `Owner`, `SubnetOwner`, `SubnetOwnerHotkey`, `Keys` values | `AccountId32` (Default modifier, 32 zero bytes as the metadata default) | `DefaultAccount` | typed `.get() -> Option<&AccountId>` with zero → `None`; JSON unchanged; applied by rule over the pinned metadata: every `Default` storage entry whose value is `AccountId32` and whose `default` bytes are 32 zeros (exactly these four at spec 477; §5.1, D-001) |
| `name`, `symbol` | `Vec<Compact<u8>>` | `CompactBytesText` | bytes ≥ 64 take two bytes each ([R] C2 §1.1.1) |

**Price-argument gate.** `xtask regen` fails when an argument of a call in a covered pallet is typed `TaoBalance` or
`AlphaBalance` (or an `Option` of either), has a name containing `limit` or `price`, and has neither a price override
nor an explicit `amount` entry in `codegen/overrides.toml`. At spec 477 this matches exactly the seven SubtensorModule
price arguments above, plus Contracts' `storage_deposit_limit`, which gets an `amount` entry.

**Shape hashes** [J-MA G4] are B's structural hash, computed by `xtask` from the pinned metadata and lazily per live
spec:

```text
shape(id) = blake2_256(SCALE(node(id)))
node(id)  = Composite(fs)      => ("C", [(name?, shape(f.ty)) in declaration order])
          | Variant(vs)        => ("V", [(index, name, [(name?, shape(f.ty))]) sorted by index])
          | Sequence(t)        => ("S", shape(t))      | Array(n, t)      => ("A", n, shape(t))
          | Tuple(ts)          => ("T", [shape(t)])    | Compact(t)       => ("K", shape(t))
          | Primitive(p)       => ("P", p)             | BitSequence(o,s) => ("B", bit_order(o), shape(s))
          // a type already on the recursion stack contributes ("R", depth); type paths and docs are excluded
          // the V15 outer enums are opaque: node(outer_enums.call_enum_ty) = ("O", "RuntimeCall"), likewise
          // ("O", "RuntimeEvent") and ("O", "RuntimeError")
```

For bit sequences, `bit_order(o)` is the validated `Lsb0`/`Msb0` semantic tag. These zero-field marker types
have identical structural fields but different SCALE bit meaning, so their tags must differ; unknown
orders fail closed. The M1 shape regressions pin this distinction.

**Outer enums are opaque.** Every call that carries a nested call takes the outer `RuntimeCall` enum as an argument
type: `Utility.batch_all(calls: Vec<141>)` with type 141 = `outer_enums.call_enum_ty` at spec 477, and
`Box<RuntimeCall>` in `Sudo.sudo`, `Multisig.as_multi`/`as_multi_threshold_1` and `Proxy.proxy_announced`
(`meta/finney.json`). A fully structural hash would recurse through every pallet's call enum, so adding a call
anywhere (3 of the last 6 release pairs did) would change the exact hashes of those five wrapper calls. The 12 catalog
intents that wrap them (`batch`, `remove_stake`, `remove_stake_limit`, `move_stake`, `move_swap_stake`, `unstake_all`,
`multisig_approve`, `multisig_execute`, `multisig_threshold_1`, `execute_proxy_announced`, `set_root_claim_threshold`,
`set_subnet_emission_enabled`) would then be refused by every older rtao build. `compose_call` splices nested calls as
bytes (`CORE/codec/encode.rs:528-532`) and each inner call is verified by its own descriptor when it is composed, so a
wrapper's hash covers only its own arguments. A property test asserts that adding a variant to an unrelated pallet's
`Call` enum leaves the exact hashes of `Utility.batch_all`, `Multisig.as_multi`, `Proxy.proxy_announced` and
`Sudo.sudo` unchanged.

Two strengths reconcile B's robustness with C's and D's by-name tolerance [J-PD concern 10]:

- **exact**, for calls (argument names, order and types), positional decoders (`MetagraphView`) and primitive storage
  values. Any change is a mismatch.
- **projection**, for by-name `DecodeAsType` models. Only the fields the model declares are hashed, sorted by name,
  recursively. Appended runtime fields do not change it; a removed, renamed or retyped declared field does.

**`verify_surface(&RuntimeHandle) -> SurfaceReport`** runs at connect and on every runtime upgrade [J-MA G9]. It checks
every descriptor in the generated `USED` set: the calls referenced by intents' `WRAPS`, the descriptors in reads'
declared hits, and model types. Results are classified breaking, behavioural or additive ([R] H §3.3). The policy is
fail-closed **per call**:

- an intent whose wrapped call changed shape returns `TxError::SurfaceChanged { pallet, call, detail }` at `plan`, and
  nothing is signed;
- a typed read whose model no longer matches falls back to the dynamic `Value` path and logs
  `surface_mismatch{descriptor}`; typed-only APIs return `ReadError::SurfaceChanged`;
- the metagraph view falls back to the owned by-name model, then to dynamic (§8.2);
- the gauge `rtao_surface_breaking_items` exposes the count.

### 3.8 Typed call builders and `CallBytes`

Generated from the IR (`calls[].args[].{name, type_ident}`, the content of `PY/_generated/calls.py`):

```rust
pub mod subtensor_module {
    pub fn add_stake_limit(hotkey: AccountId, netuid: NetUid, amount_staked: TaoBalance,
                           limit_price: PriceRao, allow_partial: bool) -> Call { /* record Value */ }
    pub fn commit_timelocked_mechanism_weights(netuid: NetUid, mecid: MechId, commit: Vec<u8>,
                                               reveal_round: u64, commit_reveal_version: u16) -> Call { … }
}
pub struct Call { pub descriptor: &'static CallDescriptor, pub args: Value }
impl Call {
    /// The one encoder: Runtime::compose_call (CORE/codec/encode.rs:532). Verifies the descriptor's shape first.
    pub fn compose(&self, rt: &RuntimeHandle) -> Result<CallBytes, ComposeError>;
}
pub struct CallBytes { bytes: Bytes, spec_version: u32, transaction_version: u32, hash: [u8; 32] /* blake2_256 */ }
```

- **One encoder.** Generated builders produce core `Value`s, so the bytes come from the same encoder the golden vectors
  pin. B's `[p, c] ++ args.encode()` codec path is rejected [J-CF §3.4].
- **Spec stamping** ([P] B §3.8, [J-CF graft 11]): `CallBytes` records `(spec_version, transaction_version)`.
  - Nesting two `CallBytes` from different stamps is an error.
  - `execute` refuses to submit a call composed under a different spec than the head runtime and re-plans once
    (divergence D-013).
  - `submit_signature` refuses if **either** stamp changed since `prepare_call` (D-008; Python checks only
    `spec_version`, `PY/_transport/interface.py:617-623`; [R] A1 §10 item 16).
- **Golden check**: the 10 golden `calls` (`PYT/fixtures/golden.json`; transfers, `add_stake`, `remove_stake`,
  `burned_register`, `set_weights`, `System.remark`, nested `Sudo.sudo`, `Utility.batch`, `Proxy.proxy`) re-encode
  through the typed builders byte for byte (§11.1). The golden file embeds the spec-461 metadata it was recorded with.

### 3.9 Submission pipeline

`Client::submit(call: CallBytes, signer: &dyn Signer, opts: &SubmitOptions) -> Result<ExtrinsicReport, SubmitFailure>`
ports `PY/_transport/interface.py:501-663` and `PY/_substrate.py:625-811` ([R] A1 §4.8-4.10).

`Client::submit_leased(call: CallBytes, signer: &dyn Signer, lease: NonceLease, opts: &SubmitOptions) ->
Result<ExtrinsicReport, SubmitFailure>` takes a lease by value: a lease cannot travel inside the borrowed
`&SubmitOptions`, and `settle` consumes it. Exactly as with `NoncePolicy::Auto`, the lease settles on inclusion (or pool
acceptance under `Wait::Pool`) and drops on every other path. The shield path uses it (§3.11).

```rust
pub struct SubmitOptions {
    pub mortality: Mortality,          // Mortal(Period(128)) — PY/settings.py:22 (extracted)
    pub wait: Wait,                    // Finalization — PY/executor.py:782-788
    pub nonce: NoncePolicy,            // Auto | Explicit(u64)
    pub deadline: Option<Duration>,    // None (Python) — the framework sets one
    pub submit_deadline: Option<Duration>, // preparation, signing and author acknowledgment only; None by default
    pub reconcile: bool,               // false (Python) — the framework sets true (§3.9.6)
    pub metadata_decimals: u8,          // 9; matches the signing device's display configuration
    pub metadata_symbol: String,        // "TAO"; part of the cached RFC-0078 ChainInfo (§3.9.2)
}
pub enum Wait { Pool, Inclusion, Finalization }   // (wait_for_inclusion, wait_for_finalization) = (F,F) | (T,F) | (_,T)
pub enum Mortality { Immortal, Mortal(Period) }   // Period is rounded by era_birth to a power of two in 4..=65536
#[non_exhaustive]
pub enum SubmitFailure {                 // the executor maps it per the §3.9.5 table
    Client(ClientError),                 // metadata, encoding, signing disabled, policy refusal, connection exhausted
    Submit(rtao_rpc::SubmitError),       // incl. FateUnknown { hash }
    Pool(PoolRejection),                 // typed pool rejection (§3.9.5)
    Retracted { hash: H256 },            // watch ended after retraction; reconcile finalized history
    Expired { hash: H256, expiry: u64 }, // mortal validity passed without inclusion
}
```

Both deadlines start at the public submission entry. The shorter limit covers nonce allocation, preparation, signing,
signature attachment and author acknowledgment. After acknowledgment, only `deadline` bounds inclusion, finality and
reconciliation. A synchronous signer that returns after its limit is refused before any later RPC or broadcast. Every
timeout drops an unsettled lease. The executor's overall deadline starts before planning and unlocking, and it passes
only the remaining time to the client; retry and runtime-replan paths share that budget.

#### 3.9.1 Nonces: Python semantics enforced by RAII

Python's `NonceCache` works like this (`PY/_transport/extrinsics.py:55-84` [V]):

- the first request per account asks the node with `account_nextIndex`, which is pool-aware;
- later requests increment locally, so pipelined submissions get consecutive nonces;
- `clear()` runs on **any** failure between signing and inclusion, because "the chain never consumed that nonce"
  (`_failed_submit`, `PY/_substrate.py:753-772` [V]);
- `prepare_call` and the fee probe use `use_cache=False`.

B's graft makes forgetting the clear impossible. `Client` owns exactly one `Arc<NonceManager>` in its shared inner
(`client.nonces()`), and every clone, `Snapshot`, `Executor` and framework service uses it, as Python keeps one
`NonceCache` per connection (`PY/_transport/interface.py:134`). Executors built over clones of one `Client` (the
`WeightScheduler`, the `AxonPublisher`, `cx.executor`, owner tooling) therefore never hand out the same nonce:

```rust
pub struct NonceManager { accounts: DashMap<AccountId, Arc<AccountNonce>> }
struct AccountNonce {
    fetch: tokio::sync::Mutex<()>,             // serializes lease() per account, incl. the first account_nextIndex RPC
                                               // (Python holds one asyncio.Lock across next_for)
    next: parking_lot::Mutex<Option<u64>>,     // next nonce to hand out; a SYNC lock never held across .await,
                                               // so Drop can clear it without awaiting
}
impl NonceManager {
    pub async fn lease(&self, who: &AccountId) -> Result<NonceLease, ClientError>;               // cached or account_nextIndex
    pub async fn lease_pair(&self, who: &AccountId) -> Result<(NonceLease, NonceLease), ClientError>; // n, n+1; next = n+2 (§3.11)
    pub async fn fresh(&self, who: &AccountId) -> Result<u64, ClientError>;                      // use_cache = False
    pub fn pin(&self, who: &AccountId, nonce: u64);                                            // explicit nonce (Python `pin`)
}
#[must_use]
pub struct NonceLease { who: AccountId, nonce: u64, cell: Weak<AccountNonce>, settled: bool }
impl NonceLease {
    pub fn value(&self) -> u64;
    /// The extrinsic reached the state where the chain consumes the nonce: included (any dispatch outcome),
    /// or accepted by the pool when Wait::Pool.
    pub fn settle(mut self) { self.settled = true; }
}
impl Drop for NonceLease {
    fn drop(&mut self) { if !self.settled { /* *cell.next.lock() = None: the next lease re-asks account_nextIndex */ } }
}
```

**`FateUnknown` follows Python, not C** [J-CF concern 8]. The lease drops unsettled, so the cache is cleared, and the
next submission asks the pool-aware `account_nextIndex`:

- if the unknown transaction still sits in the pool, the node returns n+1 and nothing collides;
- if it was dropped, the node returns n and the nonce is reused, as it should be.

C's "keep the nonce reserved until reconciliation" would strand later pipelined nonces when the transaction had in fact
been dropped, and it is rejected. Reconciliation (§3.9.6) is layered *on top* to answer "did it land?", never to hold a
nonce. Hotkeys that ever signed have `sufficients = 1` and never reset their nonce ([R] C1 §7.4), so rtao never assumes
zero.

#### 3.9.2 Era, metadata hash, tip

- **Mortal (default 128).** `TxParams.era = {"period": p, "current": finalized_number}` lets the core compute the phase
  (`CORE/codec/encode.rs:445-485`). `era_block_hash = block_hash(era_birth(p, finalized_number))`
  (`CORE/codec/extrinsic.rs:407-412`). The era is anchored at the **finalized** head, as in Python
  (`PY/_transport/interface.py:485-499` [J-CF §7 #7]); the core's own client uses the best head. If finality lags by
  more than `p/2`, the submission still proceeds, but `rtao_finality_lag_blocks` and a warning make the shortened
  validity visible. Shield periods (4 or 8 blocks) are refused instead when the lag would expire them before inclusion
  (§3.11 step 1).
- **Immortal**: `"00"`, anchored at genesis.
- **Metadata hash.** `None` (mode `Disabled`) unless the signer declares `SignerCaps::METADATA_HASH` (Ledger). In that
  case it is `metadata_digest(v15_bytes, ChainInfo { spec_version, spec_name, base58_prefix, decimals, token_symbol })`
  (`CORE/digest/mod.rs:34-52, 102-107`; [R] A1 §8.1), filled as Python fills it: `spec_name` from the runtime version
  (`state_getRuntimeVersion`, `PY/_transport/contract.py:60`), `base58_prefix` from the client's SS58 format (42), and
  `decimals` and `token_symbol` from the signer's options, which default to 9 and `"TAO"`, the values the runtime
  compiles in (`PY/ledger.py:37-41, 68-81, 115-128`; `UP/runtime/build.rs:10-18`). A constant `spec_name` would sign a
  wrong digest after a runtime rename, which the device and the chain both reject. The digest is computed once per spec
  and `ChainInfo`, off the reactor, and cached on the `RuntimeHandle` (§3.1), so Ledger signing pays only
  `generate_extrinsic_proof`. The mode byte is signed, so it is decided before the payload is built.
- **Tip is always 0.** Priority is flat and tips buy nothing on subtensor
  (`UP/runtime/src/transaction_payment_wrapper.rs:265-303`, [R] C1 §0.5).

#### 3.9.3 Sign, assemble, submit, watch

1. `(extra, additional) = runtime.signature_payload_parts(&params)`;
   `payload = runtime.signature_payload(call, &params)`, which is blake2-256 above 256 bytes.
2. `signer.sign(SigningRequest { call, included_in_extrinsic: &extra, included_in_signed_data: &additional, payload,
   payload_json, context })` returns a `MultiSignature`. A 65-byte reply is split into the variant byte and the
   signature, and the variant must equal the signer's crypto type (`PY/_transport/extrinsics.py:202-239`).
3. `(bytes, hash) = runtime.encode_signed_extrinsic(call, pubkey, &sig, version_byte, &params)` (v4 only).
4. Submit:
   - `Wait::Pool`: `author_submitExtrinsic`. The result is `success = true`, `"Submitted (not waiting)."`, which means
     **pool acceptance**, exactly Python's gotcha ([R] A1 §10 item 1). It is reproduced, not "fixed". rtao adds
     `ExtrinsicResult::inclusion: Inclusion { Unobserved | PoolAccepted | InBlock | Finalized }` so a Rust caller is never
     confused.
     `Unobserved` represents definite rejection or unknown transmitted fate; it does not claim pool acceptance.
   - Otherwise: `author_submitAndWatchExtrinsic`.
     - `usurped` and `invalid` are terminal failures, classified through the extracted `_WATCH_STATUS_RE`
       (`PY/result.py`).
     - `retracted` is **not** terminal: the pool documents that retracted transactions may be included in a later block
       and excludes `Retracted` from its final statuses (polkadot-sdk `cacb431`,
       `substrate/client/transaction-pool/api/src/lib.rs:95-125, 162-172`). Keep watching for a later `inBlock` or
       `finalized`; if the subscription ends first, reconcile (§3.9.6) until the finalized head passes
       `birth + period`.
     - `finalityTimeout(h)` means the extrinsic is in block h but finality did not arrive within 512 blocks
       (`:106-109, 143-147`): resolve the outcome from block h (§3.9.4) and confirm finality by following finalized
       heads.
     - `dropped`: with `reconcile` on, treat it as `FateUnknown` and reconcile; with it off, keep Python's failure.
     - Python raises on all five statuses (`PY/_transport/extrinsics.py:352, 364-369`), so under the default
       `Wait::Finalization` a short fork reports a failure for an extrinsic that then lands, which invites a double
       submission. rtao's handling of `retracted` and `finalityTimeout` is registered as D-024.
     - `inBlock` resolves under `Wait::Inclusion`; `finalized` resolves under `Wait::Finalization`.
     - The subscription is always unsubscribed (`author_unwatchExtrinsic`).
5. The lease settles on inclusion, or on pool acceptance for `Wait::Pool`, and only once an outcome is resolved. Every
   `Err` path drops it unsettled.

#### 3.9.4 Outcome resolution: Python's receipt semantics (staged `ops::receipt`, PR-U2)

1. `chain_getBlock(hash)`. Locate the extrinsic by blake2-256 of each raw extrinsic. If it is absent, return
   `ExtrinsicNotFound`, never a fabricated outcome (`interface.py:691-697`).
2. Read `System.Events` at that block (core `Value` records), filtered by `ApplyExtrinsic(idx)`.
3. Success means `System.ExtrinsicSuccess` and **no** nested failure. Failure means `System.ExtrinsicFailed` (module
   errors resolved by `Runtime::module_error(pallet_index, error[0])` at *that block's* runtime), or a MevShield failure
   event, or an `Err` in any of the six nested-result events of `_NESTED_RESULT_EVENTS`
   (`PY/_transport/utils/receipt.py:8-16` [V]):

   | Event | Field |
   |---|---|
   | `Sudo.Sudid` | `sudo_result` |
   | `Sudo.SudoAsDone` | `sudo_result` |
   | `Proxy.ProxyExecuted` | `result` |
   | `Multisig.MultisigExecuted` | `result` |
   | `Utility.BatchInterrupted` | `error` (a bare `DispatchError`) |
   | `Utility.ItemFailed` | `error` (a bare `DispatchError`) |

   The table is extracted, not hand-copied. The core checks only the three `Proxy`/`Sudo`/`Multisig` ones
   (`CORE/client.rs:1018-1048` [V]), and it misreads `Sudo.Sudid` (code reading, not executed): it looks up field
   `result` and otherwise uses the whole attribute map (`:1025-1037`); named event fields decode to a map keyed by field
   name (`CORE/codec/decode.rs:428-435`), and `variant` is a top-level field lookup (`client.rs:1515-1527`), so an `Err`
   under `sudo_result` is never found and a failed `Sudo.sudo` reports success through the core client. rtao's
   `ops::receipt` reads `sudo_result`; a `FakeChain` receipt test with a failed `Sudid` (M2) and scenario 18's two
   Sudo-wrapped ops (M5) pin it and are PR-U2's evidence.
4. Fee: `TransactionPayment.TransactionFeePaid.actual_fee`, else the sum of `Treasury.Deposit` and `Balances.Deposit`
   (`receipt.py:75-96`).
5. The result is `ExtrinsicReport { hash, inclusion, success, message, block_hash, block_number, extrinsic_index, extrinsic_id:
   "{block}-{idx:04}", fee: Option<TaoBalance>, events: Vec<EventRecord>, error: Option<DispatchErrorInfo> }`. The
   executor lifts it into `ExtrinsicResult` (§4.7).

#### 3.9.5 Transport failures during submit (port of `_failed_submit`, `PY/_substrate.py:753-772` [V])

The lease is already cleared by `Drop`. `Client::submit` reports these as `SubmitFailure` (§3.9), and the executor
maps them as follows:

| Failure | Python | rtao |
|---|---|---|
| `RpcPolicyRejection`, `MaxRetriesExceeded` | raise `RpcPolicyError` / `RpcConnectionError` | `Err(TxError::RpcPolicy(..))` / `Err(TxError::Rpc(Exhausted))` |
| non-RPC exception | raise `ChainError(str(e))` | `Err(TxError::Chain(..))` |
| `SubstrateRequestException` (pool rejection, `-32xxx`, **fate unknown**) | failed `ExtrinsicResult` with `chain_error_from_substrate_request` | `Ok(ExtrinsicResult { success: false, error: Some(classified) })`; fate unknown keeps Python's exact text, "Connection dropped while an extrinsic submission was in flight; …" (`rpc.py:701-708` [V]), and also sets the typed flag `ExtrinsicResult::fate_unknown: Option<H256>` |

Pool rejections parse into a typed `PoolRejection`:

- `Custom(u8)` resolves through the custom-code table, which is extracted from `common/src/transaction_error.rs` with
  `syn` and cross-checked against `PY/error_map.py:488-518` (§4.7);
- standard `InvalidTransaction` texts resolve through the extracted needle table (`error_map.py:524-541`);
- `Future`, `Stale`, `PriorityTooLow`, `AlreadyImported` and `TemporarilyBanned` are recognized by code first and
  needle second.

#### 3.9.6 Retries and reconciliation

- **Retries** happen at executor level only, when `ExecuteOptions::retries > 0` (default 0, Python). The transient set
  is Python's (`_TRANSIENT_SUBSTRINGS = ("priority is too low", "transaction is outdated", "stale")`,
  `PY/executor.py:52-56` [V], extracted). The match runs on the typed `PoolRejection`, with a one-block sleep and a
  fresh nonce per attempt. Retries reuse the reviewed call and resolved signer. A runtime stamp mismatch triggers at
  most one rebuild and policy check before signing the new call. Dispatch failures are never retried. Shielded submissions are never retried either, as in
  Python: `execute` hands `mev_shield_required` intents to `submit_shielded` (`PY/executor.py:812-824`) before its
  retry loop (`:833-845`), and `submit_shielded` has no `retries` parameter (`:897-909`). With `ShieldPeriod` the
  era-caused `Stale` cannot occur (`UP/runtime/src/check_mortality.rs:94-100`), so a carrier `Stale` is `CheckNonce`'s
  outdated nonce (`UP/runtime/src/check_nonce.rs:145-146`). It is surfaced as `PoolRejection::Stale`, both pair leases
  drop, and the caller may resubmit. (D-011, which assumed Python retries carriers, is withdrawn.)
- **Reconciliation** (`SubmitOptions::reconcile`, off by default; on in the framework, §9.3) answers "did a
  `FateUnknown` extrinsic land?" It also runs after a `retracted` status with no later inclusion and, with `reconcile`
  on, after `dropped` (§3.9.3):
  1. Drop the unsettled lease and clear the nonce cache before scanning; reconciliation holds no nonce reservation.
     Scan finalized blocks by number (gap-free, §3.13) from the submission head to `birth + period` for the extrinsic
     hash.
  2. If found, resolve the outcome normally. The lease has already been dropped, so the next allocation still
     re-asks the pool-aware nonce.
  3. If not found once the finalized head passes `birth + period`, report `Expired`: definitely not included.
  Weight commits additionally use the `LastUpdate[NetUidStorageIndex][uid]` effect as a fast path (the SN2 pattern,
  [R] E §4.1 #3). Mortality makes the scan finite. With Python's 128-block default this answers within about 26
  minutes at 12 s blocks, well inside a 360-block tempo, so the framework does not need C's 32-block era (§9.3).

### 3.10 Fee estimation (staged `ops::fee`, PR-U1)

This is a port of `PY/_transport/interface.py:714-731` [J-CF §2].

1. Get the nonce from `account_nextIndex` without touching the cache.
2. Build the extrinsic with an **immortal** era and a zeroed 64-byte signature.
3. Call `state_call("TransactionPaymentApi_query_info", xt ‖ u32_le(len))`.
4. Decode `RuntimeDispatchInfo` by the runtime-API output type id, as u64 balances ([R] E §2.8 warns against u128
   helpers).
5. Return `partial_fee` and `weight { ref_time, proof_size }`. The weight feeds multisig `max_weight`.

**Fee payer**:

- the signer pays by default;
- for a hotkey-signed leaf in the coldkey-pays list, the owning coldkey pays. The list is extracted from the runtime's
  `call_filter_group!(ColdkeyPaysFeeCalls, …)` (`UP/runtime/src/fee_filters.rs:15-34` [J-CF §7 #24]), and CI asserts
  that Python's mirror `PY/fee_filters.py:12-32` equals it;
- proxied calls under `RealPaysFee` charge the real account (the 478 wrapper, [R] C1 §7.1).

Failure handling keeps Python's asymmetry ([R] A1 §10 item 4): a missing estimate is a **warning** in `plan`, and a
hard `PolicyError` in `submit_shielded` when `max_fee` is set.

The probe is public, as on Python's `Client`: `Client::estimate_weight(call, address) -> Result<Weight, ClientError>`
returns step 5's weight (`PY/client.py:554-572`), and `Client::estimate_shielded_carrier_fee(fee_payer) ->
Result<Balance<Tao>, ClientError>` prices the 8,192-byte carrier of §3.11 step 7 (`PY/client.py:464-466`).
`Client::decode_scale(type_name, bytes) -> Result<DynValue, ClientError>` decodes by a metadata type name
(`PY/client.py:264-271`; `multisig pending` decodes `"Call"` with it, `PY/cli/multisig_helpers.py:897`).

### 3.11 MEV-shielded submission

This ports `_submit_encrypted_call` and `_resolve_shielded_inner` (`PY/executor.py:897-1214`, [R] A1 §6.5), with the
chain's limits as types [J-CF graft 11]:

```rust
pub enum ShieldPeriod { P4, P8 }   // the ONLY era type the shield path accepts; chain max is 8
                                   // (UP/runtime/src/check_mortality.rs:19,94-100 [V]); Python default 8 (PY/settings.py:29)
pub struct ShieldCiphertext(Vec<u8>);  // constructor rejects > 8,192 bytes (_MAX_SHIELDED_CIPHERTEXT_BYTES, PY/executor.py:61, extracted)
pub struct ShieldOptions { pub period: ShieldPeriod /* P8 */, pub wait: Wait /* Inclusion (executor.py:905-907) */,
                           pub max_fee: Option<Balance<Tao>> }
```

1. Read `MevShield.NextKey` at the best head B. If it is absent, retry twice, one block apart
   (`executor.py:1027-1034`).

   **Era check (D-025).** Read the finalized head F too. The carrier and the inner extrinsic are applied together in
   block B + 2, by the author whose key `NextKey` holds (`UP/pallets/shield/README.md:15, 34`;
   `UP/pallets/shield/src/lib.rs:324, 616-620`; polkadot-sdk
   `substrate/client/basic-authorship/src/basic_authorship.rs:464-545`). An era of period p anchored at F is valid
   only below F + p (`substrate/primitives/runtime/src/generic/era.rs:86-99`; frame_system `check_mortality.rs:64-72`).
   If B + 2 ≥ F + p (that is, B − F ≥ p − 2), refuse with `TxError::ShieldEraExpired { period, finality_lag: B − F }`:
   at B − F = p − 2 the pool accepts the carrier but it can never be included, and above that the pool rejects it.
   `P8` therefore tolerates a lag of at most 5 and `P4` at most 1. Finney's measured lag is 2-3 blocks, so `P4` is
   refused in normal operation, and the CLI never offers it. Python submits and fails. A raw shielded call
   (`ExecuteOptions::shielded`, §4.6) accepts only periods 4 and 8.
2. `client.nonces().lease_pair(signer)` returns n (carrier) and n+1 (inner). It reserves both under the account's fetch
   lock and leaves `next = n + 2`; rtao never calls `pin` in the shield path.
   - The carrier lease settles on carrier inclusion, and the inner lease on inner inclusion (step 6).
   - Under `Wait::Pool`, both settle on carrier pool acceptance, because `account_nextIndex` cannot see the encrypted
     inner's nonce.
   - An inner lease dropped unsettled (the carrier is included but the inner never lands) clears the cache, so the
     next lease re-asks `account_nextIndex`.
   - Python takes n from its cache and pins n+1, then n (`PY/executor.py:1037-1052`;
     `PY/_transport/interface.py:519-523`; `PY/_transport/extrinsics.py:67-81`). Its cache is left at n, so the next
     submission from that process reuses n+1 and is rejected as `Stale` once the inner has landed
     (`UP/runtime/src/check_nonce.rs:145-146`). This is D-026.
3. Sign the inner call at `inner.value()` (n+1) with `ShieldPeriod`.
4. `ShieldCiphertext::seal(&next_key, &inner)` wraps `mlkem::seal(pk, inner, include_key_hash = true)`
   (`CORE/mlkem/mod.rs:53`). It is bounded before any carrier signing.
5. Compose `MevShield.submit_encrypted(ciphertext)`, sign it at n with `ShieldPeriod`, and submit it with
   `client.submit_leased(.., carrier, ..)` (§3.9). The inner extrinsic is sealed into the carrier and is never
   submitted on its own.
6. Resolve the inner extrinsic. Scan blocks from the carrier's inclusion to `included_at + period` for the inner hash,
   with one shared budget of `4 * (period + 1)` missing-height polls
   (`_SHIELDED_POLL_ATTEMPTS_PER_BLOCK`, extracted), as Python does. Under `Wait::Finalization`,
   re-resolve after finality (`_SHIELDED_FINALITY_RPC_RETRIES = 4`) and rescan on a reorg. If the inner extrinsic never
   lands, the result becomes a failure with Python's message. The inner lease settles when the inner extrinsic is
   found and drops otherwise.
7. Preflight:
   - price `submit_encrypted` with an 8,192-byte ciphertext;
   - when the intent reports `required_free`, block execution if free TAO covers the reserve but not reserve plus
     carrier (`executor.py:955-979`);
   - carriers cannot pay fees in alpha ([R] A1 §10 item 13).
8. Routing: `execute` sends `mev_shield_required` intents (`burned_register`, `add_collateral`) through this path
   before any plan (`executor.py:812-824`). `mev_shield_default` is honored only by the CLI, with precedence
   required → flag → config → intent default (`PY/cli/context.py:260-292`). Shielding is downgraded when free TAO cannot
   cover the carrier fee (`context.py:1047-1080`).

### 3.12 Sudo, proxy, multisig, batch composition

```rust
pub mod compose {
    pub fn sudo(rt: &RuntimeHandle, inner: CallBytes) -> Result<CallBytes, ComposeError>;                // never double-wrapped
    pub fn proxy(rt: &RuntimeHandle, real: AccountId, force: Option<ProxyType>, inner: CallBytes) -> Result<CallBytes, ComposeError>;
    pub fn batch_all(rt: &RuntimeHandle, calls: Vec<CallBytes>) -> Result<CallBytes, ComposeError>;      // nesting refused
    pub async fn as_multi<V: View>(v: &V, signer: AccountId, signatories: &[AccountId], threshold: u16,
                                   inner: CallBytes, est: &FeeEstimate) -> Result<CallBytes, ComposeError>; // full call only
    pub async fn approve_as_multi<V: View>(v: &V, signer: AccountId, signatories: &[AccountId], threshold: u16,
                                           call_hash: H256, timepoint: Timepoint, est: &FeeEstimate)
        -> Result<CallBytes, ComposeError>;                                // hash only; used by the multisig_approve port
    pub fn as_multi_threshold_1(rt: &RuntimeHandle, others: &[AccountId], inner: CallBytes) -> Result<CallBytes, ComposeError>;
}
```

- **Order is fixed** (`nest_origin_wrappers`, `PY/executor.py:99-121, 175-219`):
  1. pinned prior-round call data, if present;
  2. the semantic build against the origin view (or the `proxy_for` view);
  3. `Sudo.sudo` when `origin == Root`;
  4. `Proxy.proxy(real, force_proxy_type, call)`;
  5. the intent's `wrap_call` adapter (saved multisig).
  Every wrapper is a core `compose_call` with `Value::Bytes(inner)`, spliced verbatim (`CORE/codec/encode.rs:528-585`).
- **`ProxyType`** is generated from the metadata enum. CI asserts that its variant names equal the extracted
  `PY/intents/proxy.py` `PROXY_TYPES` (`:21-41`) [J-CF graft 1]. A string the chain does not know is a parse error.
- **Multisig**:
  - construction refuses fewer than two distinct signatories and thresholds outside `1..=signatories.len()`
    and the authenticated runtime `Multisig.MaxSignatories` cap before returning a funding address (D-039);
    `Executor::multisig` pins a runtime asynchronously, while `Multisig::new` requires that runtime explicitly.
    Approval rechecks the cap before funding probes, help or key unlock; origin composers enforce it too;
  - account and sorted signatories come from `multisig_account_id` (`CORE/codec/extrinsic.rs:419-437`);
    `other_signatories` is the sorted set minus the signer;
  - the existing timepoint comes from `Multisig.Multisigs[account, call_hash]`;
  - `max_weight` comes from the fee probe's `weight`;
  - the opening approval uses `as_multi` with the full call and `maybe_timepoint = None`. Intermediate approvals sent
    through `multisig_approve` use `approve_as_multi` with the call hash and the opening timepoint
    (`PY/intents/multisig.py:438-462`). `multisig_execute` and the saved-multisig adapter always use `as_multi` with
    the full call and the pending timepoint (`PY/intents/multisig.py:227-245, 317-330`; `PY/multisig.py:249-269`).
    pallet_multisig dispatches only when the call is supplied and approvals reach the threshold; a hash-only approval
    at the threshold only records `MultisigApproval` (polkadot-sdk `substrate/frame/multisig/src/lib.rs:442,
    671-715`). Python's `multisig_approve` still sends a hash-only approval at the threshold, and the chain accepts it
    (any signatory can execute later), so `plan` keeps the call and adds the warning "this approval cannot execute
    the call; send multisig_execute" (D-034) instead of refusing;
  - exact prior-round bytes are pinned for timelock-bearing calls, because rebuilds are not byte-stable
    (`PY/intents/multisig.py:293-299`);
  - the funds preflight is `DepositBase + threshold × DepositFactor` plus `_FEE_ALLOWANCE_RAO = 5_000_000`, extracted
    (`PY/multisig.py:121-162`). It lives in `rtao_tx::multisig`, because the raw-call account object `Multisig::approve`
    (§4.6) uses it; the M5 multisig intents reuse it.
- **Batch**: `Utility.batch_all` only, nesting refused. Spend aggregates (any unbounded child makes the batch
  unbounded), netuids are the union, and extras are namespaced as in `PY/intents/batch.py:54-164`.

### 3.13 Block streams, waiting helpers, completion followers

- `HeadTracker` keeps `watch` channels of the best and finalized heads. On `SubscriptionLost` it resubscribes and
  **backfills** missed heights by number with `chain_getBlockHash(n)`.
  - `client.blocks(Finality::{Best, Finalized})` therefore yields every height exactly once (finalized) or reports
    reorgs (best). Python's stream simply dies on reconnect (`rpc.py:582-602`). This is divergence D-012.
  - `client.blocks_strict(..)` keeps Python's semantics.
- `client.blocks(..)` yields `BlockHeader { number, parent_hash, raw }` (`PY/client.py:74-80`), and
  `client.block_info(n)` returns `BlockInfo { number, hash, timestamp, explorer_url, header, extrinsics }`
  (`:83-92, 349`; `multisig pending` recovers call data through it, `PY/cli/multisig_helpers.py:945-975`).
- `wait_for_block(n)`, `wait_for_timestamp(t)` (sleeping through long gaps on chain time) and `wait_for_epoch(netuid)`
  port `PY/client.py:376-444`. `wait_for_epoch` returns `EpochEvent { netuid, block, previous }` (`:109-115`). It
  watches `LastEpochBlock[netuid]` advance, the only trustworthy signal, because the schedule is stateful ([R] C2 §0
  item 6).
- `wait_for_event::<E>(filter, until)` is the typed lazy-event follower.
- **`register_subnet` completion** ports `_complete_subnet_registration` (`PY/executor.py:333-568`):
  1. scan the receipt for `NetworkAdded` vs `NetworkRegistrationQueued`;
  2. on the queued path, rescan the inclusion block (on-idle events are not in the receipt), then every height up to
     the head, then follow heads;
  3. accept only a `NetworkAdded` whose `SubnetOwner`/`SubnetOwnerHotkey` match and which is no longer in
     `NetworkRegistrationQueue`.
  Progress is reported as `Queued`, `Waiting`, `Registered`. The default timeout is `None`, as in Python ([R] A1 §10
  item 14).

### 3.14 Offline and external signing

`Client::prepare_call(call, signer, &SubmitOptions) -> UnsignedExtrinsic` uses a fresh nonce under `NoncePolicy::Auto`
without reserving cached state (`interface.py:589-590`); `Explicit(nonce)` selects that nonce without a lease.
Preparation uses only the signer's public identity, crypto type and capabilities. `submit_signature(unsigned, signature, &SubmitOptions)` refuses on a spec **or** transaction
version change (D-008).

Prepared transaction fields are private with read-only getters. Call bytes, nonce, mortality, metadata hash, payload,
JSON view and context therefore describe one immutable transaction. Signature attachment validates its variant and
both runtime stamps before author submission; cancellation drops the explicit lease used by external submission.

`UnsignedExtrinsic` serializes to exactly Python's `to_dict()`/`from_dict()` JSON (`PY/_transport/contract.py:67-134`).
That includes `SignerPayloadJSON` with numeric fields as big-endian hex at SCALE width (`extrinsics.py:87-132`). A call
prepared by Python can therefore be signed by an rtao signer and submitted by either side, and the interop lane tests
it (§11.6).

### 3.15 Python transport parity checklist

Every row is a test in `crates/rtao-rpc/tests/` or `crates/rtao-client/tests/`, running against `FakeNode` (a real
loopback WebSocket server, §11.3). Together they port `PYT/unit/test_rpc.py` test by test, with `// pyt:` provenance
comments.

| Python behavior | rtao test |
|---|---|
| unanswered idempotent frames re-sent before the gate opens; sent submits fail "fate unknown"; a submit queued while disconnected is sent normally | `reconnect_resends_reads_not_sent_submits` |
| subscriptions receive an error on reconnect | `subscription_lost_on_reconnect` |
| rotation after 5 consecutive failures; give up after 5 × endpoints; backoff schedule | `pool_rotation_backoff_and_exhaustion` (paused clock) |
| handshake 403/429 with `X-RateLimit-*`, `Retry-After`, `X-RateLimit-Reset` fallback, body detail | `handshake_policy_refusal_headers` |
| `-32004` message match; `-32005` with metadata keys; other `-32004` stays ordinary | `jsonrpc_policy_classification` |
| close 1008 is terminal with the reason | `close_1008_is_terminal` |
| `State already discarded` goes to the archive pool once | `state_discarded_uses_archive` |
| finalized-only hash cache across a reorg | `block_hash_cache_finalized_only` |
| runtime upgrade picked up within about a block | `runtime_upgrade_switches_codec` (also localnet `set_code`, §11.4) |
| nonce: increment locally, clear on any failure, explicit pin, fresh for prepare | `nonce_lease_semantics` (+ proptest, §11.7) |
| mortal era anchored at the finalized head | `era_anchor_finalized` |
| `Submitted (not waiting).` with `success = true` | `no_wait_reports_pool_acceptance` |
| V15 fetch; V14 only on the needle | `metadata_v15_needle_fallback_only` |
| disk cache format read and written both ways | `runtime_disk_cache_python_format` (+ interop, §11.6) |
| a submit whose caller was cancelled before transmission is never sent | `cancelled_unsent_submit_is_never_transmitted` (drop the socket, cancel the submit, reconnect, assert no `author_submitExtrinsic` frame) |
| a response above 16 MiB in one frame is accepted (Python `max_size=2**32`) | `large_single_frame_response` (a 20 MiB single-frame result) |
| `wss://` with an explicit ring config, also when a second rustls provider is enabled | `wss_connect_with_extra_roots` (self-signed `FakeNode` certificate through `SessionConfig::extra_roots`; §2.7) |
| dropping the last `Client` clone stops the background tasks | `client_drop_closes_socket` (`FakeNode` sees the close within 1 s) |
| `retracted` then `inBlock`/`finalized` is a success; `finalityTimeout(h)` resolves from block h (D-024) | `retracted_then_finalized_is_success`, `finality_timeout_resolves_from_block` (`FakeNode` status scripts `ready → inBlock(A) → retracted(A) → inBlock(B) → finalized(B)`, with one settled lease, and `finalityTimeout(A)`) |

---

## 4. Intents and reads parity

Upstream surface at `parity_tag` [V]:

- **`CAT/intents.json`**: 83 intents. Signer: 75 coldkey, 8 hotkey. Origin: 74 signed, 7 subnet_owner, 2 root. Together
  they wrap 116 distinct `(pallet, call)` pairs (139 wrap entries).
- **`CAT/reads.json`**: 102 reads in 14 categories. 81 have a single chain hit, 10 have none, 11 have several
  [J-MA §3.4].
- **`CAT/errors.json`**: 18 semantic codes and 390 classified chain error names.
- **e2e manifest**: `UP/sdk/bittensor-core/tests/e2e-manifest.json` has 112 entries, 73 `intent_*` and 39 `test_*`.
  Report B's "72 + 40" was wrong, and C repeated it [J-CF concern 14].

### 4.1 Where each piece comes from

| Needed | Source at `parity_tag` | How rtao gets it |
|---|---|---|
| op name, summary, description, signer, origin, verify read, wraps, JSON Schema, docs URL, CLI string, `python_class`, chain `sources` | `CAT/intents.json` | vendored, then generated into `rtao_catalog::INTENTS` and intent interfaces (§4.2) |
| field **defaults** (`slippage_protection = True`, `rate_tolerance = 0.05`, `mechid = 0`, `commit_reveal_version = 4`, …) | dataclass `field(default=…)`; **absent from the catalog**, where 0 properties carry `default` [J-MA §3.4] | Rust-native extractor (§4.9) → `vendor/upstream/extracted/intent_fields.json`; upstream PR-U11 adds them to the catalog |
| `mev_shield_default`, `mev_shield_required`, `all_amount_fields` | class vars (`PY/intents/base.py:160-174`) | extractor → `intent_classvars.json`; PR-U11 |
| `build`, `preflight`, `effects`, `warnings`, `blocks`, `spend`, `touches_netuids`, `"all"` resolution, adapters | method bodies in `PY/intents/*.py` (8,335 lines) | **ported by hand** (46 `build()` bodies generated, §4.3); checked by the oracle (§11.5) |
| read name, params and docs, category, namespace, chain `hits` | `CAT/reads.json` | generated `ReadSpec`, `ReadName`, dispatch, namespace accessors (named from `category`, §4.8) |
| read parameter defaults and requiredness | the `@read` signatures in `PY/reads/*.py` (`neurons` `lite=True`, `neurons.py:41`; `weights`/`bonds`/`timelocked_weight_commits` `mechid=0`, `weights.py:32, 59, 89`; `miner_collateral` `coldkey_ss58=None`, `collateral.py:67-68`), `scalar_read`'s `netuid=None` (`PY/reads/base.py:156-166`), and the CLI's own-key rule (`PY/cli/query.py:30, 39-58`); the catalog's `params` carry only a JSON type | extractor → `vendor/upstream/extracted/read_params.json` `{read, param, default (JSON literal or null), required, own_key}`, using `_is_required`: a param with no default, or with default `None`, is required, except the own-key `hotkey_ss58`/`coldkey_ss58` |
| static read hit sets | each read function in `PY/reads/*.py` plus the module-local helpers it calls, transitively, and module-level `Item(...)`/`Method(...)`/`Constant(...)` literals | helper-aware extractor → `vendor/upstream/extracted/read_hits.json`; items reached outside the module in `parity/read_hits_extra.toml` (§4.8) |
| read return shapes | dataclasses and dicts in `PY/reads/*.py` | hand-written `Serialize` models; checked against the registry (§8.1) and the read oracle |
| 18 codes, remediation, explanations; 390 names → code, description | `CAT/errors.json` | generated `ErrorCode`, `CHAIN_ERRORS` |
| custom tx codes 0-27 and 255, invalid-tx needles, dispatch errors, substring fallback, help overrides, watch-status regex | `PY/error_map.py:488,524,555`; `PY/result.py:27-36,78,116,291,308,475` | extractor; custom codes also from `common/src/transaction_error.rs` with `syn`; CI asserts the two agree |
| hyperparameter table and owner-setter map (34 names over 33 setters, [J-CF §7 #16]) | `PY/hyperparams.py:110-408` (`HYPERPARAMS`, `Hyperparam(…)` calls), `:416-461` (`STORAGE_ITEMS`, each resolved to its storage value ident), `:32-37, 83, 467-471, 626-657` (ratio, ident and codec tables), `PY/intents/hyperparameters.py:31-66` | extractor (literal-argument constructor calls and the resolved forms of §4.9 layer 4) → `rtao_catalog::hyperparams::TABLE` (§5.5) |
| coldkey-pays-fee calls | runtime `fee_filters.rs:15-34` (`call_filter_group!`); Python mirror `PY/fee_filters.py:12-32` | `syn` over the **runtime** source (authoritative); CI asserts the Python mirror is equal |
| `PROXY_TYPES` | `PY/intents/proxy.py:21-41` | extractor; CI asserts equality with the metadata `ProxyType` enum |
| `SelectiveMetagraphIndex` 0..=76 | `pallets/subtensor/src/rpc_info/metagraph.rs` (`from_index` arms; the order equals the struct's field order, 77/77 [J-MA §3.2]) | `syn` extractor |
| network table, era defaults, explorer and docs templates, retry substrings, shield bounds, multisig allowance, RPC policy constants | `PY/settings.py`, `PY/executor.py:45-66`, `PY/multisig.py`, `PY/_transport/rpc.py:52-66` | extractor → `rtao_catalog::py::*` and `rtao-rpc/src/generated/py_rpc.rs`. **No Python constant is hand-copied** |
| environment variables | every `os.getenv`, `os.environ.get`, `os.environ[...]` and Typer `envvar=` in `PY/**` (28 names plus the computed `BT_PW_*` family) | extractor → `vendor/upstream/extracted/env_vars.json`, mapped in `parity/env.toml` (§4.9) |
| covered pallets and raw-only calls | `UP/sdk/python/codegen/check.py:58-72, 74-300` | extractor → coverage gate (§4.9) |
| samples and fake-substrate defaults | `PYT/harness/samples.py` (`INTENT_SAMPLES`, `READ_SAMPLES`), `PYT/harness/fake_substrate.py:34-77` (`DEFAULT_STORAGE/CONSTANTS/RUNTIME`) | extractor (dev keys resolved to ss58 through a fixed table) → oracle inputs and `FakeChain` defaults |

### 4.2 Intent representation

Every intent has a **generated interface** and a **hand-written behavior** impl. The trait split follows B, so roles and
outputs are types, and D, so interfaces are generated. Interfaces come from `xtask regen`
(`crates/rtao-intents/src/generated/interfaces.rs`), built from the catalog, the extracted defaults and class vars,
`codegen/intent_types.toml` (field type rules) and `codegen/intent_callmap.toml`:

```rust
// @generated — do not edit
/// Stake TAO to a hotkey on a subnet. … (catalog `description`, verbatim)
#[doc(alias = "bittensor.intents.staking.AddStake")]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]   // as from_args (PY/intents/base.py:366-372); AnyIntent::from_tool strips a top-level "op" first (§4.5)
pub struct AddStake {
    pub hotkey_ss58: AccountId,                                // "*_ss58" → AccountId (serde: ss58 string)
    pub netuid: NetUid,
    pub amount_tao: Amount,                                    // anyOf[number, decimal string, "all"] + all_amount_fields; resolved per §5.2
    #[serde(default = "defaults::add_stake::slippage_protection")] pub slippage_protection: bool,  // extracted: True
    #[serde(default = "defaults::add_stake::rate_tolerance")]      pub rate_tolerance: Ratio,      // extracted: 0.05
}
impl IntentInterface for AddStake {
    const SPEC: &'static IntentSpec = &rtao_catalog::INTENTS[IntentId::AddStake as usize];
    type Role = Coldkey;                                       // catalog `signer`
    type Origin = Signed;                                      // catalog `origin`
    type Output = ();                                          // codegen/intent_types.toml overrides where Python returns data
    const WRAPS: &'static [&'static CallDescriptor] = &[      // typed references: a removed or renamed call breaks the build
        &rtao_runtime::calls::subtensor_module::ADD_STAKE, &rtao_runtime::calls::subtensor_module::ADD_STAKE_LIMIT,
    ];
}
```

Field type rules (`codegen/intent_types.toml`, D §4.2):

- `*_ss58` → `AccountId`; `*_ss58s` → `Vec<AccountId>`;
- `netuid`, `origin_netuid`, `dest_netuid` → `NetUid`; `mechid` → `MechId`; `uids` → `Vec<Uid>`;
- money (`anyOf` with the decimal pattern) → `Amount`, resolved per §5.2: the unit from the wrapped call's argument
  ident, the expected netuid from the sibling field that `codegen/intent_types.toml` names. A money property without a
  `_tao`/`_alpha` suffix (today `swap_basket.amount`, `swap_basket.min_amount_out` and
  `set_root_claim_threshold.threshold`) needs an explicit `intent_types.toml` entry naming its unit source; otherwise
  the generator fails;
- `number` → `Ratio` (exact decimal);
- other integers → the narrowest type of the wrapped call's argument ident;
- explicit overrides per field. `SetWeights.weights` is the untagged `WeightsArg { Map, List }`, Python's
  list-or-mapping input (`PY/intents/weights.py` `_as_pairs`).

`WeightsArg`, `Amount` and `Ratio` implement `Deserialize` through `serde_json::Value` (numbers read from
`Number::as_str()`), not through serde's derived untagged visitor: with the `arbitrary_precision` feature the core
forces on, a derived untagged enum fails on non-integer numbers parsed from text (§2.7).

The behavior trait mirrors the `Intent` ABC hook for hook (`PY/intents/base.py:138-399`; P-A §4.1). Each default
equals Python's default:

```rust
pub trait IntentInterface: Send + Sync + 'static {        // generated impls (above); Send + Sync, §2.5
    const SPEC: &'static IntentSpec;
    type Role: Role; type Origin; type Output: IntentOutput;
    const WRAPS: &'static [&'static CallDescriptor];
    fn to_json(&self) -> serde_json::Value;                  // {"op": …, fields…}, money as exact decimal strings (§4.5)
}
pub trait Intent: IntentInterface {
    fn summary(&self) -> String;
    fn build<V: View>(&self, cx: &BuildCx<'_, V>) -> impl Future<Output = Result<Built, IntentError>> + Send;
    fn effects<V: View>(&self, cx: &PreflightCx<'_, V>) -> impl Future<Output = Result<Vec<String>, IntentError>> + Send
        { async { Ok(vec![self.summary()]) } }
    fn warnings<V: View>(&self, cx: &PreflightCx<'_, V>) -> impl Future<Output = Result<Vec<String>, IntentError>> + Send
        { async { Ok(vec![]) } }
    fn blocks<V: View>(&self, cx: &PreflightCx<'_, V>) -> impl Future<Output = Result<Vec<String>, IntentError>> + Send
        { async { Ok(vec![]) } }
    /// Default composes effects/warnings/blocks (base.py:204-226).
    fn preflight<V: View>(&self, cx: &PreflightCx<'_, V>) -> impl Future<Output = Result<IntentPreflight, IntentError>> + Send
        { default_preflight(self, cx) }
    fn spend(&self) -> Spend { Spend::None }                                   // base.py:300-309
    fn touches_netuids(&self) -> SmallVec<[NetUid; 2]> { generated_touches(self) } // fields named *netuid (base.py:311-318)
    fn affects_all_subnets(&self) -> bool { false }
    fn wrap_call<V: View>(&self, cx: &BuildCx<'_, V>, call: CallBytes)
        -> impl Future<Output = Result<CallBytes, IntentError>> + Send { async { Ok(call) } }
    fn origin_view<'a>(&'a self, keys: &'a dyn KeyView) -> OriginView<'a> { OriginView::Keys(keys) } // base.py:248-298
    fn validate(&self) -> Result<(), IntentError> { Ok(()) }                   // ported __post_init__ checks
    /// Python's semantic_intent(): policy and composition run on it. Emitted by xtask as `self` for every
    /// generated intent; the multisig adapters return their inner intent.
    fn semantic_intent(&self) -> &dyn DynIntent;
}
pub struct BuildCx<'a, V> { pub view: &'a V /* pinned */, pub origin: OriginView<'a>, pub runtime: &'a RuntimeHandle,
                            pub clock: &'a dyn Clock }
pub struct Built { pub call: CallBytes, pub extras: serde_json::Map<String, serde_json::Value> } // BuiltCall (base.py:414-422)
pub struct IntentPreflight { pub effects: Vec<String>, pub warnings: Vec<String>, pub blocks: Vec<String>,
    pub required_free: Option<Balance<Tao>>, pub available_free: Option<Balance<Tao>>, pub estimated_fee: Option<Balance<Tao>>,
    pub facts: Vec<(String, String)>, pub facts_title: FactsTitle }           // base.py:109-134
```

- **Roles are types** [J-CF graft 14, J-PD graft 2]. `Executor::execute<I: Intent, K: KeySource<I::Role>>`.
  `HotkeyFile` implements only `KeySource<Hotkey>`, so `executor.execute(&Transfer { .. }, &hotkey)` does not compile
  in a neuron binary. This is C's "neuron runtimes never load a coldkey" as a type error. Bare signers
  (`KeypairSigner`, `LedgerSigner`, …) implement `KeySource` for both roles, because in Python a bare signer signs
  either role (`PY/intents/base.py:258-298`).
- **The `dyn` path keeps Python's runtime check.** `DynIntent` (object-safe, JSON in and out) and
  `DynKeySource::signer_for(SignerRole)` serve the CLI, agents and `execute_tool`. A missing role yields
  `SignerError::RoleUnavailable` with Python's message (`PY/signing.py:227-263`).
  `Intent::signer_role` supplies the instance role, forwarded by `DynIntent::signer_role_dyn`;
  it defaults to the catalog signer. Batch inherits its children's unanimous role. Its generated
  associated role remains the catalog's `Coldkey`; a hotkey Batch uses the dynamic executor API.
  Typed executor entry points refuse a differing instance role before public key access or private
  acquisition. CLI wallet selection, public payer lookup and lazy acquisition use the instance role.
- **Typed outputs without changing JSON.** `ExtrinsicResult<O>` keeps Python's `data` map verbatim for JSON and adds
  `output: O`, which `IntentOutput::from_data(&data)` decodes from the same map. The typed outputs are:
  - `SetWeights` and `CommitWeights`: `WeightsSubmitted { reveal_round: Option<u64> }`;
  - `RegisterSubnet`: `SubnetRegistered { netuid, mode, registered_at_block, … }` (`PY/executor.py:358-386, 501-526`);
  - `CreatePureProxy`: `PureCreated { pure, spawner, height, ext_index }` (`executor.py:571-593`);
  - the multisig intents: `MultisigCall { call_hash, call_data }`.

  Everything else is `()`.
- **No `jsonschema` crate.** Python validates through dataclass construction and `__post_init__`, not JSON Schema.
  rtao uses serde (`deny_unknown_fields`, money parsing) plus the ported `validate()`. The catalog schema is emitted
  verbatim by `tools()`, and the parity gate asserts that the serde field set and required set equal it (§4.9). This
  removes D's `jsonschema` dependency [J-PD §4 D].
- **Adapters**: `MultisigAdapter<I>` and `MultisigThreshold1Adapter<I>` override `wrap_call` and `origin_view` and
  expose the inner intent as the semantic intent (`PY/intents/multisig.py:276-398`). `execute_proxy_announced` keeps
  Python's quirk of building the inner intent against the signing wallet ([R] A1 §10 item 6). That is listed in
  `divergences.toml` as `kept`, not changed.

### 4.3 Generated pass-through bodies, and their limits

Checked against the `build()` bodies, **46 of 83** intents compose exactly one call whose arguments are fields renamed
only by suffix stripping (`_ss58`, `_tao`, `_alpha`, `_rao`, `_hex`), a generic origin default or a field-type
conversion (parity-inventory §4.1). The other 37 select between calls, compose several, or derive arguments. The
field-name heuristic of [J-MA §3.4] gets the count right but counts `multisig_threshold_1` and misses `transfer_all`
(parity-inventory §4.2). For the 46, `xtask` emits `build()` into `generated/passthrough.rs` from a generated
field-to-argument map, with unit conversions carried by field types (`take: Ratio → PerU16`, proportions →
`U64Normalized`, money resolved per §5.2).

The generator lands in **M2**, with the rest of `xtask regen`'s intent output (interfaces, `AnyIntent`,
`defaults.rs`). Ops still in `pending.toml` keep their `NotYetPorted` stubs; `transfer_all` (M2) and the nine M4 owner
and registration ops use the generated bodies as they land, and M5 adds the remaining 36 (§14).

`codegen/intent_callmap.toml` can declare renames (`swap_basket`: `dest_netuid` → `destination_netuid`) to move
near-pass-through intents into the generated set.

This **does not** mean the 46 cost nothing [J-MA concern 8]. `summary()`, the preflight hooks, `"all"` resolution
and executor post-processing stay hand-written wherever Python has them:

- `burned_register`: MEV-required, with a burn/lock split preview (`PY/intents/registration.py:27-44`);
- `register_subnet`: queued completion, executor-level (§3.13);
- `set_children`: proportion conversion and validation;
- `claim_root`: the 771-line fee model (`PY/intents/_root_claim_fee.py`), ported as a preview only.

M5's scope (§14) counts all 83 intents as hand-reviewed, not 37.

### 4.4 Hand-ported bodies

Bodies mirror Python file for file, so a Python diff lands on one obvious Rust module:

```
crates/rtao-intents/src/
  staking.rs (PY/intents/staking.py, 1,925 lines)   weights.rs (weights.py, 563)   registration.rs (595)
  transfer.rs  evm.rs  children.rs  coldkey.rs  collateral.rs  lock.rs  governance.rs  hyperparameters.rs  identity.rs
  root.rs  serving.rs  proxy.rs  multisig.rs (536)  crowdloan.rs  leasing.rs  association.rs  batch.rs
  money.rs (_money.py, 165)  root_claim_fee.rs (_root_claim_fee.py, 771)
  generated/{interfaces.rs, any_intent.rs, defaults.rs, passthrough.rs}
```

- Every public item carries `#[doc(alias = "bittensor.<dotted.path>")]` (A's rule R3). rustdoc search then finds Rust
  items by their v11 names, and the parity symbol layer joins on the alias (§4.9).
- Provenance is file-level, in `parity/map.toml` with blob SHAs (§4.9). This replaces D's per-function
  `// py: file:Lx-Ly` comments, which go stale under upstream edits [J-MA G1].
- Python behaviors reproduced literally ([R] A1 §3, §10) include:
  - slippage limits `price × (1 ± tol)` with `allow_partial = false` and default tolerance 5%;
  - `"all"` = free − ED − 500,000 rao (`PY/intents/staking.py:248-267`), or `u64::MAX` claim-all where Python uses it;
  - `move_swap_stake` swapping `amount − 1 rao`;
  - `set_take` choosing increase or decrease from `Delegates`;
  - `alpha_low` and `alpha_high` sharing one setter;
  - fixed-point values encoded as `{bits}`;
  - weights preflight reading `LastUpdate` at `NetUidStorageIndex(netuid, mechid)` while computing reveal rounds with the
    real netuid (`weights.py:164-167, 288-290`);
  - `claim_root.subnets` sent but ignored on chain ([R] A1 §10 item 11).

### 4.5 Compile-time completeness

`AnyIntent` is generated with one variant per catalog op:

```rust
// @generated
#[non_exhaustive]
pub enum AnyIntent {
    #[cfg(feature = "staking")] AddStake(AddStake),
    // … 83 variants, each gated by its Python group feature
}
impl AnyIntent {
    pub fn from_tool(op: &str, args: serde_json::Value) -> Result<Self, IntentError>;   // strips a top-level "op" key
                                                    // (its value is ignored, as from_args does), then serde + validate()
    pub fn to_json(&self) -> serde_json::Value;     // "op" first, money as exact decimal strings (_encode, PY/intents/base.py:357-364)
    pub fn spec(&self) -> &'static IntentSpec;
    pub fn as_dyn(&self) -> &dyn DynIntent;                                              // match-delegated
}
```

Every variant requires `impl Intent`, so a catalog op without an implementation is a **compile error**. During the
build-out an op may be listed in `parity/pending.toml`, and `xtask regen` then emits a stub `impl Intent` whose `build()`
returns `IntentError::NotYetPorted(op)`. The parity gate lets `pending.toml` **only shrink**, except for entries that a
pin bump adds for ops newly added upstream (§4.9). Reads work the same way through `ReadName` (§4.8).

Python's `to_dict()` "round-trips through `from_args` exactly", and `from_args` ignores an `"op"` key
(`PY/intents/base.py:357-372`; `registry.build`, `PY/intents/registry.py:34-40`), so `execute_tool(op,
intent.to_dict())` and agents that echo the op key work. An oracle vector asserts that `from_tool(op,
to_json(sample))` round-trips for all 83 samples.

### 4.6 Executor, plan, policy (`rtao-tx`, port of `PY/executor.py`, 1,319 lines)

```rust
pub struct Executor { client: Client, policy: Option<Policy> }   // nonces: client.nonces(), one per Client (§3.9.1)
impl Executor {
    pub async fn plan<I: Intent, K: KeySource<I::Role>>(&self, i: &I, keys: &K, o: &PlanOptions) -> Result<Plan, TxError>;
    pub async fn preflight<I: Intent, K: KeySource<I::Role>>(&self, i: &I, keys: &K, o: &PlanOptions) -> Result<IntentPreflight, TxError>;
    pub async fn execute<I: Intent, K: KeySource<I::Role>>(&self, i: &I, keys: &K, o: &ExecuteOptions)
        -> Result<ExtrinsicResult<I::Output>, TxError>;
    pub async fn submit_shielded<I: Intent, K: KeySource<I::Role>>(&self, i: &I, keys: &K, o: &ShieldOptions)
        -> Result<ExtrinsicResult<I::Output>, TxError>;
    // dyn twins: Python's runtime role semantics (CLI, agents)
    pub async fn plan_dyn(&self, i: &dyn DynIntent, keys: &dyn DynKeySource, o: &PlanOptions) -> Result<Plan, TxError>;
    pub async fn execute_dyn(&self, i: &dyn DynIntent, keys: &dyn DynKeySource, o: &ExecuteOptions) -> Result<ExtrinsicResult, TxError>;
    pub async fn execute_tool(&self, op: &str, args: serde_json::Value, keys: &dyn DynKeySource, o: &ExecuteOptions)
        -> Result<ExtrinsicResult, TxError>;                                                     // PY/client.py:483-488
    // raw and offline paths
    pub async fn submit_call<R: Role, K: KeySource<R>>(&self, call: CallBytes, keys: &K, o: &ExecuteOptions)
        -> Result<ExtrinsicResult, TxError>;     // refused when an active Policy lacks allow_raw_calls; no policy, no
                                                 // restriction (PY/executor.py:1216-1235)
    pub async fn submit_call_dyn(&self, call: CallBytes, keys: &dyn DynKeySource, signer: SignerRole, o: &ExecuteOptions)
        -> Result<ExtrinsicResult, TxError>;     // `rtao call --signer coldkey|hotkey`
    pub async fn multisig(&self, signatories: &[AccountId], threshold: u16) -> Result<Multisig, TxError>;  // PY/client.py:564-590
    pub async fn prepare_call(&self, call: CallBytes, address: AccountId, o: &PrepareOptions) -> Result<UnsignedExtrinsic, TxError>;
    pub async fn submit_signature(&self, u: UnsignedExtrinsic, sig: MultiSignature, o: &ExecuteOptions) -> Result<ExtrinsicResult, TxError>;
    pub fn tools(&self) -> Result<&'static [ToolSpec], IntentError>;                            // = list_tools (registry.py:43-66)
}
pub struct ExecuteOptions {                       // defaults = PY/executor.py:782-788
    pub period: Option<u32>,                      // Some(128); None = immortal
    pub wait_for_inclusion: bool,                 // true
    pub wait_for_finalization: bool,              // true
    pub retries: u32,                             // 0
    pub proxy_for: Option<AccountId>, pub force_proxy_type: Option<ProxyType>,
    pub policy: Option<Policy>,
    pub wait_for_registration: bool,              // true
    pub registration_timeout: Option<Duration>,   // None
    pub progress: Option<tokio::sync::mpsc::Sender<Progress>>,
    pub reconcile: bool,                          // false (rtao addition; framework sets true)
    pub deadline: Option<Duration>,               // None (rtao addition)
    pub submit_deadline: Option<Duration>,        // None; admission bound independent of era inclusion
    pub shielded: bool,                           // false; raw calls only: §3.11 with ShieldPeriod::P8, or P4 when
                                                  // period == Some(4); other periods are refused (PY/executor.py:1216-1256)
}
```

**Raw calls and the multisig account object** (`PY/multisig.py:166-313`, `PY/executor.py:1216-1256`):

```rust
pub struct Multisig { pub signatories: Vec<AccountId>, pub threshold: u16, pub address: AccountId }  // rtao_tx::multisig
pub struct ApproveOptions { pub signer: SignerRole /* Coldkey */, pub wait_for_inclusion: bool /* true */,
                            pub wait_for_finalization: bool /* false */, pub shielded: bool /* false */ }  // :184-193
impl Multisig {
    pub async fn approve(&self, ex: &Executor, call: CallBytes, keys: &dyn DynKeySource, o: &ApproveOptions)
        -> Result<ExtrinsicResult, TxError>;
}
```

`approve` ports four pieces of Python:

1. `_preflight_funds`, best effort. The opener is the signer when no `Multisig.Multisigs[address, call_hash]` entry
   exists; the required balance is `DepositBase + threshold × DepositFactor` plus the `as_multi` fee. A shortfall
   raises `ChainError { code: InsufficientBalance }` with Python's message and `fund_help` as remediation
   (`PY/multisig.py:32-49, 121-162, 270-313`). It lives in `rtao_tx::multisig`, and the M5 multisig intents reuse it.
2. `_compose_as_multi`: the timepoint from `Multisig.Multisigs`, `max_weight` from `Client::estimate_weight` (§3.10),
   the other signatories sorted by account bytes, always `as_multi` with the full call (§3.12).
3. The clear submission path.
4. The shielded path through §3.11 with `ShieldPeriod::P8`.

The typed `submit_call<R, K>` keeps its role type parameter, and `submit_call_dyn` serves the CLI. All of this lands in
M4, because `rtao call --multisig/--multisig-threshold/--signatories/--other-signatories` is CLI v0 (§10.6). Tests port
`PYT/unit/test_multisig_safety.py`.

**`plan`** follows Python exactly ([R] A1 §4.1-4.6):

Read queries use one pinned snapshot; composition uses the current head signing runtime. This distinction lets a
runtime replan preserve historical view identities while constructing bytes valid for the current transaction version.
The tool manifest projects the authenticated catalog's Python schema and fields once; malformed generated data returns
an error. `IntentRegistry` injects the feature-enabled tool parser from `rtao-intents` without a dependency cycle.
`rtao::Executor::new(client)` attaches that registry when the facade's `intents` feature is enabled. The wrapper
delegates planning and execution to `rtao_tx::Executor`, shares its client and nonce manager, and retains the
registry through policy and clock configuration. Applications using the lower layer directly can construct
the same configured executor with `rtao_intents::executor(client)` or install their own `IntentRegistry`.

1. Coerce addresses: wallet-like values in `*_ss58` fields become the matching key's address (`executor.py:142-157`).
2. Compose: pinned inner call data, then the semantic build against the origin view, then sudo, proxy and adapter
   wrappers (§3.12).
3. Accounts: `dispatch_origin` versus `fee_payer` (`_intent_accounts`, `:234-246`). The public view never unlocks
   (`PY/signing.py:266-283`).
4. Run the intent preflight.
5. Fee: the intent's exact fee if it supplies one, else the probe (§3.10). A failure is a warning.
6. Add `COLDKEY_FEE_WARNING` when a hotkey signs a coldkey-pays-fee leaf. Leaves are found by walking the
   Proxy/Utility/Multisig wrappers over the runtime-extracted set (`PY/executor.py:744-745`).
7. Policy check, with the intent's `blocks` merged into the violations.

**`execute`**:

1. `mev_shield_required` intents go straight to `submit_shielded` (`executor.py:812-824`).
2. Otherwise `plan`, and refuse with `PolicyError` unless the plan is `ok`.
3. Resolve the signer for the role, then submit **exactly** `plan.call`, so the dry run and the signed bytes cannot
   diverge (`PY/intents/plan.py:89-135` [J-CF §2]).
4. Transient-only retries (§3.9.6).
5. Re-check nested dispatch failures.
6. Merge extras: `pure_proxy` data, plan extras, and the `register_subnet` completion (§3.13).

If the head runtime changed between plan and submit, `execute` re-plans once (D-013).

```rust
pub struct Policy {                                   // PY/intents/plan.py:21-85, Python's violation wording
    pub max_fee: Option<Balance<Tao>>,                // set + no estimate ⇒ violation ("must not fail open")
    pub max_spend: Option<Balance<Tao>>,              // Spend::Unbounded ⇒ violation whenever set
    pub allowed_netuids: Option<BTreeSet<NetUid>>,    // affects_all_subnets ⇒ violation whenever set
    pub allow_raw_calls: bool,                        // submit_call / submit_signature refused unless true
}
/// What Policy::check needs from an intent (blanket-implemented for every DynIntent; raw calls implement it with
/// is_raw = true). This is also the shape proposed upstream as PR-U8.
pub trait PolicySubject: Send + Sync { fn spend(&self) -> Spend; fn touches_netuids(&self) -> SmallVec<[NetUid; 2]>;
                          fn affects_all_subnets(&self) -> bool; fn is_raw(&self) -> bool; }
impl Policy { pub fn check(&self, subject: &dyn PolicySubject, fee: Option<Balance<Tao>>) -> Vec<String>; } // on semantic_intent()
pub struct Plan { pub op: &'static str, pub summary: String, pub signer: SignerRole, pub signer_address: AccountId,
    pub fee: Option<Balance<Tao>>, pub effects: Vec<String>, pub warnings: Vec<String>, pub violations: Vec<String>,
    pub extras: serde_json::Map<String, serde_json::Value>, pub spend: Spend, pub args: serde_json::Value,
    call: CallBytes /* private: execute submits exactly this */ }
impl Plan { pub fn ok(&self) -> bool; pub fn to_json(&self) -> serde_json::Value; } // fee_tao, spend_tao, spend_unbounded, ok, docs_url
```

Two quirks are kept literally:

- `allowed_netuids` is checked against `touches_netuids()`, which is empty for intents without a `netuid` field, so
  `transfer` and `claim_root` pass any allowlist ([R] A1 §10 item 20). This is kept for parity and documented.
- `tests/policy_equivalence.rs` builds 21 of the core's 22 trusted constructors (`CORE/transaction.rs:251-826`) and
  pairs them with rtao intents: 18 by op name; `transfer_allow_death` (`:285`) ↔ `Transfer { keep_alive: false }`
  (`PY/intents/transfer.py:52-63`); `move_stake_limit` (`:499`) ↔ a cross-subnet `MoveStake` with slippage protection
  (`PY/intents/staking.py:848-874`); `transfer_stake_and_hotkey` (`:603`) ↔ `TransferStake` with a different
  `dest_hotkey_ss58` (`:1487-1499`). It asserts that `bittensor_core::transaction::Policy::check` and rtao's
  `Policy::check` report the same violation *set* (wording excluded). `set_root_claim_type` (`:720`) is a retired call,
  absent from spec-477 metadata (85 SubtensorModule calls, none of them it), and is excluded, mapped n/a like its e2e
  name (§11.2).

### 4.7 Results and the error model

```rust
pub struct ExtrinsicResult<O = ()> {        // PY/result.py:697-749; to_json() == Python to_dict()
    pub success: bool, pub message: String, pub block_hash: Option<H256>, pub extrinsic_id: Option<String>, // "{block}-{idx:04}"
    pub explorer_url: Option<String>,       // finney and archive only (template extracted from PY/settings.py)
    pub fee: Option<Balance<Tao>>, pub events: Vec<EventRecord>, pub error: Option<ChainError>,
    pub data: serde_json::Map<String, serde_json::Value>,   // Python's dict, verbatim
    pub output: O,                                          // typed view of `data` (§4.2)
    pub inclusion: Inclusion,                               // rtao addition: Unobserved | PoolAccepted | InBlock | Finalized
    pub fate_unknown: Option<H256>,                         // rtao addition (§3.9.5)
}
impl<O> ExtrinsicResult<O> { pub fn raise_for_failure(self) -> Result<Self, ChainError>; pub fn to_json(&self) -> serde_json::Value; }

pub struct ChainError { pub message: String, pub name: Option<String>, pub code: ErrorCode }   // result.py:501-562
impl ChainError {
    pub fn remediation(&self) -> &'static str;   // per-name override → substring override → per-code default
    pub fn description(&self) -> Option<&'static str>;
    pub fn explanation(&self) -> &'static str;   // EXPLANATIONS[code]
    pub fn docs_url(&self) -> Option<String>;    // absolute DOCS_URL + /errors/chain/<Name> or /errors/<code-with-dashes>; None for unnamed Unknown
}
#[non_exhaustive] pub enum ErrorCode { /* GENERATED from CAT/errors.json `codes` (18): InsufficientBalance,
    InsufficientLiquidity, RateLimited, NotRegistered, NotAuthorized, AlreadyExists, NotFound, SubnetNotExists,
    SubtokenDisabled, Disabled, TooEarly, Expired, LimitExceeded, UnitMismatch, InvalidArgument, PolicyViolation,
    Internal, Unknown */ }
pub enum TxError {                           // mirrors BittensorError's tree (result.py:45-73, 687-693)
    Policy(PolicyError),                     // code = PolicyViolation
    Chain(ChainError),                       // includes client-side preflight errors with chain codes (weights.py:154-162)
    RpcPolicy(RpcPolicyError),               // status_code, retry_after (raw str), policy, reason
    Rpc(RpcConnectionError), ConnectionNotReady,
    Intent(IntentError), Signer(SignerError), UnitMismatch(UnitError),
    SurfaceChanged { pallet: String, call: String, detail: String },   // rtao (§3.7)
    SigningDisabled { reason: String },                                // rtao (extension guard, genesis, V14)
    ShieldEraExpired { period: u32, finality_lag: u64 },               // rtao (§3.11 step 1, D-025)
}
impl TxError { pub fn code(&self) -> ErrorCode; }
/// Rpc(RpcError::Policy) → RpcPolicy; other Rpc → Rpc; SigningDisabled and SurfaceChanged → the same-named variants;
/// the rest → Chain with the mapped code. ChainError stays the Python-shaped chain error (result.py:501-562).
impl From<ClientError> for TxError { /* … */ }
```

**Classification** (`classify(message, name) -> ErrorCode`) keeps the precedence of `PY/result.py:483-498`:

1. `NAME_TO_CODE` (generated, 390 names);
2. `DISPATCH_ERRORS` (extracted);
3. a name ending in `RateLimitExceeded` or containing `RateLimit` → `RateLimited`;
4. with no name, the extracted substring fallback;
5. otherwise `Unknown`.

`chain_error_from_substrate_request` parses `"Custom error: N"` and `{"Custom": N}` through the custom-code table,
standard `InvalidTransaction` texts through the needles, and watch fatals through `_WATCH_STATUS_RE`
(`result.py:574-634`). Module errors resolve `(pallet_index, error_index)` through `Runtime::module_error` at the
block's runtime, never through a stale table. That includes nested module errors inside `Sudo.Sudid`,
`Proxy.ProxyExecuted` and `Multisig.MultisigExecuted`, which Python resolves through its generated table
(`PY/result.py:636-656`) while resolving top-level errors live (`PY/_transport/utils/receipt.py:55-65`,
`PY/_substrate.py:796-810`). The two agree at the pinned spec (405/405 at 477), so they differ only across upgrades and
on historical blocks (D-028).

**Message text.** A named error's `message` comes from a name-keyed docs map built from `MODULE_ERRORS` in
pallet-index order, last name wins, then `DISPATCH_ERRORS[name].1`, then the name (`_CATALOG_DOCS_BY_NAME`,
`PY/result.py:35, 565-571`; 10 names carry different docs in different pallets, for example `InsufficientLiquidity`
in SubtensorModule and Swap). Nested module errors take `docs or name` (`:653-656`). The generated `MODULE_ERRORS` table
therefore serves docs, gates and message text, never resolution. Oracle vectors pin `ChainError.to_dict()` for every
custom code 0-27 and 255 and every `DISPATCH_ERRORS` name.

**Gates**, a port of `codegen/check.py --names` (`:349-455`) running as Rust tests in `pr-gates`:

- every chain error name in the pinned metadata classifies, and every classified name still exists;
- every code has a remediation and an explanation;
- the custom-code key set equals the `u8` arms parsed with `syn` from `common/src/transaction_error.rs` at `parity_tag`
  (`impl From<CustomTransactionError> for u8`, `:38` [J-MA §3.1]). Python's own gate hard-codes
  `frozenset(range(28)) | {255}` (`check.py:349-351`); rtao's gate tracks the runtime.

`rtao explain <code|Name>` renders the same data (§10).

### 4.8 Reads

**Generated** (`crates/rtao-reads/src/generated/`):

```rust
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub enum ReadName { AcceptsLockedAlpha, AlphaPrice, AlphaPrices, /* … 102 … */ }
pub struct ReadSpec { pub name: &'static str, pub summary: &'static str, pub description: &'static str,
    pub params: &'static [ParamSpec],             // (name, JsonType, doc, required: bool, default: Option<&'static str>
                                                  //  (a JSON literal), own_key: bool), from read_params.json (§4.1)
    pub category: Category, pub namespace: &'static str, pub docs_url: &'static str,
    pub hits: &'static [Hit],                     // H = extracted static hits ∪ read_hits_extra.toml (below) ⊇ catalog hits
    pub render: Option<RenderHint>,               // Grouped { key } | Matrix { row, col, value } (PY/reads/base.py:33-54)
    pub pinning: Pinning }                        // Single | Pinned (every multi-RPC read is Pinned, §3.5)
pub async fn dispatch(view: &dyn DynView, name: ReadName, params: &serde_json::Map<String, serde_json::Value>)
    -> Result<serde_json::Value, ReadError>;      // one match arm per read: missing impl = compile error (or pending stub);
                                                  // fills extracted defaults before calling the typed fn
#[non_exhaustive]
pub enum ReadError {
    Client(ClientError),
    InvalidParam { name: String, reason: String },
    NotYetPorted(ReadName),
    SurfaceChanged { descriptor: &'static str, detail: String },   // typed-only APIs after a model mismatch (§3.7)
}
impl ReadError { pub fn code(&self) -> rtao_catalog::ErrorCode; }
```

**Hand-written** (`crates/rtao-reads/src/reads/<module>.rs`, one per `PY/reads/<module>.py`: accounts, chain,
collateral, delegation, epochs, hyperparameters, identity, leasing, locks, neurons, prices, root, staking, subnets,
weights):

- Each read is a generic `async fn` over `V: View` with a typed, `Serialize` return model using Python field names.
- Scalar reads (`scalar_read`, `PY/reads/base.py:156-166`) are generated whole.
- Multi-item reads use `fetch` (§3.6).
- Reads with domain math port Python's arithmetic exactly, including float math, because the read oracle compares
  values. Example: the `subnet_convictions` decay projection and bisection (`PY/reads/locks.py:188-286`). A Rust-only
  helper `conviction_exact` offers the pallet's fixed-point version separately, so there is no divergence on the parity
  surface. B's float-to-fixed change is rejected for the read itself.

**Parameters.** The catalog's `params` carry only a JSON type, so requiredness and defaults come from the extracted
`read_params.json` (§4.1). `dispatch` fills the defaults, so `client.read("neurons", {netuid: 1})` returns the lite
payload as Python does (an oracle vector checks it; `READ_SAMPLES` already omit `lite`, `mechid` and `coldkey_ss58`,
`PYT/harness/samples.py:219, 225, 261, 295, 305`). `rtao query` emits `--x/--no-x` for boolean params and the wallet
fallback for own-key params (`PY/cli/query.py:218-246`). The catalog layer asserts that the (name, type) set equals
`CAT`. `dispatch` lands in M2, the CLI part in M4.

**Namespaces**: the 14 catalog categories map to one accessor struct each, named from the catalog `category` (names as
in the parity-inventory §5 headings). The catalog's `python` field names only the method; `xtask` asserts that its
`sub.<ns>.` prefix, where present, equals the category's namespace (the `metagraph` read has no `python` key, because
the curated `Subnets.metagraph` shadows it, `PY/namespaces.py:161-173`). Python's
`sub.delegation.delegate_take(…)` becomes `client.delegation().delegate_take(&hotkey)`. Every namespace exists on
`Client` (head) and `Snapshot` (pinned) through the generated `rtao_reads::Namespaces` extension trait,
blanket-implemented for `View` and exported from `rtao::prelude`. This keeps the namespace types
in the reads layer without a client-to-reads dependency cycle. The `block=` keyword of Python's namespace methods
(`PY/namespaces.py:52-97`) becomes `client.at(BlockRef::Number(b)).await?.delegation()…`. The curated extras are
hand-written one-liners: `balances().get/get_many/existential_deposit`, `staking().get/positions`,
`subnets().burn/commit_reveal_enabled/metagraph/commitments/info/all`, `neurons().all` (`namespaces.py:99-213`). Two of
them need the commitment machinery and the `Metagraph` model, and are placed to keep §2.3's layering:

- `subnets().commitments()` and the catalog `commitments` read (`PY/reads/identity.py:139-162`) live in
  `rtao_reads::commitments`. That module owns `NeuronCommitment`, `CommitmentMap`, the status machine and the decoders
  (`PY/metagraph.py:37-125, 406-560`) and `fetch_commitments`/`fetch_commitment_map` (`:258-291`); `rtao-metagraph`
  uses it for `Metagraph.commitments`. `NeuronCommitment::reveals_at` uses `rtao_primitives::drand::reveal_time`
  (`PY/metagraph.py:25, 120-124`; §9.8), not `rtao_tx::timelock`.
- `subnets().metagraph(netuid)` is `rtao_metagraph::SubnetsMetagraphExt` (§8.2), implemented for
  `rtao_reads::Subnets<'_, V>` and in `rtao::prelude` under feature `metagraph` (§2.4), so
  `client.subnets().metagraph(NetUid(1))` compiles with the prelude import every example already has. The generated
  `Subnets` accessor emits no inherent `metagraph` method (the read's `python` is empty), because an inherent method
  would shadow the trait method. The catalog `metagraph` read stays Python's raw `SubnetInfoRuntimeApi.get_metagraph`
  dict with `name`/`symbol` as text (`PY/reads/subnets.py:170-190`), reachable through `dispatch(ReadName::Metagraph,
  …)` and the free fn `rtao_reads::reads::subnets::metagraph`.

**Exact hits** [J-MA G7]. The catalog `hits` come from scanning only each read function's own source
(`read_targets`, `UP/website/apps/bittensor-website/scripts/generate.py:405-480`), so items reached through module
helpers or module-level `Item(...)`/`Method(...)` descriptors are missing: 12 reads touch more, 7 of them with non-empty
catalog hits (parity-inventory §5.15; `PY/reads/locks.py:33-40`, `collateral.py:15-17`, `root.py:28-44`). rtao
therefore checks against its own hit set:

- `xtask extract` computes each read's static hit set into `vendor/upstream/extracted/read_hits.json`. It scans
  `PY/reads/<module>.py` helper-aware: the read function plus the module-local functions it calls, transitively,
  resolving `st.`/`api.`/`constants.` chains and module-level `Item(...)`/`Method(...)`/`Constant(...)` literals.
- Items reached outside the module go in `parity/read_hits_extra.toml`, each with a reviewed reason and Python
  citation, for example view methods (`view.timestamp()` → `Timestamp.Now`) and `PY/metagraph.py`'s
  `fetch_commitments`.
- Let H = extracted ∪ extra. The catalog layer asserts that the `CAT` hits are a subset of H.
- A `RecordingView` wraps `FakeChain` and records every storage item, runtime API and constant a read touches. The test
  asserts that every recorded run touches a subset of H, and that the union of the runs over the read's samples equals
  H. The samples are `READ_SAMPLES` plus per-branch samples (for example `miner_collateral` with and without
  `coldkey_ss58`, which reads `Owner` only when the key is omitted, `collateral.py:86-92`), so a branchy read is not
  required to touch everything in one run. The per-branch samples live in `vendor/oracle/samples_extra.json` under
  `reads`, next to the intents' root-subnet samples, so the oracle dumps them like `READ_SAMPLES` and Python's result
  is checked on every branch too; each is added with the read it covers (M2-M6).
- As an independent cross-check, the oracle's `dump_reads.py` records the items `FakeSubstrate` served per sample
  (`touched` in `vendor/oracle/reads.json`), and `xtask oracle` fails if one falls outside H.

`ReadSpec.hits` and `verify_surface` use H, and the helper-aware scan is proposed upstream for `generate.py` (PR-U16).
This lands in M2, before the first read enters the test.

**JSON parity** follows `btcli query --json`'s `_jsonable` (`PY/cli/query.py:104-116` [V]):

- a `Balance` renders as `str(Balance)`, which rtao reproduces byte for byte (§5.3);
- datetimes render ISO-8601 at second precision;
- dataclasses become dicts, tuples become lists;
- floats are rendered from the same f64 values.

The read oracle compares rtao's JSON with Python's on the same seeded state as **parsed values**, by token kind. The
comparator walks both trees and compares integer tokens (no `.`, `e` or `E`) exactly, as i128/u128 and as canonical
decimal strings beyond that range; non-integers as f64 (shortest round trip, so `serde_json`'s `1e-05` equals
`json.dumps`'s `1e-5`); and an integer against a non-integer as a mismatch. It never uses `serde_json::Value` equality,
which compares number spelling under the `arbitrary_precision` feature the core forces on (§2.7), and it never parses
integers as f64, which would make finney's total issuance 11,614,906,923,326,217 equal to …216. The M0 test pins both,
plus 2^53 + 1, `u64::MAX / 4` and its neighbor, and `1e-05` against `1e-5`. Python-identical *bytes* for CLI output is
an M8 nicety (§10.3) and not a gate.

### 4.9 Parity enforcement (the machinery)

`parity/` holds:

| File | Content |
|---|---|
| `map.toml` | one row per watched upstream file: `py` (monorepo path), `rs` (Rust paths), `kind` (`generated`, `extracted`, `vendored`, `ported`, `oracle-only`, `n/a`), `synced_blob` (git blob SHA of the upstream file when last reconciled, or `pending`), `synced_tag`, `notes`. Watched roots: `sdk/python/bittensor/**`, `sdk/python/pyproject.toml` (extracted: version, `[project.scripts]`), `sdk/python/completions/**`, `sdk/python/scripts/{record_golden,record_shape_corpus,build_basket_index}.py`, `sdk/python/codegen/**`, `sdk/python/tests/{unit,harness,fixtures,e2e}/**`, `sdk/bittensor-core/**`, `sdk/bittensor-core-py/**` (`oracle-only`: Python reaches the core only through this 1,865-line binding, which changed in v473→v475; the rows for `values.rs`, `runtime.rs`, `timelock.rs` and `keys.rs` name the rtao code that mirrors their shapes), `common/src/{transaction_error,proxy}.rs`, `runtime/src/{fee_filters,check_mortality,transaction_payment_wrapper}.rs`, `pallets/subtensor/src/rpc_info/**`, `pallets/subtensor/src/coinbase/{run_coinbase,reveal_commits,tempo_control}.rs`, the chain-rule sources the framework mirrors (kind `ported`: `pallets/subtensor/src/subnets/{serving,weights,subnet}.rs` → `AxonPublisher`, `WeightScheduler`, `OwnerOps::create_subnet`; `pallets/subtensor/src/utils/{misc,rate_limiting}.rs` and `pallets/admin-utils/src/lib.rs` → `HyperparamPlanner`; `tempo_control.rs` → `rtao-core-ext::epoch_schedule` and `HyperparamPlanner`), `docs/guides/signed-requests.mdx`, the catalogs, `eco-tests/**`. `pallets/subtensor/src/lib.rs` is too large for a blob row; its constants are extracted instead (layer 5). From M0 the file holds every row of python-module-map.md (357 at v477), each `pending` until ported and blessed |
| `pending.toml` | ops and reads not yet ported (shrink-only) |
| `divergences.toml` | each deliberate difference: id, upstream symbol, Python behavior, rtao behavior, rationale, pinning test, upstream issue/PR, class (`stricter`, `improved`, `kept`, `addition`, `typed-only`) (Appendix C) |
| `raw_only_extra.toml` | chain calls rtao deliberately leaves raw-only beyond Python's `RAW_ONLY` |
| `e2e_map.toml` | each of the 112 manifest names, and each `def test_*` in `sdk/python/tests/e2e/*.py` (keys `pyt:e2e/<file>::<function>`, discovered with `rustpython-parser` at `parity.commit`; 115 keys at v477) → `rtao = "<target>::<test fn>"`, `na = "<reason>"`, or `pending = "M<k>"`. `parity check` fails on an unmapped key and on an `rtao` value that names no test function in `crates/rtao-testkit/tests/{e2e,interop}/` (resolved with `syn`); strict mode (M8) fails on any `pending`. A key leaves `pending` in the milestone that ships the code it exercises: the 72 live `intent_*` keys with the table-driven plan test (M5); the 24 `test_*` keys that parity-inventory §3 ties to ops with the latest of those ops' milestones; the other 15 `test_*` keys as listed in parity-inventory §2.5; the three `pyt:e2e` keys with scenarios 4 (M4), 14 (M7) and 19 (M7) |
| `env.toml` | each environment variable the Python package reads (§4.1) → its rtao consumer (`crate::item`) or a divergence id |
| `read_hits_extra.toml` | per read, the reviewed chain items it reaches outside its module (view methods, `fetch_commitments`), each with a Python citation (§4.8) |
| `cli_json_volatile.toml` | per CLI command, the JSON fields derived from the wall clock that interop lane 9 skips, each with a Python citation (§11.6) |
| `exports_extra.toml` | each name of a public `__all__` list that has no `#[doc(alias)]` counterpart, with its reason (a CLI-only home or n/a) (layer 8) |
| `matrix.json` | generated by `xtask parity`: every row family below with rtao status and oracle status |

Kind `vendored` means copied byte for byte at `parity_tag`, with a sha256 in `upstream.toml [artifacts]` (the 21
EVM ABIs, `bridge.html`, `vault.html` and the chain-specs PNG). A changed blob is re-vendored by a pin bump and
auto-blessed only when its consumers' tests pass (ABI decode tests; the bridge and Vault pages served); the extraction
layer and the extracted-table rule of auto-bless do not apply to it.

`cargo xtask parity check` runs **offline in `pr-gates`** and fails on any unexplained difference:

1. **File layer** (from A).
   - The upstream tree at `parity.commit` is read from the vendored `vendor/upstream/tree.txt` (§1.2), so ordinary
     pushes need no network. `pin-bump` and `upstream-next` refresh it from a cached partial clone
     (`git clone --filter=blob:none`; `git ls-tree -r`).
   - Rows start `pending` (seeded in M0 from python-module-map.md). The layer checks presence (new or vanished files)
     for every row and blob equality only for blessed rows; strict mode (M8) allows no `pending` row.
   - A new upstream file under a watched root without a `map.toml` row fails, and so does a row whose `py` no longer
     exists.
   - A row whose `synced_blob` differs from the current blob fails, and the report shows
     `git diff <synced_blob> <current_blob>`. This catches behavior-only Python changes that leave the catalogs
     unchanged, such as v466 → v467 rewriting 142 lines of `_root_claim_fee.py` with `intents.json` unchanged
     [J-MA §1, concern 2].
2. **Symbol layer** (from A, Rust-native).
   - Every changed `.py` file is parsed, old and new blob, with `rustpython-parser` (pinned registry version; xtask only), and
     the diff covers module-level names, classes, dataclass fields (name, annotation, default, `help`), `ClassVar`s,
     functions and methods with parameters and defaults, and decorator arguments.
   - Each changed symbol joins its Rust item through the `#[doc(alias)]` index, which `syn` builds from the rtao tree.
     The report reads "Python `X` changed → Rust `crates/…:line`" or "no Rust counterpart".
3. **Catalog layer.** Generated registries are compared with the vendored catalogs:
   - intents: name, `python_class`, signer, origin, verify, wraps as a set, and the normalized schema (property names,
     JSON types, required set, descriptions);
   - reads: name, params, docs, category, hits;
   - errors: codes and names.
   Defaults are compared between the extracted Python defaults and the generated `defaults.rs`.
4. **Extraction layer.** `xtask extract --check` re-extracts every literal table from the vendored sources at
   `parity_tag` (`vendor/upstream/src/`, §1.2) and diffs the committed JSON and the generated Rust. The extractor
   accepts literals (`str`, `int`, `float`, `bool`, `None`, implicit string concatenation, tuples, lists, dicts, sets,
   `frozenset(...)`), enum attribute references, constructor calls whose arguments are accepted, and seven resolved
   forms that the named tables need (`RAO_PER_TAO = 10**9`, `frozenset({-32004, -32005})`, `os.getenv(…) or …`,
   `st.SubtensorModule.Rho`, `DEFAULT_RATE_TOLERANCE`, `field(default_factory=lambda: [0])`, `Path.home() / …`):
   1. unary `-`/`+` on numeric literals, and constant folding of integer `+ - * // % ** << >> | &`, float `+ - * /`,
      string `+` and string × integer `*` over accepted operands, with overflow checks (|result| < 2^256);
   2. bare names bound at module level to an accepted value, in the same module, in the module the name is imported
      from, or in `PY/settings.py`; in test-harness tables, dev-key names resolve through the fixed ss58 table of §4.1;
   3. environment defaults, resolved at run time: `os.getenv("N") or <lit>` becomes `{env, default, empty_is_unset:
      true}` and `os.environ.get("N", <lit>)` becomes `{env, default, empty_is_unset: false}`;
   4. attribute chains into the generated descriptor modules (`st.<Pallet>.<Item>`, `api.<Api>.<method>`), emitted as
      `{pallet, item}` and checked against the pinned metadata;
   5. `field(default_factory=lambda: <accepted>)`;
   6. the builtin type names `str`, `int`, `float` and `bool` used as values (type tags, `PY/config.py` `SETTABLE`);
   7. `Path.home() / <str> / …`, emitted as `{home_relative: [...]}`.

   Anything else fails loudly with file and line, which forces a hand port (A's transpile subset). Derived tables
   (comprehensions such as `_REVERSE_NETWORKS` and `_CATALOG_DOCS_BY_NAME`) are not extracted; rtao derives them. An M1
   test, `extract_check_covers_named_tables`, runs `xtask extract --check` over every table §4.1 and the module map
   name as extracted: `PY/settings.py`; `PY/_transport/rpc.py:52-66, 294-300`; `PY/error_map.py:488, 524, 555`;
   `PY/result.py:27-36, 78, 116, 291, 308, 475`; `PY/executor.py:52-66`; `PY/multisig.py:162`;
   `PY/hyperparams.py:32-37, 83, 110-408, 416-461, 467-471, 626-643, 646-657`; `PY/intents/hyperparameters.py:31-66`;
   `PY/intents/proxy.py:21-41`; `PY/intents/_money.py:89-111`; the intent field defaults and class vars;
   `PY/fee_filters.py`; `PY/evm/networks.py:27-43`; `PY/config.py:12-56`; `PY/basket_index_data.py`;
   `codegen/check.py:58-72, 74-300`; `PYT/harness/samples.py`; and `PYT/harness/fake_substrate.py:34-77`.
   `SetHyperparameter.wraps` is derived in Rust from extracted `OWNER_HYPERPARAMETERS`, with exact bounded
   source-AST validation, insertion-order deduplication and provenance for both declarations. Changes to its
   generator shape fail the gate; this does not permit general comprehension evaluation.
5. **Chain-source layer** (`syn`, D and A):
   - `transaction_error.rs` custom codes;
   - `fee_filters.rs` `call_filter_group!` (the Python mirror must equal it);
   - `SelectiveMetagraphIndex::from_index` arms, which must also equal the `SelectiveMetagraph` field order;
   - `freeze_struct` hashes of `rpc_info/*.rs` as layout tripwires (`metagraph.rs:14,119` [J-MA §3.2]);
   - `PROXY_TYPES` versus the metadata `ProxyType` enum;
   - `MAX_SHIELD_ERA_PERIOD` from `runtime/src/check_mortality.rs:19` (a private runtime constant, absent from
     metadata) into `rtao_catalog::rt::MAX_SHIELD_ERA_PERIOD`; tests assert `ShieldPeriod::MAX.blocks() ==
     rt::MAX_SHIELD_ERA_PERIOD` and `py::MEV_SHIELD_ERA_PERIOD <= rt::MAX_SHIELD_ERA_PERIOD` (`PY/settings.py:24-29`
     defines a default that must not exceed the runtime bound);
   - the `prod_or_fast!` rate-limit pairs that are `#[pallet::type_value]`s and absent from metadata
     (`MechanismCountSetRateLimit`, `MechanismEmissionRateLimit`, `MaxUidsTrimmingRateLimit`;
     `pallets/subtensor/src/lib.rs:1128-1132, 3567-3577`) into `rtao_catalog::rt::rate_limits` (§9.7);
   - the chain rules the framework mirrors, into `vendor/upstream/extracted/chain_rules.json`: `MIN_TEMPO`,
     `MAX_TEMPO`, `MIN_ACTIVITY_CUTOFF_FACTOR_MILLI`/`MAX_ACTIVITY_CUTOFF_FACTOR_MILLI` (`lib.rs:2325-2331`),
     `DefaultAdminFreezeWindow` (`:1436-1438`) and the `rate_limit_on_subnet`/`TransactionType` arms
     (`utils/rate_limiting.rs:38-53, 89-131, 134-152`). A code-only change to an arm produces no metadata drift, so
     this and the blob rows above are what catch it.
6. **Coverage gate** (a port of `check.py:303-346`). Every call of the 13 covered pallets in the pinned metadata is
   wrapped by some `WRAPS`, listed in the extracted `RAW_ONLY`, or listed in `raw_only_extra.toml`. A new chain call
   therefore forces a decision. Root-origin intents must name an existing `verify` read.
7. **Oracle layer.** The committed `vendor/oracle/*.json` (§11.5) is compared with rtao's behavior in ordinary
   `cargo test` (no Python needed).
8. **Completeness.**
   - `pending.toml` may only shrink, unless the change is a pin bump (it moves `[parity]` in `upstream.toml`) and the
     new entries correspond to new upstream ops.
   - Every `divergences.toml` entry needs a test and an upstream link.
   - Every name in `PY/__init__.py` `__all__` (`:122-219`) has a row in python-module-map.md's top-level exports table
     or a `#[doc(alias)]` hit. The same holds for the other public `__all__` lists, which the root list does not cover:
     the 14 outside `_transport/` and `cli/` at v477 (`intents`, `reads`, `evm`, `extension`, `vault` and
     `vault/signer.py`, `http_auth.py`, `keyfiles.py`, `ledger.py`, `timelock.py`, `wallet.py`, `basket_index.py`,
     `masked_input.py`, `error_descriptions`). Each name needs a `#[doc(alias = "bittensor.<module>.<name>")]` hit or
     a reasoned row in `parity/exports_extra.toml` (a CLI-only home, or n/a), so a public function added to, say,
     `bittensor.evm` cannot stay unported once its file is blessed.
   - From M8 the gate is strict: zero pending, zero unexplained rows.
9. **Environment layer.** `xtask extract` collects into `vendor/upstream/extracted/env_vars.json` (name, file:line,
   literal default) every `os.getenv("X")`, `os.environ.get("X")`, `os.environ["X"]` and Typer `envvar="X"` in
   `PY/**`, resolving a module-level string constant used as the name (`ENV_VAR`, `PY/cli/logs.py:32, 51`).
   `parity/env.toml` maps each name to its rtao consumer or a divergence id, and the gate fails on an unmapped or
   vanished name. `cli_tree.json` records each option's `envvar`. The seed (28 names plus `BT_PW_*`, v477):

   | Names | rtao consumer |
   |---|---|
   | `BTCLI_CONFIG`, `BTCLI_ADDRESSES_PATH`, `BTCLI_PROXIES_PATH`, `BTCLI_MULTISIGS_PATH`, `BTCLI_MULTISIG_CACHE`, `BTCLI_SUBNET_NAMES_CACHE`, `BTCLI_TOKEN_SYMBOLS_CACHE`, `BITTENSOR_RUNTIME_CACHE_DIR` | `rtao_client::config` (§10.2) and the runtime cache (§3.4) |
   | `BT_CHAIN_ENDPOINT` | `rtao_client::Network` (the `local` preset, `PY/settings.py:38`) |
   | `BT_NETWORK`, `BT_WALLET`, `BT_WALLET_HOTKEY`, `BT_WALLET_PATH`, `BT_FALLBACK_ENDPOINTS`, `BT_ARCHIVE_ENDPOINTS`, `BT_RETRY_FOREVER`, `BT_SIGNER_ADDRESS`, `BT_EXTENSION_BROWSER`, `BT_EXTENSION_BRIDGE` | `rtao-cli` globals (§10.2) and `NeuronConfig` (§9.9); `BT_EXTENSION_BROWSER` also `rtao_signer::extension` (`PY/extension/browser.py:26`) |
   | `BITTENSOR_LOG` | `rtao-cli` logging (§10.3) |
   | `BT_WALLET_PASSWORD`, `BT_WALLET_PASSWORD_FILE`, `BT_PW_*` | `rtao_wallet` (§6.2); `BT_WALLET_PASSWORD` also the `rtao_evm` keystore (`PY/evm/keys.py:122`) |
   | `BT_EVM_ENDPOINT` | `rtao_evm::EvmNetwork` |
   | `BITTENSOR_EXTENSION_BRIDGE_TOKEN`, `BITTENSOR_EXTENSION_BRIDGE_PID` | `rtao_signer::extension` (§6.5) |
   | `GH_TOKEN`, `GITHUB_TOKEN` | `rtao-cli` `upgrade` |
   | `TERM_PROGRAM` | n/a (terminal theming) |

**Bless and auto-bless.**

- `cargo xtask parity bless <py-path>…` records the new blob SHA after a human has ported the change, which gives an
  audit trail in the diff.
- To keep 2-4 releases a week manageable [J-MA §3.1], a pin bump (`xtask parity report --autobless`) auto-blesses a
  changed blob **only if** all
  three hold: its symbol diff is empty, its extracted tables are unchanged, and the oracle vectors of every op or read
  mapped to that file are unchanged. Typical cases are comment and docstring edits. Everything else needs a human port
  plus `bless`.
- For the three catalogs, the file layer, the catalog layer and auto-bless compare the **normalized** catalog: each
  `sources[]` entry without `line`, `end_line` and `url`. Those fields shift whenever `runtime/src/lib.rs` or a pallet
  is edited (`git diff v477 850f2755` of `reads.json` is 27 lines, all of them line/end_line/url), so a catalog blob
  whose normalized form is unchanged is auto-blessed, and the pin bump refreshes its `[artifacts]` sha256.

**Optional source comparison.** A relevant client update can compare catalogs, watched blobs and symbols on
reviewed upstream branches against `parity_tag`. Source data comes from the monorepo rather than the live website,
whose deployment may lag the tag. Scheduled scans and automatic issue creation are deferred (Q30).

---

## 5. Money and units (`rtao-primitives`)

Unit identities come from metadata wherever the runtime declares them (`TaoBalance`, `AlphaBalance`, `NetUid`,
`MechId`, `NetUidStorageIndex`, `PerU16`; [R] A2 §0.7). The remaining kinds (blocks, epochs, per-million,
u64-normalized) come from the extracted `HYPERPARAMS` table. rtao adds no unit knowledge of its own beyond the
semantic overrides of §3.7.

### 5.1 Identifiers

```rust
#[derive(Copy, Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Default, Serialize, Deserialize)] #[serde(transparent)]
pub struct NetUid(pub u16);              impl NetUid { pub const ROOT: NetUid = NetUid(0); }
pub struct MechId(pub u8);               // hard cap 16 ([R] C1 §2.4)
pub struct Uid(pub u16);
pub struct BlockNumber(pub u64);
pub struct NetUidStorageIndex(pub u16);  // mechid * 4096 + netuid (GLOBAL_MAX_SUBNET_COUNT, extracted from PY/settings.py:31)
impl NetUidStorageIndex {
    pub const STRIDE: u16 = 4096;
    pub fn new(netuid: NetUid, mech: MechId) -> Result<Self, UnitError>;   // checked arithmetic
    pub fn split(self) -> (NetUid, MechId);                                 // get_mechagraph returns the index in `netuid`
}
pub struct AccountId(pub [u8; 32]);      // Display/FromStr = ss58 (prefix 42) via rtao_core::keys::{ss58_from_public, public_key_from_ss58}
/// AccountId read from a ValueQuery map whose default is the all-zero account (Owner, SubnetOwner, SubnetOwnerHotkey, Keys).
pub struct DefaultAccount(AccountId);
impl DefaultAccount {
    pub fn get(&self) -> Option<&AccountId>;   // None when all 32 bytes are zero (typed API only; D-001)
    pub fn raw(&self) -> &AccountId;
}
impl Serialize for DefaultAccount { /* the ss58 of raw(), INCLUDING the zero account: Python's JSON is unchanged */ }
pub enum CryptoType { Ed25519 = 0, Sr25519 = 1 }   // CORE/keys/mod.rs:28-29; also the MultiSignature variant byte
```

- **Generated storage keys take the right identifier type.** `LAST_UPDATE` and `WEIGHTS` take `NetUidStorageIndex`,
  so passing a bare `NetUid` is a compile error. Python builds the index by hand (`PY/intents/weights.py:164-167`),
  a known foot-gun ([R] A1 §10 item 8).
- **Encoding into the core.** All identifiers implement `IntoCoreValue`:
  - `NetUid` becomes `Value::Int`;
  - `AccountId` becomes `Value::Bytes(32)`, which `coerce_account_id` accepts (`CORE/codec/encode.rs:488-535` [J-CF §7
    #22]).
  The golden `calls` replay pins these conversions (§11.1).
- **Decoding.** All identifiers implement `scale_decode::IntoVisitor` by unwrapping single-field composites, so `NetUid`
  decodes from `subtensor_runtime_common::NetUid(u16)`, from `Compact<NetUid>` and from a bare `u16` key.
- **`DefaultAccount` and D-001** [J-CF concern 9]. A storage miss decodes from the entry's `default` bytes in metadata
  (§3.6; Python does the same, `PY/_transport/storage.py:22-27`), and the pinned metadata shows that `Owner`,
  `SubnetOwner`, `SubnetOwnerHotkey` and `Keys` are `Default` entries whose default is 32 zero bytes (`jq` over the
  finney metadata, spec 477; they are the only `Default` entries with an `AccountId32` value). So Python's
  `str(value) if value else None` never yields `None` for them (`PY/reads/neurons.py:75-78`,
  `PY/reads/collateral.py:88-92`); `SubnetOwner` is also read by the `register_subnet` completion follower
  (`PY/executor.py:468-481`). Report A2 §9 Q9 inferred this; the metadata settles it offline, so no localnet gate is
  needed. Codegen applies the `DefaultAccount` override by rule over the pinned metadata (§3.7), and an offline test
  over the vendored metadata (`zero_account_default_decodes_to_zero_bytes`) pins it. **Only the typed accessor** maps
  zero to `None`; the JSON keeps Python's zero-account ss58 string, so the read oracle still matches. An upstream issue
  reports Python's dead `None` check (Appendix D).

### 5.2 Amounts: wire newtypes, static TAO versus alpha, Python-shaped balances

```rust
// Wire level (what generated calls and storage use): transparent SCALE, CompactAs.
pub struct TaoBalance(pub u64);      // rao; runtime ident `TaoBalance`
pub struct AlphaBalance(pub u64);    // alpha base units of *some* subnet; the netuid is context
pub struct PriceRao(pub u64);        // rao per alpha (limit_price override, §3.7)
pub struct StakeWeight(pub u64);     // alpha + tao × TaoWeight (Metagraph.total_stake override, §3.7)

// Domain level (intents, framework, users): TAO vs alpha is a compile-time distinction.
pub trait Unit: sealed::Sealed + Copy + Send + Sync + 'static { type Ctx: Copy + Eq + Hash + fmt::Debug + Send + Sync; }
pub enum Tao {}    impl Unit for Tao   { type Ctx = (); }
pub enum Alpha {}  impl Unit for Alpha { type Ctx = NetUid; }
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct Balance<U: Unit> { rao: u64, ctx: U::Ctx }
impl Balance<Tao> {
    pub const ZERO: Self;
    pub const fn from_rao(rao: u64) -> Self;
    pub fn parse(s: &str) -> Result<Self, UnitError>;                // exact decimal, ≤ 9 fractional digits
    pub fn from_tao_lossy(x: f64) -> Result<Self, UnitError>;        // explicit name; rejects NaN, negatives, overflow
    pub fn checked_add(self, o: Self) -> Result<Self, UnitError>;  pub fn checked_sub(self, o: Self) -> Result<Self, UnitError>;
    pub fn saturating_sub(self, o: Self) -> Self;
    pub fn to_wire(self) -> TaoBalance;
    pub fn to_decimal_string(self) -> String;                        // exact, "1.500000000"
}
impl Balance<Alpha> {
    pub fn new(netuid: NetUid, rao: u64) -> Result<Self, UnitError>;            // netuid 0 is TAO: use Balance<Tao>
    pub fn netuid(self) -> NetUid;
    pub fn checked_add(self, o: Self) -> Result<Self, UnitError>;      // Err(UnitMismatch) on different netuids
    pub fn to_wire_for(self, call_netuid: NetUid) -> Result<AlphaBalance, UnitError>;  // Python call_amount check
}
impl TryFrom<DynBalance> for Balance<Alpha> { /* Err(UnitMismatch) when netuid == 0 */ }
pub enum Money { Untagged { rao: u64 }, Tagged(DynBalance) }         // parsed input, or a typed Balance/DynBalance
pub enum Amount { Exact(Money), All }                                // the "all" sentinel (PY/intents/_money.py:34,51-55)
pub enum ResolvedAmount { Exact(DynBalance), All }                   // after unit and netuid resolution (rules below)
pub enum Spend { None, Bounded(Balance<Tao>), Unbounded }            // PY/intents/_money.py:37-48

/// Chain-native netuid-tagged balance; valid-chain JSON follows Python's formatting.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct DynBalance { pub rao: u64, pub netuid: NetUid, pub symbol: Option<Symbol> }
impl DynBalance {
    pub fn tao(&self) -> Result<Balance<Tao>, UnitError>;            // checked u64 conversion; Err unless netuid == 0
    pub fn alpha(&self) -> Result<Balance<Alpha>, UnitError>;        // checked u64 conversion; Err if netuid == 0
    pub fn unit(&self) -> &str;                                      // τ, the chain symbol, or α + subscript netuid
}
impl fmt::Display for DynBalance { /* == Python str(Balance), byte for byte (§5.3) */ }
impl Serialize for DynBalance { /* == _jsonable: str(Balance) */ }
```

Rules:

- **All balances use the chain's native range.** `DynBalance`, typed `Balance<U>` and wire newtypes store u64 rao.
  Checked arithmetic enforces matching netuids and returns errors for underflow or overflow; display symbols do
  not affect equality or hashing. Negative and oversized external input is rejected without truncation. Python's
  arbitrary signed local balance arithmetic is deliberately outside parity (D-035); formatting and exact decimal
  round trips remain compatible for valid chain values. M0 properties adapt the upstream suite to `0..=u64::MAX`
  with explicit out-of-range and arithmetic-boundary tests. The user chose this simpler native API in Q25,
  superseding Q24's signed `DynBalance` decision. Intent input validation remains as below.
- **TAO plus alpha does not compile** [J-PD graft 3]. Alphas of different subnets are a checked `UnitMismatch`, never a
  panic. There are no `Add`/`Sub` impls on money and no `Sum`, only `try_sum`. The money modules deny
  `arithmetic_side_effects` (§2.5).
- **Intent money fields** hold `Amount`. Serde accepts a number, a decimal string matching `money_schema`'s pattern
  (exact, at most 9 fractional digits) or `"all"` (only for `ALL_AMOUNT_FIELDS`), yields `Money::Untagged`, and rejects
  negatives (`PY/intents/_money.py:29-69`). Typed constructors also take `Balance<Tao>`, `Balance<Alpha>` or
  `DynBalance` (`Money::Tagged`). Each field is resolved by a port of `call_amount`/`tao_amount`/`alpha_amount`
  (`_money.py:51-165`):
  - **Unit**: the wrapped call argument's runtime type ident from the `CallDescriptor` (`TaoBalance`/`AlphaBalance`),
    else the extracted `_FALLBACK_AMOUNT_UNITS` (`:89-111`); for example, `swap_basket.min_amount_out` is a bare `u64`
    at spec 477.
  - **Expected netuid**: 0 for TAO; otherwise the sibling field named in `codegen/intent_types.toml`. `amount_alpha`
    maps to `netuid` for `remove_stake(_limit)`, `lock_stake` and `add_collateral`, and to `origin_netuid` for
    `move_stake`, `swap_stake`, `move_swap_stake` and `transfer_stake`; `min_alpha` maps to `netuid`;
    `swap_basket.amount` to `origin_netuid` and `swap_basket.min_amount_out` to `dest_netuid`; each `swap_basket_many`
    leg's `amount`/`min_amount_out` to its own `origin_netuid`/`dest_netuid`; `set_root_claim_threshold.threshold` and
    every `*_tao` field to TAO.
  - **Tagging**: `Untagged` is tagged with the expected netuid, so netuid 0 yields a TAO-denominated amount exactly as
    Python tags root alpha (`:93-98`). `Tagged` with another netuid fails with `UnitError::UnitMismatch` and Python's
    message (`:152-163`).

  The generated accessor `fn <field>_resolved(&self) -> Result<ResolvedAmount, IntentError>` performs this.
  `validate()` calls every accessor (Python normalizes in `__post_init__`, for example `RemoveStake`,
  `PY/intents/staking.py:701-704`), and `build()`, `summary()` and `spend()` use them. This is what makes the root
  branches work: `remove_stake`, `remove_stake_limit`, `move_stake`, `swap_stake`, `move_swap_stake` and
  `transfer_stake` on netuid 0 (`PY/intents/staking.py:306-345, 772-776, 914-916, 1102-1104`), and `swap_basket(_many)`
  legs that touch netuid 0. `Balance<Alpha>` (netuid ≠ 0) stays the framework's domain type, reached with
  `TryFrom<DynBalance>`.
- **Read models use `DynBalance`** because their JSON must be Python's `str(Balance)`, including the chain symbol that
  is tagged at decode time from the `TokenSymbol` map (7-day disk cache shared with btcli, `PY/config.py:22-27`).
  Typed code converts with `.tao()` or `.alpha()`.
- **`StakeWeight`** is typed separately so it cannot pass as TAO [J-PD graft 4]. Its JSON keeps Python's rendering,
  where v11 tags `total_stake` as an alpha `Balance` (`PY/metagraph.py:366`), so this is a typed-only difference
  (D-019).

### 5.3 Python's `str(Balance)`, byte for byte

Python renders `f"{TAO_SYMBOL}{self.amount:,.9f}"` (TAO) or `f"{self.amount:,.9f}{self.unit}"` (alpha), where
`amount = self.rao / RAO_PER_TAO` and `RAO_PER_TAO = 10**9` (`PY/balance.py:113-149`, `PY/settings.py:15` [V]).
`int / int` true division is correctly rounded to the nearest f64, so for amounts above 2^53 rao (about 9.0M TAO) the
text is *not* the exact decimal [J-CF concern 12]. Read JSON contains these strings through `_jsonable`, so rtao
emulates them exactly instead of narrowing the claim:

```rust
pub mod pyfmt {
    /// Python `rao / 10**9`: the correctly rounded f64 quotient (round-half-even), computed exactly in u128
    /// (no double rounding for rao ≥ 2^53).
    pub fn rao_to_f64(rao: u64) -> f64;
    /// Python `format(x, ",.9f")`: exact binary-to-decimal expansion of x rounded half-to-even at 9 places
    /// (u128 arithmetic suffices for 1e-9 ≤ x < 2^64), then ',' grouping of the integer part.
    pub fn fmt_comma_9f(x: f64) -> String;
    pub fn balance_str(rao: u64, netuid: NetUid, symbol: Option<&str>) -> String;   // τ prefix / unit suffix / α + subscript
    pub fn float_repr(x: f64) -> String;   // Python repr(float): shortest round-trip, 'e-05' exponent style, '.0' for integers
    pub fn fmt_g(x: f64) -> String;        // Python format(x, "g"): used in btauth messages ("{max_age:g}s")
}
```

`fmt_comma_9f` does not rely on Rust's `{:.9}` tie behavior; it is implemented from the exact binary value. The
pure-function oracle (§11.5) pins it with 10,000 `str(Balance.from_rao(r))` vectors over `0..=u64::MAX`: random values, values around 2^53,
21×10^15, ties at the tenth decimal (e.g. multiples of 2^-10), and zero. Human-readable *tables* (Rich in Python) are
not claimed byte-identical (§10.3).

### 5.4 Fixed point and ratios

| Type | Wire | Value | Used by |
|---|---|---|---|
| `PerU16(u16)` | u16 | x/65535 | take, incentive, dividends, consensus, kappa, `alpha_low`/`alpha_high` |
| `Permill(u32)`, `Perbill(u32)`, `Percent(u8)` | int | x/1e6, x/1e9, x/100 | `RATIO_TYPE_DENOMINATORS` (`PY/hyperparams.py:32-37`) |
| `PerMillion(u64)` | u64 | x/1e6 | `bonds_moving_avg` |
| `U64Normalized(u64)` | u64 | x/u64::MAX | child proportions (`PY/reads/delegation.py:156-166`), `tao_weight` |
| `U64F64 { bits: u128 }` | `{bits}` | bits/2^64 | `burn_increase_mult`, `collateral_drain_ratio`, basket pricing |
| `U96F32 { bits: u128 }` | `{bits}` | bits/2^32 | `RootProp`, `MinerBurned` (present in pinned v478 metadata) |
| `I96F32 { bits: i128 }` | `{bits}` | bits/2^32 | `moving_price`, `RootClaimableThreshold` (`bits >> 32` whole rao, `PY/reads/root.py:54-77`) |
| `I64F64 { bits: i128 }` | `{bits}` | bits/2^64 | `SubnetEmaTaoFlow` |
| `I32F32 { bits: i64 }` | `{bits}` | bits/2^32; `int_part()` = floor | `alpha_sigmoid_steepness` (`PY/reads/subnets.py:126-147`) |
| `Ratio(f64)` | — | Python float semantics | intent inputs like `rate_tolerance = 0.05` (Python computes limits in float, so rtao does too) |

- **Decoding.** Fixed-point types decode from the runtime's `{bits}` composite by field name. The fractional-bit count
  is checked against the typenum `Frac` path at codegen (§3.7).
- **`to_f64`.** `to_f64()` equals Python's `bits / 2**n`: the integer converts to f64 with one correct rounding, and the
  division by a power of two is exact. The value vectors compare with bit equality.
- **Display.** `Display` is exact decimal, and `to_f64()` is display math only.
- **Weights are the one place Python's float math reaches the chain.** They are handled in §9.3: `round_ties_even`,
  Python's operation order, and oracle vectors.
- **`ratio_fraction`.** `rtao_primitives::ratio::{ratio_fraction, RATIO_TYPE_DENOMINATORS}` are derived from these
  types and land in M1: `ratio_fraction("PerU16", x) = x / 65535` is the only form M1 and M2 need (`PY/metagraph.py:346`,
  `PY/reads/weights.py:73`, `PY/reads/delegation.py:25-28`). A catalog-layer test asserts equality with the extracted
  `RATIO_TYPE_DENOMINATORS` (`PY/hyperparams.py:32-37`). The helper rounds the exact integer quotient once, avoiding
  double rounding above 2^53; it neither clamps raw values nor normalizes unknown or differently cased identities.

### 5.5 Hyperparameter units

`PY/hyperparams.py` is split by layer, because a function in `rtao-primitives` (L2) cannot read a table generated into
`rtao-catalog` (L3):

- `rtao_primitives::ratio` (§5.4) and `rtao_primitives::hyperparams::proportion_to_raw` (`:581-621`) are table-free
  and land in M1 (M5's children intents use `proportion_to_raw`, `PY/intents/children.py:29, 80`).
  `proportion_to_raw` takes a u64 denominator and returns u128 so Python's rounded result remains exact:
  `1.0 * u64::MAX` rounds to 2^64, while raw integer u64::MAX remains unchanged. The chain encoder checks the
  final field width. `proportion_to_raw_with_label` supplies field-specific diagnostics; booleans are rejected.
- `rtao_primitives::hyperparams::{Hyperparam, HyperparamTable}` carry the ported functions of `:474-769` as methods on
  `&HyperparamTable`: `metadata_kind`, `kind_of`, `doc_of`, `short_of`, `normalized`, `annotate`, `value_forms`,
  `codec_of`, `raw_bounds` and `to_raw` (M4).
- The data is generated into `rtao_catalog::hyperparams::TABLE: HyperparamTable` from `HYPERPARAMS` (`:110-408`),
  `STORAGE_ITEMS` (`:416-461`, each item resolved at extraction to its storage value's type ident and cross-checked
  against the pinned metadata), `_IDENT_KINDS` (`:467-471`), `_IDENT_CODEC` (`:626-643`), `_KIND_CODEC`
  (`:646-657`) and `_FRACTION_DENOMINATOR` (`:83`).
- `rtao_intents::hyperparameters::to_raw(name, value)` delegates to `rtao_catalog::hyperparams::TABLE.to_raw`, so
  there is one implementation.

Python's precedence is kept. `kind_of` is metadata-primary: the unit-bearing type ident of the parameter's storage
value (from Python's generated descriptors, so from the pinned metadata) decides, and the hand table covers the rest
(`:474-500`). `codec_of` takes explicit `bits`/`signed`, then the storage value's type ident, then the kind default
(`:663-680`).

- **Input rule.** A value with a decimal point is the human form; a plain integer is the raw value.
- **No Rust change for new hyperparameters.** A new upstream hyperparameter needs only re-extraction.
- **`--units` gate.** `tests/hyperparam_units.rs` ports Python's `--units` gate (`codegen/check.py:472-507`): every
  extracted kind must agree with the metadata ident of its storage item.
- **No hand-transcribed table.** B's hand-transcribed `hyperparams.toml` is rejected [J-MA §3.2].

---

## 6. Wallets and signing (`rtao-wallet`, `rtao-signer`)

### 6.1 On-disk wallets (btcli-compatible, through the core)

- **Layout** is identical to Python's: `<path>/<wallet>/coldkey`, `coldkeypub.txt`, `hotkeys/<hotkey>`,
  `hotkeys/<hotkey>pub.txt`. The default path is `~/.bittensor/wallets` (`PY/wallet.py:11`). `BT_WALLET_PATH` is read
  only by the CLI's `--wallet-path` option (`PY/cli/main.py:203-206`); rtao's library `Wallet` reads no environment
  variable either, and `rtao-cli` and `NeuronConfig` (§9.9) read it. The compound form `"cold/hot"` parses with
  Python's validation messages (`PY/wallet.py:29-70`).
- **Loading.**
  1. Read the bytes and detect the format with the core's detectors (`$NACL`, `$ANSIBLE_VAULT`, `gAAAAA`;
     `CORE/keyfiles/mod.rs:29-47`).
  2. Plaintext files, which is how hotkeys are written by default, decode through `keyfile_json` (the staged copy of
     `deserialize_keypair_from_keyfile_data`) **without `host`**. It carries every btwallet quirk: the leading-space
     `" ss58Address"` key, `cryptoType` as number or string, the secret precedence phrase → seed → private key → ss58,
     a retry with the other crypto type on address mismatch, bare hex or mnemonic files, and the polkadot-js export
     hint (`keyfiles/mod.rs:216-370` [V]).
  3. Encrypted files need feature `encrypted` (core `host`): `decrypt_keyfile_data` (NaCl argon2i13 with a fixed salt,
     Ansible Vault, legacy Fernet with PBKDF2 at 10M iterations), then the same JSON decode. rtao implements none of
     these formats.
- **Creating and regenerating.**
  - `Keypair::generate_mnemonic(12|15|18|21|24)` → `from_mnemonic`, `from_seed` or `from_uri` →
    `serialized_keypair_to_keyfile_data`.
  - Coldkeys are encrypted with `encrypt_keyfile_data` (`$NACL`). Hotkeys are written without a password, as in Python
    (`PY/wallets.py:239-245`).
  - Writes go to a temp file, then fsync, then rename. Files are 0o600 and directories 0o700
    (`PY/keyfiles.py:128-163`; `PYT/unit/test_keyfiles_golden.py:121-140`).
  - Nothing is overwritten without `Overwrite::Yes`.
  - Polkadot-js JSON import uses `Keypair::from_encrypted_json` (`host`).
- **Unlock** happens once per process in `spawn_blocking` (Argon2i, 512 MiB, t = 8). The key lives in core `Zeroizing`
  buffers behind an `Arc`, mirroring Python's lazy `WalletSigner` (`PY/signing.py:143-224`). The coldkey address comes
  from `coldkeypub.txt` and never requires unlocking (`PY/intents/base.py:288-298`).

### 6.2 Password resolution (exactly `PY/wallets.py:62-118` [V])

```rust
pub struct PasswordSources {
    pub password: Option<SecretString>,      // 1. explicit (SDK, tests)
                                             // 2. BT_WALLET_PASSWORD (always consulted)
                                             // 3. BT_PW_<keyfile path, separators and dots → "_", uppercased>
                                             //    (PY/keyfiles.py env_var_name), decoded by core get_password_from_environment
    pub password_file: Option<PathBuf>,      // 4. argument, else BT_WALLET_PASSWORD_FILE (whole file, trimmed)
    pub keychain: bool,                      // 5. macOS Keychain, service "bittensor.coldkey", account = wallet name
    pub macos_prompt: bool,                  // 6. native macOS dialog (osascript)
    pub prompt: PromptPolicy,                // 7. Never (library default) | IfTty (CLI) | Always
}
pub enum PromptPolicy { Never, IfTty, Always }
pub async fn resolve_wallet_password(w: &Wallet, s: &PasswordSources) -> Result<Option<SecretString>, WalletError>;
```

- **Python's exact branch semantics** are kept. With `keychain` set and no stored item, the result is an error unless
  `macos_prompt` is also set, using Python's message pointed at `rtao wallet keychain save`.
- **Keychain access** shells out to `/usr/bin/security find-generic-password -s bittensor.coldkey -a <wallet> -w`,
  exactly as Python does (`PY/macos_password.py:58-80` [V]), with no shell. Items saved by `btcli wallet keychain save`
  (whose trusted application is `security`) are then readable by rtao without a second system prompt, and the reverse
  holds too. A native `security-framework` call would trigger an access prompt for a different binary [I].
- **The library never prompts.** `PromptPolicy::Never` is the library default and the CLI uses `IfTty` (divergence
  D-003, from A). Without a TTY the CLI refuses to prompt, as btcli does (`PY/keyfiles.py:42-62`). On an interactive
  wrong password the CLI re-prompts twice (`PY/cli/context.py:1900-1935`). Every other source fails fast with
  `WalletError::WrongPassword { source }`.
- **`rtao wallet save-password-env`** prints `export BT_PW_…=<encoded>` using the staged pure encoder (G10). It never
  calls `set_var`.
- **Secrets** are `secrecy::SecretString` and zeroized on drop. No `Debug` impl exposes them.
- **`keyring`** (Linux Secret Service) is an rtao-only source between 4 and 5, behind a feature and off by default.

### 6.3 The `Signer` contract

```rust
bitflags! { pub struct SignerCaps: u8 {
    const RAW_PAYLOAD   = 1 << 0;   // signs signature_payload bytes (keypairs, remote)
    const METADATA_HASH = 1 << 1;   // needs CheckMetadataHash=Enabled + RFC-0078 proof (MetadataVerifyingSigner: Ledger)
    const PAYLOAD_JSON  = 1 << 2;   // signs Polkadot-JS SignerPayloadJSON (ExtensionPayloadSigner: browser extension)
    const UNSIGNED_TX   = 1 << 3;   // takes the whole prepared extrinsic (UnsignedExtrinsicSigner: Vault, Ledger)
    const OUT_OF_BAND   = 1 << 4;   // human round trip (Vault QR): the executor prepares shield payloads before the first scan
}}
pub trait Signer: Send + Sync + 'static {
    fn account(&self) -> AccountId;
    fn crypto_type(&self) -> CryptoType;
    fn caps(&self) -> SignerCaps { SignerCaps::RAW_PAYLOAD }
    fn sign<'a>(&'a self, req: &'a SigningRequest<'a>) -> BoxFuture<'a, Result<MultiSignature, SignerError>>;
    /// Raw message signing (btauth/1, EVM association, `wallet sign`). Hardware signers refuse (no blind signing).
    fn sign_raw<'a>(&'a self, _msg: &'a [u8]) -> BoxFuture<'a, Result<[u8; 64], SignerError>> {
        Box::pin(async { Err(SignerError::RawSigningUnsupported) })
    }
}
pub struct SigningRequest<'a> {
    pub call: &'a [u8],
    pub included_in_extrinsic: &'a [u8],      // core signature_payload_parts().0 (era, nonce, tip, mode)
    pub included_in_signed_data: &'a [u8],    // core signature_payload_parts().1 (spec, tx, genesis, era hash, metadata hash)
    pub payload: &'a [u8],                    // core signature_payload() (blake2-256 above 256 bytes)
    pub payload_json: &'a SignerPayloadJson,  // PY/_transport/contract.py:87-132 quirks (BE hex at SCALE width)
    pub context: &'a SigningContext,          // raw V15 bytes, ChainInfo, genesis, spec/tx versions, ss58 format
    pub purpose: SignPurpose,                 // Extrinsic | ShieldInner | ShieldCarrier
}
pub struct MultiSignature { pub crypto: CryptoType, pub bytes: [u8; 64] }   // 65-byte replies split and variant-checked
```

`dyn Signer` with boxed futures is deliberate. Signers are heterogeneous runtime choices (`--ledger`, `--signer
extension`), they sign at most a few times per block, and `sign_raw` lets btauth/1 run on any capable signer. Dispatch
follows Python's capability order (`PY/_transport/extrinsics.py:321-347`): an unsigned-extrinsic signer first, then
the extension payload signer, then a plain `sign`.

### 6.4 Roles, key sources and key views

```rust
pub enum SignerRole { Coldkey, Hotkey }
pub trait Role: sealed::Sealed + Send + Sync + 'static { const KIND: SignerRole; }
pub enum Coldkey {}   impl Role for Coldkey { const KIND: SignerRole = SignerRole::Coldkey; }
pub enum Hotkey {}    impl Role for Hotkey  { const KIND: SignerRole = SignerRole::Hotkey; }

/// Attached public views, never unlock. Missing attached keys return KeyUnavailable(role).
pub trait KeyView: Send + Sync {
    fn hotkey_address(&self) -> Result<AccountId, SignerError>;
    fn hotkey_public_key(&self) -> Result<[u8; 32], SignerError>;
    fn coldkey_address(&self) -> Result<AccountId, SignerError>;
    fn bare_signer_account(&self) -> Option<AccountId> { None }
}
/// Compile-time role check (typed path).
pub trait KeySource<R: Role>: KeyView {
    fn address(&self) -> Result<AccountId, SignerError>;
    fn signer(&self) -> impl Future<Output = Result<Arc<dyn Signer>, SignerError>> + Send;   // may unlock lazily, once
}
/// Runtime role check (CLI, agents, execute_tool): Python's resolve_signer (PY/signing.py:227-263).
pub trait DynKeySource: KeyView {
    fn signer_for<'a>(&'a self, role: SignerRole) -> BoxFuture<'a, Result<Arc<dyn Signer>, SignerError>>;
}
/// Role-dependent defaults matching PY/intents/base.py:248-298.
pub struct IntentKeyView<'a> { /* keys: &'a dyn KeyView, signer_role: SignerRole, op: &'a str */ }
impl<'a> IntentKeyView<'a> {
    pub fn new(keys: &'a dyn KeyView, signer_role: SignerRole, op: &'a str) -> Self;
    pub fn hotkey_address(&self, override_address: Option<AccountId>) -> Result<AccountId, SignerError>;
    pub fn hotkey_public_key(&self) -> Result<[u8; 32], SignerError>;
    pub fn coldkey_address(&self) -> Result<AccountId, SignerError>;
}
```

`KeyView` describes attached wallet keys, so a bare signer returns `KeyUnavailable` from all three attached-key
accessors and exposes its own account through `bare_signer_account()`. `IntentKeyView` supplies the missing intent
role context: an explicit hotkey address wins before public I/O, an attached key wins next, and a bare identity is a
default only when its requested role matches the intent's signing role. A bare signer still signs either role through
`KeySource<R>` and `DynKeySource`; signing a coldkey intent does not attach a default hotkey. Missing-key diagnostics
match Python, while public-file errors propagate without a fallback or unlock.

`KeypairSigner` clones share one pinned-core `Keypair` through `Arc` until its final owner drops. The core separately zeroizes retained mnemonic and seed buffers. Rtao cannot wipe the opaque signing pair, and the pinned ed25519 pair has no guaranteed secret wipe on drop. Borrowed constructor inputs remain the caller's responsibility; secret-dependent constructor errors use fixed diagnostics after wiping the discarded owned message.
`Arc<S>` forwards the complete `Signer` contract. Bare `Clone + Signer` values implement both typed key-source roles
and dynamic selection, including a non-clone backend held as `Arc<dyn Signer>`. Wallet sources implement public views
and their supported role markers separately. Unknown dynamic role tokens return `SignerError::InvalidRole` before
signer acquisition.

| Type | `KeySource<Coldkey>` | `KeySource<Hotkey>` | Notes |
|---|---|---|---|
| `HotkeyFile` | **no** | yes | neuron runtimes get only this, so coldkey intents do not compile there; `coldkey_address()` reads `coldkeypub.txt` for views |
| `ColdkeyFile` / `Wallet::coldkey_only()` | yes | no | cold-wallet machines |
| `Wallet` | yes (unlock via §6.2; encrypted needs `encrypted`) | yes | CLI and owner tooling default |
| `KeypairSigner` (URI, mnemonic, seed, unlocked) | yes | yes | a bare signer signs either role, as in Python |
| `LedgerSigner`, `VaultSigner`, `ExtensionSigner`, `RemoteSigner` (M10) | yes | yes (bare signers) | normally coldkey |
| `KeyedWallet { coldkey, hotkey }` | if present | if present | mirrors Python's keyed wallet |

There are no GATs in these signatures. B's `type Signer<'a>` is replaced by `Arc<dyn Signer>` [J-PD §4 B].

### 6.5 Signer implementations

| Signer | Feature | Built on | Caps | Notes |
|---|---|---|---|---|
| `KeypairSigner` | always | `Keypair::sign` (`CORE/keys/mod.rs:391`; sr25519 `substrate` context, ed25519 RFC 8032) | RAW | dev URIs, mnemonics, unlocked keyfiles; inline signing |
| `LedgerSigner` | `ledger` | the core's `LedgerDevice` behind the `LedgerBackend` seam (below): `sign(account, index, call ‖ extra ‖ additional, proof)`; proof = `generate_extrinsic_proof(call, extra, additional, metadata_v15, chain_info)` (`CORE/digest/mod.rs:117`) | METADATA_HASH \| UNSIGNED_TX | ed25519 at `m/44'/354'/account'/0'/index'`; one device mutex across whole operations; `spawn_blocking`; Polkadot generic app ≥ 100.0.5 ([R] G §4.4); refuses raw signing; tested against Ledger's emulator (§11.9) |
| `VaultSigner` | `vault` | port of `PY/vault/uos.py` (UOS framing, multipart envelope) plus `prepare_call`/`submit_signature`; serves upstream `PY/vault/static/*` verbatim (embedded with `include_bytes!` from `vendor/`, Apache-2.0) from `VaultSessionServer`, a one-shot loopback server on an ephemeral port (`PY/vault/server.py:50-242`): `/` serves `vault.html`, `/bittensor-chain-specs.png` the chain-specs QR, and `/ws` is a WebSocket upgraded like the extension bridge's (axum, then `hyper::upgrade::on` and tokio-tungstenite at the workspace version, §2.3 rule 2); the page sends a hello with `role: "vault"` and the session id (a stale session is closed with 1008, a replaced tab with 1012), receives `sign-request`/`scan-request`/`result` and answers `signature`/`scanned`/`error`; UOS frames are rendered as SVG data URIs, byte mode without mode optimization, error correction M, 4-module quiet zone (`PY/vault/qr.py:15-28`); `scan_vault_address` parses `substrate:<ss58>:<genesis>` or a bare ss58 (`PY/vault/scan.py:21-49`); `vault::{VaultSessionServer, VaultError, VaultPageError, scan_vault_address}` mirror `PY/vault/__init__.py`'s `__all__` | UNSIGNED_TX \| OUT_OF_BAND | shielded submissions need two scans inside the 8-block era, so both payloads are prepared before the first scan |
| `ExtensionSigner` | `extension` | the v11 bridge protocol on `127.0.0.1:39295`, token in `~/.bittensor/extension_bridge.token`, serving upstream `PY/extension/static/bridge.html` verbatim (`PY/extension/bridge.py:17-40`) | PAYLOAD_JSON | wire-compatible with the Python daemon |
| `RemoteSigner` (M10) | `remote` | rtao addition: `POST /sign` with the `SigningRequest` minus secrets, authenticated by btauth/1 with a separate client key; it ships with its wire schema, a reference server that decodes and policy-checks the call before signing, and replay tests, and stays outside `full` until then (§14 M10) | any | HSM/KMS deployments |

Fee-free hotkey calls (weights, serving, `associate_evm_key`, `pow_register`, `set_commitment`) are `Pays::No`, so a
zero-balance hotkey can run a validator ([R] C1 §0 item 4). Fee-bearing hotkey calls on the runtime allow-list are
charged to the owning coldkey, and `plan` emits Python's `COLDKEY_FEE_WARNING` (§4.6).

Python's documentation claims hotkey-signed extrinsics are refused for shielding, but no such check exists in the code
([R] A1 §10 item 18). rtao follows the code and records the docs drift in `divergences.toml` as `kept`.

**Ledger.** `LedgerSigner` reaches the device through `LedgerBackend` (`app_version`, `address`, `sign`), an rtao-internal
trait that mirrors Python's `_device_cls` seam (`PY/ledger.py:44-51`; `PYT/unit/test_ledger_signer.py:51-54`), because
the core's `LedgerDevice` is a concrete type with a private HID transport (G16). Its implementations are the core's
`LedgerDevice::open()` in production, `FakeLedger` for the ports of `test_ledger_signer.py`, and, in `rtao-testkit`, the
core's own `LedgerDevice` built by `LedgerDevice::with_transport` over a Speculos TCP or replay transport, which rtao's
workspace takes from the core patch until PR-U17 ships (§1.11, §11.9); the trait and `LedgerSigner::with_backend` are
public only with `rtao-signer`'s feature `test-util` (§2.3 rule 8).

- The device receives the **unhashed** payload `call ‖ included_in_extrinsic ‖ included_in_signed_data`, never
  `SigningRequest.payload`, which is hashed above 256 bytes; the app hashes long blobs itself
  (`DOTAPP/app/src/crypto.c:119-126`). The 65-byte reply becomes `MultiSignature { Ed25519, … }`.
- One device mutex covers whole operations: a signature's INIT…LAST sequence, and the address and version calls. The core
  locks per APDU only (`CORE/signers/hid.rs:36-43, 74-83`), while the app resets a signing in progress on any address
  request and refuses every APDU while a review is pending (`DOTAPP/app/src/apdu_handler.c:43-44, 268-270`).
- `ChainInfo` is filled as Python fills it (§3.9.2); `decimals` and `token_symbol` are `LedgerSigner` options, 9 and
  `"TAO"` by default. The app's 16 KiB buffer must hold payload ‖ proof (`DOTAPP/app/src/common/tx.c:36-37`); a larger
  request gets the device's `0x6983` error, as in Python.
- Device errors keep the core's messages (`CORE/signers/ledger.rs:169-219`), which upstream's troubleshooting guide
  quotes (`UP/docs/guides/ledger.mdx:100-110`).
- Raw signing stays refused, as in Python (`PY/ledger.py:165-175`): the app's `SIGN_RAW` signs only `<Bytes>`-wrapped
  data (`DOTAPP/app/src/common/tx.c:74-104`), the core does not implement it, and rtao never produces the wrapped form
  (§7.1, Q4), so the §6.3 comment "Hardware signers refuse" stays correct.
- A shielded submission needs two approvals inside the 8-block shield era, as in Python (`PY/cli/context.py:316-320`).

**Extension bridge.** `BridgeServer`, `BridgeClient`, `ExtensionSigner`, `connect_extension_signer`, `ensure_bridge`,
`run_bridge` and `stop_bridge_daemon` are Python library API (`PY/__init__.py:47-57, 198-206`), so all of them live in
`rtao-signer` under feature `extension`. The bridge is an HTTP and WebSocket server on port 39295
(`PY/extension/bridge.py:14, 19, 235`) and a WebSocket client (`PY/extension/client.py:11`). The server serves
`bridge.html` with axum and upgrades its WebSocket route through `hyper::upgrade::on` plus
`tokio_tungstenite::WebSocketStream::from_raw_socket`, at the workspace tokio-tungstenite version (§2.3 rule 2); the
client uses tokio-tungstenite directly. `ensure_bridge` spawns `rtao extension bridge --daemon --pid-file …` (the
binary from `RTAO_BIN` or `PATH`) instead of Python's `python -m bittensor.extension.daemon`
(`PY/extension/session.py:121-140`). It writes the same token and pid files (`BITTENSOR_EXTENSION_BRIDGE_TOKEN`,
`PY/extension/tokens.py:14`; `BITTENSOR_EXTENSION_BRIDGE_PID`, `PY/extension/session.py:24`) and connects to an
already running Python daemon on port 39295. `rtao-cli`'s `extension bridge` command only runs the server.

### 6.6 EVM keys and helpers (`rtao-evm`, parity with `PY/evm/*`, 1,243 lines)

- **Addresses and conversions:**
  - `h160_to_ss58` is `blake2_256("evm:" ‖ h160)` (`PY/intents/evm.py:26-33`). It lives in
    `rtao_primitives::evm`, so the EVM intents need no `rtao-evm` dependency;
  - `ss58_to_h160_truncated`;
  - wei/rao conversions (18 versus 9 decimals);
  - chain ids 964 and 945 (`PY/evm/networks.py`, extracted).
- **Keys**: keystore-V3 EVM keys stored next to the wallet.
- **Contracts**: the precompile catalog with ABIs embedded from `vendor/upstream/evm/abi/*.json` (vendored at
  `parity_tag`), and a minimal eth JSON-RPC client (reqwest).
- **Association proof**: the EIP-191 signature over `hotkey_pubkey ‖ keccak256(scale(u64 block))`
  (`PY/intents/association.py:44-95`).
- **Dependencies**: `alloy-primitives`, `alloy-sol-types`, `alloy-json-abi`, `alloy-signer-local`. They stay behind
  feature `evm` and out of neuron graphs.

---

## 7. Neuron networking (`rtao-btauth`)

v11 removed Axon, Dendrite and Synapse. Identity is btauth/1 signed HTTP; blacklisting is a metagraph lookup; priority
is rate limiting keyed by the verified hotkey (`UP/docs/migration.mdx:455-480`, [R] G §6.1). The normative spec is
`UP/docs/guides/signed-requests.mdx` (vectors at `:252-289`). The reference implementation is `PY/http_auth.py`
(369 lines). rtao implements it byte for byte and reuses the core for the cryptography.

### 7.1 Core (no I/O, no tokio)

```rust
pub const PROTOCOL: &str = "btauth/1";  pub const VERSION: &str = "1";  pub const NO_RECEIVER: &str = "-";
pub const DEFAULT_MAX_AGE: Duration = Duration::from_secs(10);         // PY/http_auth.py:68
pub const DEFAULT_ALLOWED_SKEW: Duration = Duration::from_secs(2);     // :69
pub const HEADER_VERSION: &str = "X-Bittensor-Version";   pub const HEADER_HOTKEY: &str = "X-Bittensor-Hotkey";
pub const HEADER_RECEIVER: &str = "X-Bittensor-Receiver"; pub const HEADER_NONCE: &str = "X-Bittensor-Nonce";
pub const HEADER_CRYPTO: &str = "X-Bittensor-Crypto";     pub const HEADER_SIGNATURE: &str = "X-Bittensor-Signature";

/// Exactly eight UTF-8 lines joined by '\n', no trailing newline (http_auth.py:157-185).
pub fn build_payload(scheme: CryptoType, method: &http::Method, target: &str /* path+query as sent, percent-encoded */,
                     body: &[u8], nonce_ns: u64, sender: &AccountId, receiver: Option<&AccountId>) -> Vec<u8>;
/// Fresh nonce per call; retries MUST re-sign (signed-requests.mdx:101-102).
pub async fn sign(signer: &dyn Signer, method: &http::Method, target: &str, body: &[u8],
                  receiver: Option<&AccountId>, now: &dyn Clock) -> Result<http::HeaderMap, AuthError>;

pub struct VerifyConfig {
    pub max_age: Duration,              // 10 s
    pub allowed_skew: Duration,         // 2 s
    pub require_receiver: bool,         // true
    pub wrapped: WrappedPolicy,         // AcceptLikePython (default) | Strict
}
pub enum WrappedPolicy { AcceptLikePython, Strict }
pub struct Verifier<S: NonceStore + ?Sized> { me: AccountId, cfg: VerifyConfig, store: Arc<S> }
impl<S: NonceStore + ?Sized> Verifier<S> {
    /// Fails if store.retention() <= max_age + allowed_skew (D-009/D-038).
    pub fn new(me: AccountId, cfg: VerifyConfig, store: Arc<S>) -> Result<Self, ConfigError>;
    /// Steps 1-4 below (shape, receiver, freshness, signature); touches no store.
    pub fn authenticate(&self, headers: &http::HeaderMap, method: &http::Method, target: &str, body: &[u8],
                        now_ns: u64) -> Result<Authenticated, AuthError>;
    /// authenticate, then commit: Python's order (http_auth.py:242-343).
    pub async fn verify(&self, headers: &http::HeaderMap, method: &http::Method, target: &str, body: &[u8],
                        now_ns: u64) -> Result<Caller, AuthError>;
}
pub struct Authenticated { /* the verified Caller, not yet checked against the replay store */ }
impl Authenticated {
    /// Step 5: the replay store (ReplayedRequest on a reused nonce).
    pub async fn commit(self, store: &dyn NonceStore) -> Result<Caller, AuthError>;
}
pub struct Caller { pub hotkey: AccountId, pub nonce_ns: BigInt, pub crypto: CryptoType }
/// Display strings are Python's verbatim (http_auth.py:274-341); they are the de-facto wire contract ([R] D §8.1).
pub enum AuthError { MalformedAuth(String), WrongReceiver(String), StaleRequest(String), ReplayedRequest(String), BadSignature(String) }
/// Async (D-005): a Redis store must not block a tokio worker. D's synchronous `check_and_store` is rejected [J-PD concern 7].
pub trait NonceStore: Send + Sync {
    fn retention(&self) -> Duration;
    fn check_and_store<'a>(&'a self, hotkey: &'a AccountId, nonce_ns: &'a BigInt) -> BoxFuture<'a, Result<bool, StoreError>>;
}
pub struct InMemoryNonceStore {
    /* sharded set + expiry wheel; retention 60 s (http_auth.py:135). Bounded per hotkey (max_per_hotkey, default
       ceil(limiter rate × retention) + burst) and globally (max_entries). At a bound it answers StoreError::Full,
       which the server maps to 503 and counts in rtao_btauth_nonce_store_full_total; it never evicts, because
       eviction reopens replays. Python's store is unbounded (http_auth.py:126-154). */
}
```

HTTP nonce text follows Python's arbitrary signed integer parsing and canonical decimal payload formatting.
Verifier freshness checks use that integer before signature/replay processing; authenticated callers and replay keys
retain it exactly. The normal signing clock and `build_payload` API use checked u64 Unix nanoseconds. This protocol
nonce is independent of Q25's native chain balance range. UTF-8 header values accept Unicode decimal digits as
Python's `int` does, normalized with maintained Unicode category data; malformed text remains rejected.

**Exact payload bytes:**

```
btauth/1
<sr25519|ed25519>                      ← re-derived from the parsed crypto type, never echoed from the header
<METHOD, uppercase>
<request target: path + "?" + query exactly as on the wire, percent-encoded>
<sha256(raw body), lowercase hex>       ← e3b0c442…b855 for an empty body
<nonce: unix ns, decimal>
<sender ss58>
<receiver ss58, or "-">
```

**Headers**: `X-Bittensor-Version: 1`, `X-Bittensor-Hotkey`, `X-Bittensor-Receiver` (when bound),
`X-Bittensor-Nonce`, `X-Bittensor-Crypto: ed25519` (omitted for sr25519) and `X-Bittensor-Signature: 0x<64-byte hex>`.
Names are case-insensitive. `X-Bittensor-Drand-*` names are reserved and never emitted or interpreted ([R] D §1.1).

**Verification order** is normative (`http_auth.py:242-343` [V], `signed-requests.mdx:78-99`):

1. **Shape.** Version must be `"1"`; then hotkey, nonce (decimal), crypto (default sr25519; `SR25519` and `1`
   accepted) and signature (hex after an optional `0x`). Each failure is `MalformedAuth` with Python's message, for
   example `"missing X-Bittensor-Nonce"` or `"X-Bittensor-Signature is not hex"` (`:274-298`).
2. **Receiver.** If absent while `require_receiver` is set, `WrongReceiver("missing X-Bittensor-Receiver: this server
   requires receiver-bound requests")`. If present but not ours, `WrongReceiver("request was signed for a different
   receiving hotkey")` (`:300-307`).
3. **Freshness**, in integer nanoseconds:
   - `now − nonce > max_age` gives `StaleRequest("nonce is older than the {max_age:g}s freshness window")`;
   - `nonce − now > skew` gives `StaleRequest("nonce is more than {allowed_skew:g}s in the future")` (`:310-313`).
   Python compares `int > float`, which is exact, so integer comparison is equivalent. `{:g}` is rendered by
   `pyfmt::fmt_g`.
4. **Signature.** Rebuild the payload from the **received** method, target, body and receiver (`-` when absent) and
   verify under the declared scheme only:
   - `AcceptLikePython` calls `rtao_core::keys::verify`, the same function Python's verifier calls
     (`PY/http_auth.py:55` → `PY/sp_core.py:16` → `UP/sdk/bittensor-core-py/src/keys.rs:189-193` →
     `CORE/keys/mod.rs:457-469` [J-CF §2]). It accepts a raw signature or one over `<Bytes>payload</Bytes>`, so the
     acceptance sets are identical by construction.
   - **A core `Err`** (undecodable ss58, wrong signature length, `keys/mod.rs:79-100` [V]) becomes exactly
     `MalformedAuth("invalid hotkey address or signature encoding")`, as `http_auth.py:324-328` maps `ValueError`
     [J-CF concern 6]. `Ok(false)` becomes `BadSignature("signature does not verify against the claimed hotkey")`.
   - `Strict` verifies once, without the wrapper, through the staged `verify_strict` (G11; PR-U10). Python and the
     upstream docs allow other implementations to reject the wrapped form (`signed-requests.mdx:239-243`).
5. **Replay store**, consulted only after the signature verifies, so unauthenticated traffic cannot poison it:
   `ReplayedRequest("this nonce was already accepted from this hotkey")` (`:341`).

**Retention check (D-009).** Python raises `ValueError` *after* a verified signature when
`max_age + allowed_skew > store.retention` (`:333-339` [V]), and skips the check when the store has no `retention`.
rtao's `NonceStore` always declares `retention()`. The check moves to `Verifier::new` and returns `ConfigError`
with a diagnostic that explains the required strict retention margin, so a misconfigured verifier never serves a request.
D-038 additionally rejects equality: freshness accepts the endpoint while the replay store
expires records at equality. Retention must exceed the entire accepted nonce lifetime.
This configuration-time behavior is registered [J-CF concern 6].

**rtao never produces** the `<Bytes>` wrapper. Verification accepts it by default (`AcceptLikePython`), as the user
decided (§15.2 Q4): an rtao miner then accepts every request a Python miner accepts. `Strict` stays available for
operators who want to reject the wrapped form.

**Golden vectors** come from `PYT/unit/test_http_auth.py:19-36` and `signed-requests.mdx:252-289`, extracted into
`vendor/upstream/btauth_vectors.json`:

- sender `//Alice`, receiver `//Bob`, nonce `1752076800000000000`, body `{"prompt": "hello"}`,
  `POST /generate?stream=false`;
- the exact sr25519 payload, plus sign→verify, because sr25519 signatures are randomized;
- the ed25519 address `5FA9nQDVg267DEd8m1ZypXLBnvN7SFxYwV7ndqSYGiN9TTpu` with the pinned signature
  `0x1a6b6319…f704`.

Every Python negative case is mirrored by name: lowercase headers, crypto omitted for sr25519, `SR25519`/`1` spellings,
stale at +11 s, future at −3 s, replay, `retention = 0` misconfiguration, tampered body, method and path, each missing
header, `Nonce=not-a-number`, `Crypto=ecdsa`, `Signature=0xnothex`, `Version=2`. The upstream policy carries over: a
change that breaks a vector needs `btauth/2`, not a fixture edit (`test_http_auth.py:3-6`).

### 7.2 Server adapter (feature `server`: axum 0.8, tower 0.5, http 1)

```rust
pub struct BtAuthLayer { verifier: Arc<Verifier<dyn NonceStore>>, policy: Option<Arc<dyn CallerPolicy>>,
                         limiter: Option<Arc<PerHotkeyLimiter>>, body_limit: usize /* default 4 MiB [I] */ }
impl<S> tower::Layer<S> for BtAuthLayer { type Service = BtAuthService<S>; /* … */ }
pub struct VerifiedCaller(pub Caller);                       // axum extractor (request extensions)
pub trait CallerPolicy: Send + Sync {
    fn admit<'a>(&'a self, c: &'a Caller) -> BoxFuture<'a, Result<Admission, PolicyDenied>>;   // Allow { priority } | Deny
}
pub struct MetagraphPolicy {                                  // "authentication says who; policy decides" (migration.mdx:462-471)
    pub metagraph: MetagraphHandle,                           // ArcSwap<Metagraph>: lock-free load, O(1) by_hotkey (§8.4)
    pub require_registered: bool, pub require_validator_permit: bool, pub min_stake: Option<StakeWeight>,
}
pub struct PerHotkeyLimiter { /* keyed token buckets; optional stake-weighted priority under contention */ }
#[cfg(feature = "redis")] pub struct RedisNonceStore { /* SET btauth:{hotkey}:{nonce} 1 NX PX <retention> (signed-requests.mdx:97-99) */ }
```

1. Buffer the body up to `body_limit` **before** any extractor runs (413 above it). The hash needs the raw bytes, so
   streaming request bodies cannot be verified, and that is documented. Responses may stream.
2. The target is `req.uri().path_and_query()` verbatim, in percent-encoded wire form. It is never normalized, and a
   trailing `?` is never stripped.
3. **Authenticate** (`Verifier::authenticate`, steps 1-4 of §7.1). Every `AuthError` becomes **401** with
   `{"detail": "<python message>"}`, the FastAPI recipe's shape (`signed-requests.mdx:134-135`).
4. **Policy** (`CallerPolicy::admit`). A denial (verified but unwelcome) is **403**.
5. **Commit** (`Authenticated::commit`, the replay store). A replay is **401** (`ReplayedRequest`); a full store is
   **503**.
6. **Limit** (`PerHotkeyLimiter`). A rate-limit hit is **429**.
7. On success, `Caller` and the priority go into the request extensions, and the buffered body is re-injected.

Any freshly generated keypair passes signature verification, so committing before the policy would let unregistered
callers fill the replay store at tens of thousands of signatures per second. Committing after the policy means only
admitted hotkeys consume store capacity. The limiter runs after the commit because axon traffic is plain HTTP: a
captured request of a legitimate validator can be replayed inside the 10 s window, and with the limiter first each
replay would spend that validator's tokens before being rejected. The upstream recipe applies policy after `verify`
(`signed-requests.mdx:143-156`), so a replayed request from a hotkey the policy denies gets 403 here where it would get
401 there (D-032); `Verifier::verify` keeps Python's order.

The runtime wraps a miner router as `router.layer(axum::extract::DefaultBodyLimit::max(cfg.body_limit))
.layer(BtAuthLayer { .. })`, with `BtAuthLayer` outermost. axum's `Json`, `Bytes` and `String` extractors otherwise
apply their own 2 MiB limit when no `DefaultBodyLimit` is set (`REG/axum-core-0.5.6/src/ext_traits/request.rs:316-327`),
so a 2-4 MiB body would pass verification and then fail with 413 in the handler. A test posts a 3 MiB signed body to a
`Json<T>` handler.

**`Json<T>`.** `rtao_btauth::server::Json<T>` is an axum extractor with `axum::Json`'s rejection shape that parses the
buffered body through `serde_json::Value` first (`rtao_primitives::json::from_slice`). The core turns on
`serde_json/arbitrary_precision` for the whole build (§2.7), so `#[serde(untagged)]` and `#[serde(flatten)]` request
types that carry non-integer numbers fail to parse directly from text with `axum::Json<T>`. The template and the miner
guide use `rtao::btauth::server::Json<T>`, and a test parses an untagged float payload through both extractors.

### 7.3 Client adapter (feature `client`: reqwest 0.13 with the preconfigured ring rustls config of §2.7)

```rust
pub struct SignedClient { http: reqwest::Client, signer: Arc<dyn Signer>, clock: Arc<dyn Clock> }
impl SignedClient {
    pub fn request(&self, method: http::Method, url: Url, receiver: &AccountId) -> SignedRequestBuilder<'_>;  // .json/.body/.timeout/.send
    /// Results as they arrive; `fan_out` collects this stream.
    pub fn fan_out_stream<Q, R>(&self, targets: impl IntoIterator<Item = AxonTarget>,
        make: impl Fn(&AxonTarget) -> (http::Method, String, Q) + Send + 'static, cfg: FanOut)
        -> impl Stream<Item = (Uid, Result<R, FanOutError>)> + Send + '_
        where Q: Serialize, R: DeserializeOwned;
    pub async fn fan_out<Q, R>(&self, targets: impl IntoIterator<Item = AxonTarget>,
        make: impl Fn(&AxonTarget) -> (http::Method, String, Q) + Send + 'static, cfg: FanOut)
        -> Vec<(Uid, Result<R, FanOutError>)> where Q: Serialize, R: DeserializeOwned;
}
pub struct FanOut {
    pub concurrency: usize,          // 256 [I]; beyond it requests queue (the default MaxAllowedUids is 256, runtime/src/lib.rs:916)
    pub connect_timeout: Duration,   // 3 s [I]: unreachable axons fail fast and free their slot
    pub timeout: Duration,           // 12 s per attempt, the v10 dendrite default ([R] D §2.1)
    pub deadline: Option<Duration>,  // Some(timeout + 1 s), measured from the call; covers queueing and retries;
                                     // unfinished targets resolve to FanOutError::Deadline
    pub retries: u8,                 // 0; re-signs each attempt, only while the deadline allows
}
// rtao_primitives::axon (re-exported here): AxonTarget { uid: Uid, hotkey: AccountId, base_url: Url } and
// axon_base_url(ip: u128, ip_type: u8, port: u16) -> Option<Url>: http://ip:port or http://[v6]:port, None when
// ip == 0 (port of PY/metagraph.py:563-573). rtao_runtime::models::AxonInfo::base_url() and
// Metagraph::axon_targets() call it, so rtao-btauth's `client` feature needs no chain crate.
```

With a per-request timeout and a concurrency cap but no call-level deadline, a fan-out would take
`ceil(n / concurrency) × timeout × (retries + 1)` when axons hang: 48 s at 256 UIDs with the old 64-slot default.
Upstream's recipe gathers every query at once (`signed-requests.mdx:199-200`), so its wall time is about one timeout.
With these defaults the wall time is bounded by `deadline` for any number of targets, and a validator can score early
replies from `fan_out_stream`.

The body is materialized before signing. The target is `url.path()` plus `?` plus `url.query()`, which reqwest keeps
percent-encoded. **Every attempt re-signs with a fresh nonce.** Retries live above the signing step, because a
transport-level retry of an already-signed request is rejected as a replay (`signed-requests.mdx:101-102, 163-167`).

### 7.4 Legacy v10 bridge (feature `legacy-v10`)

`legacy::verify_v10(headers, body_hash: &str) -> Result<AccountId, AuthError>` is a line-for-line port of upstream's
documented bridge recipe (`UP/docs/guides/signed-requests.mdx:291-321`). It checks the sr25519 signature over
`"{nonce}.{sender}.{receiver}.{uuid}.{body_hash}"` from the `bt_header_*` headers. The subnet supplies `body_hash`,
because it depends on the schema. As in the recipe, freshness is the caller's job. Full v10 Synapse emulation, Epistula
and QUIC are outside the v11 parity target. They are post-parity optional crates (§17).

---

## 8. Metagraph and typed models (`rtao-runtime::models`, `rtao-metagraph`)

### 8.1 Typed models: by-name `DecodeAsType` over the core's registry

Upstream publishes no return types as data: the catalog has names and parameters only. Read results are therefore
hand-written models, kept cheap to maintain by four rules:

1. **Field names equal the runtime struct's field names**, taken from the `freeze_struct`-pinned `rpc_info` structs
   ([R] A2 §4.2). Models derive `scale_decode::DecodeAsType` and decode against the core's `Runtime.types`, which is a
   `scale_info::PortableRegistry` from the shared scale-info 2.11.6. `scale-type-resolver 0.2` provides
   `TypeResolver for PortableRegistry` behind its `scale-info` feature, which rtao enables explicitly
   (`REG/scale-type-resolver-0.2.0/src/portable_registry.rs:59`; [J-MA §3.3]).
2. **Appended fields cost nothing.** The derive collects named fields into a map and looks each target field up by name.
   Extra input fields are ignored, and only a removed or renamed field fails, with `CannotFindField`
   (`REG/scale-decode-derive-0.16.2/src/lib.rs:222-236, 356-371` [J-PD §3 A#3]). This is why by-name decoding, not B's
   positional codec types, is the default typed path [J-PD concern 10].
3. **Verified twice.**
   - Each model carries a **projection shape hash** (§3.7) over its declared fields. `verify_surface` checks it per live
     spec, and on mismatch the read falls back to the dynamic path.
   - `xtask regen` runs `check-models` against the pinned metadata in every drift regeneration.
   - The runtime's `freeze_struct` hashes (`metagraph.rs:14,119`) are extracted with `syn` as tripwires (§4.9).
4. **Typed equals dynamic, by property test**, per committed metadata file. A `proptest` strategy generates SCALE bytes
   from the registry. It is a port of upstream's `ByteGen` (`UP/sdk/python/scripts/record_shape_corpus.py:47-130`):
   compact widths, every enum variant, sequences of length 0-3, depth ≤ 8. For each model the test asserts that typed
   decoding succeeds iff `Runtime::decode_id` succeeds, and that the typed value projected to `Value` equals the dynamic
   `Value` [J-MA G4, J-CF §3.1].

Models (`crates/rtao-runtime/src/models/`):

- metagraph and neurons: `MetagraphWire`, `SelectiveMetagraphWire`, `NeuronInfoLite`, `NeuronInfo`;
- subnet economics: `DynamicInfo`, `SubnetInfoV2`, `SubnetState`, `SimSwapResult`, `SubnetPrice`;
- hyperparameters: `HyperparamEntry`/`HyperparamValue` (v3; forward-compatible name/value pairs, unknown names kept as
  `Other`);
- stake and delegation: `StakeInfo`, `StakeAvailability`, `DelegateInfo`;
- serving and identity: `AxonInfo` (u128 ip, `ip_type` 4/6, port, opaque protocol byte with default 4), `PrometheusInfo`,
  `NeuronCertificate`, `ChainIdentityV2`, `SubnetIdentityV3`;
- locks and collateral: `LockState`, `MinerCollateralState`;
- commitments, leases, crowdloans: `CommitmentRegistration` + `Data`, `SubnetLease`, `CrowdloanInfo`;
- proxies and multisig: `ProxyDefinition`, `MultisigInfo`;
- baskets: `BasketSummary`, `BasketPosition`, `BetaPricing`.

Dead fields are `#[deprecated]` on the typed models and render as Python renders them (0.0 or zero balances) in JSON:

- `rank`, `trust`, `pruning_score` (`metagraph.rs:782,803-804`);
- `subnet_emission`, `pending_root_emission`, `tao_dividends_per_hotkey` ([R] C2 §0.4).

### 8.2 The metagraph: three decode paths, one projection

`client.subnets().metagraph(netuid)` ports `fetch` (`PY/metagraph.py:238-291`). It is
`rtao_metagraph::SubnetsMetagraphExt`, implemented for `rtao_reads::Subnets<'_, V>` and exported from `rtao::prelude`
under feature `metagraph` (§4.8), with two methods: `metagraph(&self, netuid: NetUid) -> Result<Option<Metagraph>,
ReadError>` (commitments included, Python's default) and `metagraph_with(&self, netuid: NetUid, o: MetagraphOptions)
-> Result<Option<Metagraph>, ReadError>`. The commitment machinery it uses lives in `rtao_reads::commitments` (§4.8),
because the catalog `commitments` read needs it too and `rtao-metagraph` sits above `rtao-reads`. The steps:

1. Pin the view.
2. Call `SubnetInfoRuntimeApi.get_metagraph(netuid)`, which is one RPC.
3. With `commitments = true` (Python's default), also scan `Commitments.CommitmentOf[netuid]` and
   `Commitments.RevealedCommitments[netuid]` **at the same block**.
4. Return `None` when the subnet does not exist. Mechanisms use `get_mechagraph(netuid, mecid)`, whose `netuid` field
   is the storage index (`NetUidStorageIndex::split`, [R] C2 §0.5).

The response is parsed by the fastest path whose shape gate passes:

| Path | Gate | 256 UIDs | 2,500 UIDs | Role |
|---|---|---:|---:|---|
| **`MetagraphView`** (B's borrowed columnar parser; fixed-width columns borrowed from the raw bytes, compact columns decoded once) | **exact** shape hash of `get_metagraph`'s output type at the live spec equals the parser's committed hash | 6.5 µs | 57 µs | default |
| `MetagraphWire: DecodeAsType` (by-name owned model) | projection hash | 1,380 µs | 11,001 µs | fallback after a shape change; off the reactor above 64 KiB (§3.1) |
| dynamic `Value` + `from_dynamic` (a line-for-line port of Python's `_build` over dict-shaped values, `PY/metagraph.py:297-403`: `column()` padding, `PerU16 → f64`, `I96F32` price, collateral `None` before spec 435) | always works | 330 µs | 2,683 µs | last resort and the **reference** for differential tests |

(Timings from `SP/design-bench/bench-run-3.txt` [V], with `ss58` rendering of all keys at 158 µs and 1,273 µs.)

```rust
pub struct Metagraph {
    raw: bytes::Bytes,                         // the SCALE response, kept (persistence, re-decode)
    cols: Columns,                             // Borrowed { offsets into raw } | Owned { decoded vectors } — same accessors
    pub header: MetagraphHeader,               // netuid, mechid, name, symbol, identity, owners, block, tempo, last_step,
                                               // blocks_since_last_step, num_uids, max_uids, pool terms, hyperparameter fields
    pub at: SnapshotInfo,                      // number, hash, spec_version
    by_hotkey: OnceLock<HashMap<AccountId, Uid>>,   // O(1) caller lookup, built once per refresh (B)
    pub commitments: Option<CommitmentMap>,    // rtao_reads::commitments: uid → NeuronCommitment (+ unregistered by hotkey),
                                               // status machine and decoders of PY/metagraph.py:37-125, 406-560
}
impl Metagraph {
    pub fn len(&self) -> usize;
    pub fn neuron(&self, uid: Uid) -> Option<NeuronRef<'_>>;
    pub fn by_hotkey(&self, hk: &AccountId) -> Option<NeuronRef<'_>>;   // Python scans linearly; rtao indexes
    pub fn hotkey(&self, uid: Uid) -> Option<&AccountId>;               // borrowed, no copy
    pub fn validators(&self) -> impl Iterator<Item = NeuronRef<'_>>;
    pub fn axon_targets(&self) -> impl Iterator<Item = AxonTarget> + '_;  // rtao_primitives::axon (§7.3); ip == 0 means
                                                                         // not serving; IPv6 → [addr]:port
    pub fn to_owned_neurons(&self) -> Vec<MetagraphNeuron>;              // Python's MetagraphNeuron rows (PY/metagraph.py:127-161)
    pub fn to_json(&self) -> serde_json::Value;                          // == Python's Metagraph JSON (oracle-checked)
}
```

- **Safety.** The view parser is bounds-checked and returns `Result`; the benchmark's `assert!`/`unwrap` are not
  carried over. It is a `cargo fuzz` target (§11.7).
- **Differential test.** It runs on every recorded payload:
  - golden-461 `runtime_calls`;
  - the committed finney netuid 1, 3 and 64 payloads plus the synthetic 2,500-UID one (`vendor/bench/`, B's data);
  - current-spec localnet recordings.
  It asserts that all three paths produce identical `to_json()` [J-CF graft 13].
- **Opt-out.** `MetagraphView` can be switched off at run time through `RTAO_METAGRAPH_VIEW=0`,
  `ClientConfig::metagraph_view = false`, or the per-read `MetagraphOptions::view = false`.
  The connection setting propagates through `View::metagraph_view_enabled()` and its `DynView` twin to clones,
  snapshots and repinned dynamic views. A disabled view rejects an explicit `DecodePreference::View` request before
  the graph RPC; automatic reads use the typed or dynamic fallback.

**Selective fetches** (an rtao addition outside the parity matrix): `get_selective_metagraph` and
`get_selective_mechagraph` with the `SelectiveMetagraphIndex` enum generated from pallet source (§4.1). They decode into
`SelectiveMetagraphWire`, all fields `Option`, by name. Presets come from [R] C2 §5.2:

- `Preset::Validator` = `[Block, Tempo, LastStep, BlocksSinceLastStep, WeightsVersion, WeightsRateLimit,
  ActivityCutoff, CommitRevealWeightsEnabled, CommitRevealPeriod, NumUids, Hotkeys, Axons, Active, ValidatorPermit,
  LastUpdate, Emission, Dividends, Incentives, BlockAtRegistration, TotalStake]`, about 55-60% smaller than a full
  metagraph with a single stake pass;
- `Preset::Miner` = `[Hotkeys, ValidatorPermit, TotalStake, Validators]`.

`get_all_metagraphs`, `get_subnet_state` and `get_delegates` are never called in loops; they cost megabytes or
O(subnets × n) storage reads ([R] C2 §9 item 7).

**Weights and bonds** are sparse rows (`SparseRows { row_ptr, cols, vals }`), normalized on request as v11 does
(weights to fractions, bonds as value/65535; [R] A2 §2). They are never a dense n×n matrix, which would be 6.25M cells
for a 2,500-UID subnet.

### 8.3 Sync service (C)

```rust
pub struct SyncPolicy {
    pub cadence: SyncCadence,            // EveryEpoch { fallback_blocks } | EveryBlocks(u32) | Interval(Duration)
    pub preset: Preset,                  // Full | Validator | Miner | Custom(Vec<SelectiveMetagraphIndex>)
    pub commitments: bool,               // false in neuron presets; true for Full (Python default)
    pub early_on_events: bool,           // NeuronRegistered / AxonServed / HotkeySwapped* for this netuid → debounced refresh
    pub reconnect_after_failures: u32,   // 3 (SN2, [R] G §2.4 item 12)
}
pub struct MetagraphHandle(Arc<arc_swap::ArcSwap<Metagraph>>);   // lock-free load() per request
impl MetagraphSync {
    pub async fn spawn(client: Client, netuid: NetUid, mech: MechId, p: SyncPolicy, cancel: CancellationToken)
        -> Result<(MetagraphHandle, tokio::sync::broadcast::Receiver<MetagraphDiff>), SyncError>;
}
pub struct MetagraphDiff {
    pub from_block: u64, pub to_block: u64,
    pub registered: Vec<(Uid, AccountId)>, pub replaced: Vec<(Uid, AccountId, AccountId)>, pub deregistered: Vec<(Uid, AccountId)>,
    pub permits_gained: Vec<Uid>, pub permits_lost: Vec<Uid>, pub axons_changed: Vec<Uid>,
}
```

- **Defaults:**
  - validators: `EveryEpoch { fallback_blocks: 100 }` with `Preset::Validator`;
  - miners: `Interval(600 s)` (minimum 30 s) with `Preset::Miner`, SN2's values ([R] G §3.4 item 7);
  - the owner monitor: `EveryEpoch` with `Full`.
- **`EveryEpoch`** keys off `LastEpochBlock` advancing (§3.13).
- **Score resets.** `MetagraphDiff::replaced` drives them, because the template and SN2 reset per-UID state when the
  hotkey at a UID changes ([R] G §2.4 item 13). `hotkey_root(hk)` follows the `HotkeyRoot`/`HotkeySuccessor` lineage
  for scoring continuity across hotkey swaps.
- **Failures.** After N consecutive failures the service keeps serving the last good snapshot and emits a health event.
  It never publishes an empty metagraph ([R] E §4.2.5).

`spawn` first fetches a nonempty finalized graph, returning an error if it cannot
bootstrap. The handle's `load()` returns an `Arc<Metagraph>` without locking or
decoding; the hotkey index is built once before publication. The returned diff
receiver observes successful refreshes after that initial snapshot. `health()`
retains the last successful block, consecutive failures, threshold status,
last error and worker stop state. Session and head tracking handle physical
connection recovery; the graph service keeps the same client identity during
read failures. `shutdown().await`, caller cancellation, client closure or the
last handle's drop stops the worker and releases its pending reads.

Selective snapshots retain the partial runtime record, with unrequested
fields represented as `None` in raw JSON. Their row/header projection supplies
zero or empty values for unrequested fields. The selective mechanism API emits
the base netuid, so its header's mechanism comes from the request; full
mechanism graphs retain the response's packed netuid. Typed selective decoding
requires the actual API projection's shape to match, then falls back to the
metadata decoder. A requested moving price still requires signed I96F32.

### 8.4 Miner admission

Admission uses the `ArcSwap` handle, refreshed once per cadence, and an O(1) `by_hotkey` index built once per refresh.
It never decodes per request [J-MA G12, J-PD §2.1]. One `MetagraphPolicy::admit` call is a hash lookup plus three
comparisons.

### 8.5 Persistence

Python v11 has no persistence ("persist `mg.raw` yourself", `UP/docs/migration.mdx:308-317`). rtao offers
`persist::save(&Metagraph, path)` and `persist::load(path, &RuntimeCache)`:

- The file holds magic `RTMG`, a `u8` format version, then a SCALE header `{ genesis, block_hash, block_number,
  spec_version, netuid, mechid, preset }`, then the **raw runtime-API response bytes**.
- Writes are atomic: temp file, fsync, rename.
- Loading re-decodes with the runtime of `spec_version`, from the shared disk cache (§3.4). The file therefore outlives
  rtao's model changes.
- v10 `.pt` pickles are not read ([R] I §3.3).

### 8.6 Performance claims, corrected

- Neither the typed path nor the dynamic path is "faster" in a way that matters to a validator. One `get_metagraph`
  `state_call` took 0.71-0.88 s over one-shot HTTPS (B, Appendix A).
- `DecodeAsType` is about 4x **slower** than the core's `Value` decoder: 1,380 µs against 330 µs at 256 UIDs. A's and
  D's rationale that "typed avoids ss58, so it is faster" is wrong and is not repeated [J-CF concern 10, J-PD §2.1].
- The borrowed view exists for miner admission scale, indexers that scan all subnets, and allocation pressure, not for
  RPC latency.
- **Criterion baselines** are committed for core `Value`, `DecodeAsType`, `MetagraphView`, `Runtime::parse` (V15),
  `conform` at 2,500 weights, `fetch` key building, and btauth sign/verify. They run on `vendor/bench/` payloads and
  gate on regressions above 30% (§11.7).
- B's bench decodes `[u8; 32]` element by element through the visitor, which may overstate `DecodeAsType`'s cost
  [J-PD §2.1]. A hand visitor that slices `Array::bytes_from_undecoded()` is a measured M6 experiment, not an
  assumption [I].

---

## 9. Framework for owners, validators and miners (`rtao-neuron`)

v11 ships no framework ([R] G §6.1), so this crate is rtao's own. It is built **only** on rtao's public API (intents,
reads, `Client`, btauth), so every chain interaction inherits the parity guarantees of §4. It follows C's operator
design, with corrections from the other lenses.

### 9.1 Runtime architecture

`NeuronRuntime` supervises a `tokio::task::JoinSet` under one `CancellationToken`. Services talk through `watch` (head,
metagraph version) and `broadcast` (diffs, events) channels:

```
          ┌──────────── HeadTracker (finalized heads, gap-free) ───────────────┐
          │                                                                     │
 MetagraphSync ──► MetagraphHandle (ArcSwap) ──► BtAuthLayer + MetagraphPolicy (miner)
          │              │                                                      │
          │              └──► Validator::step (user scoring; SignedClient fan-out)
          ▼                                                                     ▼
 MetagraphDiff ──► Validator::on_metagraph (score resets)         EpochClock ◄── head watch
                                                                    │
 WeightScheduler (per mechanism) ◄──────────────────────────────────┘ (preflight → conform → commit → watch → reveal watcher)
 AxonPublisher · StateStore (300 s + exit) · RuntimeWatcher · ClockGuard · Telemetry (/metrics /healthz /readyz) · signals
```

```rust
pub struct NeuronCx {
    pub client: Client, pub executor: Executor, pub netuid: NetUid,
    pub hotkey: HotkeyFile,                      // KeySource<Hotkey> only: coldkey intents do not compile in a neuron
    pub metagraph: MetagraphHandle, pub epochs: EpochClock, pub http: SignedClient,
    pub shutdown: CancellationToken, pub metrics: Metrics, pub state_dir: PathBuf,
}
/// Shared state whose guards cannot cross an .await. std guards are unconditionally !Send and every framework callback
/// returns `impl Future + Send`, so holding read()/write() across an await is a compile error in any subnet's graph.
/// clippy's await_holding_lock does not flag tokio guards, and parking_lot guards become Send as soon as any crate
/// enables parking_lot/send_guard (both reproduced with rustc/clippy 1.99). read()/write() recover from poisoning
/// (unwrap_or_else(PoisonError::into_inner)); a compile_fail doctest holds write() across an .await inside step.
#[derive(Clone)]
pub struct StateCell<S>(Arc<std::sync::RwLock<S>>);
```

### 9.2 What a validator author writes

```rust
#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cfg = rtao::neuron::NeuronConfig::load_for(NetUid(1)).await?;       // env > ~/.bittensor/btcli.json > default
    rtao::neuron::NeuronRuntime::validator(cfg, MyValidator::default()).run().await
}

impl rtao::neuron::Validator for MyValidator {
    type State = rtao::neuron::EmaScores;                                   // keyed by hotkey, never by uid alone
    async fn step(&self, cx: &NeuronCx, s: &StateCell<Self::State>) -> anyhow::Result<()> {
        let mg = cx.metagraph.load();                                       // Arc<Metagraph>; O(1) by_hotkey
        let replies = cx.http.fan_out(mg.axon_targets(), make_request, FanOut::default()).await;
        s.write().update(&mg, &replies);                                    // a !Send guard cannot cross the next .await
        Ok(())
    }
    fn weights(&self, _cx: &NeuronCx, s: &Self::State, _mech: MechId) -> WeightsProposal { s.to_proposal() }
}
// Typed one-offs: cx.executor.execute(&ServeAxon { .. }, &cx.hotkey, &Default::default()).await?
// `cx.executor.execute(&AddStake { .. }, &cx.hotkey, …)` does not compile: AddStake::Role = Coldkey.
```

```rust
pub trait Validator: Send + Sync + 'static {
    type State: Serialize + DeserializeOwned + Default + Send + Sync + 'static;
    fn on_start(&self, cx: &NeuronCx, s: &StateCell<Self::State>) -> impl Future<Output = anyhow::Result<()>> + Send { async { Ok(()) } }
    fn step(&self, cx: &NeuronCx, s: &StateCell<Self::State>) -> impl Future<Output = anyhow::Result<()>> + Send;
    fn weights(&self, cx: &NeuronCx, s: &Self::State, mech: MechId) -> WeightsProposal;      // pure
    fn on_metagraph(&self, _d: &MetagraphDiff, _s: &mut Self::State) {}   // default: no-op; EmaScores::apply(&MetagraphDiff)
                                                                          // drops replaced and deregistered hotkeys, and the
                                                                          // template calls it
    fn step_policy(&self) -> StepPolicy { StepPolicy::default() }
}
pub enum WeightsProposal { Set(WeightsInput), HoldPrevious, Skip(&'static str) }
pub enum WeightsInput { ByUid(BTreeMap<Uid, f64>), Parallel { uids: Vec<Uid>, weights: Vec<f64> },
                        ByHotkey(Vec<(AccountId, f64)>), RawU16 { uids: Vec<Uid>, values: Vec<u16> } }
pub struct StepPolicy {
    pub cadence: StepCadence,        // EveryFinalizedBlock (default) | Interval(Duration) | Continuous { min_interval: Duration }
    pub timeout: Option<Duration>,   // None; an overrunning step is cancelled and counted as an error
    pub backoff: Backoff,            // after Err or panic: 1 s doubling to 60 s, reset on Ok
}
```

`step` runs in its own task in the runtime's `JoinSet`, paced by `step_policy()` (proposal C's "called in a loop, paced
and cancelled by the runtime", P-C line 1597). An `Err` or a panic is logged, counted in
`rtao_step_errors_total`, flips `/readyz` after 5 consecutive failures and backs off, but never ends `run()`. `run()`
returns only on shutdown or a framework-fatal error (wallet, registration, genesis), and the first shutdown signal
cancels the in-flight `step`. A `step` that returns at once (no serving axons) therefore cannot busy-spin against the
1 r/s public bucket (§3.2.5), and a transient RPC error returned from `step` cannot stop the validator.

### 9.3 `WeightScheduler`: the validator's critical path

The scheduler runs per finalized head, for each mechanism `mech < MechanismCountCurrent[netuid]`. Each mechanism has its
own rate-limit slot (`LastUpdate[NetUidStorageIndex(netuid, mech)]`, [R] C1 §2.4).

1. **Cadence.** The default is `OncePerEpoch { offset_blocks: 2 + (blake2(hotkey) mod max(1, tempo / 10)) }` (C).
   - One commit per epoch is enough: commits are tagged and revealed per epoch.
   - Submitting early in the tempo matters, because inclusion is first-come and tips buy nothing.
   - The deterministic per-hotkey jitter spreads validators across blocks [I].
   - Subnets with commit-reveal off use `RateLimited { margin }`.
   - An activity alarm fires at 80% of the effective activity cutoff (factor × tempo / 1000; [R] G §2.2 item 6).
2. **Preflight in one RPC.** A single `fetch` (§3.6) at one pinned block reads `Uids`, `LastUpdate[idx][uid]`,
   `WeightsSetRateLimit`, `CommitRevealWeightsEnabled`, `CommitRevealWeightsVersion`, `RevealPeriodEpochs`,
   `WeightsVersionKey`, `MinAllowedWeights`, `MaxWeightsLimit`, `SubnetworkN`, `SubnetEpochConsensus`,
   `ValidatorPermit`, `Tempo`, `LastEpochBlock`, `PendingEpochAt`, `SubnetEpochIndex` and `BlocksSinceLastStep`. This
   is one `state_queryStorageAt` where C's design made about 18 calls under a 1 r/s bucket [J-PD concern 6].
   - **Head.** The fetch is pinned at the best head's hash at the moment the scheduler fires (`HeadPolicy::Best` for
     this read only; finalized heads still trigger the scheduler and anchor the era). `current_block` in step 7 is that
     best block's number. The predictor assumes the commit lands in `current_block + 1`
     (`CORE/timelock/constants.rs:41-44`, `CORE/timelock/epoch_schedule.rs:116-117`), and Python reads the same items at
     the best head (`PY/intents/weights.py:283-306` → `PY/_substrate.py:417-418` →
     `PY/_transport/interface.py:218-226`). Pinning at the finalized head would make every round 4 × (best −
     finalized) rounds later than Python's, 8-12 rounds at finney's measured lag of 2-3 blocks.
   - **Refusals**, reserved for outcomes the chain would certainly reject or silently drop:
     - not registered;
     - `SubnetEpochConsensus == Null` and `ValidatorPermit[netuid][uid] == false`: a certain rejection at the pool
       (`guards/check_weights.rs:97-104`, custom code 255) and at dispatch (`subnets/weights.rs:17-26`), which Python
       also refuses client-side (`PY/intents/weights.py:184-190`);
     - rate limited (scheduled, not failed);
     - `CommitRevealWeightsVersion ≠ SUPPORTED_CR_VERSION` (below);
     - configured `version_key < WeightsVersionKey`: the chain checks the version key at **reveal** time, so a stale
       key silently loses the reveal ([R] C1 §2.3; `check_version_key`, weights.rs:1285-1292).
   - **Warnings only**: a missing validator permit on Yuma subnets and stake below `StakeThreshold`. The client would
     otherwise need to
     recompute the pallet's stake weight, and C's hard preflight risks false negatives [J-CF §3.3 defect 5].
   - Codes match the chain's (`NotRegistered`, `RateLimited`, `NotAuthorized`, `InvalidArgument`), and each comes with
     remediation text.
3. **Commit-reveal version gate (D-016)** [J-CF concern 1].
   - The chain requires `commit_reveal_version == CommitRevealWeightsVersion` and documents it as "the client
     (bittensor-drand) version" that "must match … used to gate runtime upgrades" (`UP/pallets/subtensor/src/subnets/
     weights.rs:318-337`; `macros/dispatches.rs:1946` [V]). The check is at dispatch, on a `Pays::No` call
     (`dispatches.rs:1947-1948` [V]).
   - rtao keeps Python's constant: `pub const SUPPORTED_CR_VERSION: u16 = 4`, which is `DEFAULT_COMMIT_REVEAL_VERSION`
     (`PY/intents/weights.py:33` [V]) and is tied to the core's `WeightsTlockPayload` format.
   - **The intents are unchanged from Python**: `SetWeights` always tags 4 (`weights.py:411` [V]), and `CommitWeights`
     has the field `commit_reveal_version` with default 4 (`:472-477` [V]). The oracle checks both.
   - **The framework refuses early** when storage differs, with `ChainError { name: "IncorrectCommitRevealVersion" }`,
     the remediation "the chain requires commit-reveal client version N; this rtao build supports 4; upgrade rtao", an
     alert metric, and no submission.
   - B, C and D echo the storage value instead. After a root bump that would tag a v4-format payload with the new
     version and turn a loud dispatch error into a possible silent reveal loss. That is rejected.
4. **Conform**, a port of `normalize` and `clip_to_max_weight` (`PY/intents/weights.py:54-114` [V]) and `_conform`
   (`:218-258`):
   - validate: equal lengths; finite, non-negative values; uids ≤ 65535; no duplicates;
   - unless the row is a lone self-weight and while `MaxWeightsLimit < 65535`, clip with Python's cutoff search:
     `estimation` sorted ascending, `cumsum` via left-to-right accumulation, the `n_values` count with
     `epsilon = 1e-7`, then `cutoff_scale = (limit·cumsum[n_values−1] − ε) / (1 − limit·(n − n_values))`, mass
     redistributed;
   - max-upscale with **`f64::round_ties_even((w / top) * 65535.0)`**. Python's `round()` is half-to-even, while Rust's
     `f64::round` rounds half away from zero [J-CF concern 7];
   - drop zeros;
   - enforce `len ≥ min(MinAllowedWeights, SubnetworkN)`, with the self-weight exemption.

   Every float operation keeps Python's order and associativity (`sum()` is left-to-right; `total` is computed exactly
   as Python computes it). **Oracle vectors pin u16 outputs bit for bit**: 200 seeded rows, `.5` ties, the clip cutoff
   path, `n·limit ≤ 1` (uniform), single self-weight, and 2,500-entry rows (§11.5).

   `RawU16` requires Null consensus: the preflight mints a `NullProof` only when `SubnetEpochConsensus == Null` and this
   hotkey is the sole permit holder. Values pass verbatim (`PYT/unit/test_null_weights.py:13-45`). The chain's
   `MaxWeightExceeded` is dead code ([R] C1 §0.7), but rtao clips anyway for parity.
5. **Hold-previous guard.** A row whose only positive weights are self or owner is not submitted (SN2's guard,
   [R] G §2.6 item 21).
6. **Path.**
   - Commit-reveal on: `commit_timelocked_mechanism_weights(netuid, mech, ciphertext, reveal_round, 4)` (call 118).
   - Commit-reveal off: `set_mechanism_weights` (119).
   - Never the legacy `set_weights` (0) or `commit_crv3_mechanism_weights` (117) ([R] C1 §0.6).
7. **`TimelockCommitter`**, with all cryptography from the core:

   ```text
   schedule = EpochScheduleState { last_epoch_block, pending_epoch_at, subnet_epoch_index, tempo,
                                   blocks_since_last_step, current_block }        // read at ONE block (step 2)
   first    = rtao_core::epoch::predict_first_reveal_block(&schedule, reveal_period_epochs)   // pallet predicate (G4)
   target   = first + SECURITY_BLOCK_OFFSET                                          // 3 (CORE/timelock/constants.rs)
   round    = max(1, floor((clock.now_unix() + (target − current_block) · block_time − GENESIS_TIME) / DRAND_PERIOD))
   ct       = encrypt_and_compress(WeightsTlockPayload { hotkey: pubkey.to_vec(), uids, values, version_key }.encode(), round)
   ```

   - The formula is `CORE/timelock/mod.rs:169-187`. `block_time` comes from `Aura.SlotDuration`, which makes fast-block
     localnets (0.25 s) work.
   - Size limits: ≤ 5,000 bytes on Yuma, ≤ ⌊32 KiB / mechanism count⌋ on Null ([R] C1 §2.3).
   - A pool rejection `InvalidRevealRound` (custom code 15: round below `Drand.LastStoredRound`) triggers exactly one
     recomputation.
8. **The epoch predicate is pallet-faithful (D-006)** [J-CF concern 2].
   - The core and Python use `blocks_since_last_step > MAX_TEMPO_U64` (`CORE/timelock/epoch_schedule.rs:54` [V]). The
     pallet fires its safety net at `BlocksSinceLastStep > tempo` (`run_coinbase.rs:1232` [V]) and caps the counter at
     `tempo + 1` (`:413-415` [V]).
   - After an `EpochSkipped` slot (`LastEpochBlock` advances but `BlocksSinceLastStep` is not reset), or after
     `sudo_set_tempo` lowers the tempo below the step counter (only `LastEpochBlock` is reset), the pallet fires its
     safety net as soon as the counter exceeds `tempo`: the very next block, and every block while the input state stays
     inconsistent (`run_coinbase.rs:422-438, 472-483`; `coinbase/tempo_control.rs:38` → `utils/misc.rs:119-125`). The
     core waits for `LastEpochBlock + tempo` and predicts one epoch too late. Deferral sets `PendingEpochAt`, which both
     predicates honor, and does not diverge. The ciphertext is then encrypted to a round that arrives after the actual
     reveal epoch has ended. Reveals retry every block within the
     reveal epoch, and queues for earlier epochs are deleted ([R] C1 §2.3), so the commit is silently lost.
   - rtao's staged `epoch_schedule` uses the pallet predicate and cap. A test asserts agreement with the core on
     **every upstream vector** (`CORE/timelock/epoch_schedule_vectors.rs`, transcribed to JSON by `xtask
     import-upstream` because the module is private, `CORE/timelock/mod.rs:13-14` [J-MA §3.2]) and disagreement only on
     the new safety-net vectors, which are derived from the pallet's tempo-control tests: a skip at a scheduled slot b
     fires the epoch at b + 1; a skip at an owner-triggered slot b with step counter c fires it at b + tempo + 1 − c; a
     tempo lowered below the counter in block t fires it at t + 1.
   - Reveal rounds therefore differ from Python's only in that edge case. This is registered, PR-U5 fixes the core
     (and with it Python), and the shim is deleted when PR-U5 ships in `core_tag`.
   - A's choice to inherit the core's predicate for Python parity is rejected: parity with a bug that silently loses
     validator commits is not worth keeping.
9. **`MaxEpochsPerBlock` deferral is not modeled**, and this is a deliberate choice [J-CF concern 2]. On production
   `MaxEpochsPerBlock` is 2 ([R] C1 §2.3). Deferral sets `PendingEpochAt = block + 1` and emits `EpochDeferred`
   (`run_coinbase.rs:423-431` [V]), which moves an epoch **later**:
   - The reveal attempts then start later than predicted, and our round, targeted at predicted + 3 blocks, is already
     available. Per-block retries within the reveal epoch absorb it. Deferral is in the safe direction.
   - Modeling it would need every subnet's schedule state, iteration order and Null-mode flags at future blocks.
   - The pallet's own `get_next_epoch_start_block` ignores it too (`run_coinbase.rs:1256-1262` [V]).

   The dangerous direction, an epoch firing *earlier* than predicted, comes from the safety-net predicate (fixed in
   step 8) or an owner `trigger_epoch` after the commit, which is unpredictable by construction. The scheduler watches
   `EpochDeferred`, `EpochSkipped` and `EpochTriggered` for its netuid and records them next to each reveal outcome, so
   any future miss is attributable. Asking upstream to account for deferral in `get_next_epoch_start_block` is an open
   question (§15.2 Q9).
10. **Clock** [J-CF graft 9].
    - The reveal round depends on wall-clock time (Python uses the system clock inside the core). rtao uses an
      injectable `Clock`: `SystemClock` by default for Python parity, `ChainClock` (`Timestamp.Now(head)` plus monotonic
      time since the head notification) as an option.
    - `ClockGuard` estimates skew as the minimum, over a sliding window of 20 best-head notifications, of
      `system_now − Timestamp.Now(head)`; the minimum filters out propagation delay.
    - Above 2 s it warns, because btauth's skew tolerance is 2 s. Above 12 s (one block, about 4 drand rounds; the
      +3-block offset is about 12 rounds of margin [I]) it refuses weight commits under `SystemClock` and raises a
      health event recommending `ChainClock`.
    - Metric: `rtao_clock_skew_seconds`.
11. **Submit** with **Python's defaults**: era 128 anchored at the finalized head, waiting for finalization. Bounded
    deadlines apply: 30 s to submit and the era window to inclusion (SN2's 30 s/180 s bounds, adapted;
    `SP/upstream/inference-labs-inc_subnet-2/crates/sn2-chain/src/weights.rs:13-14` [J-MA §3.3]). Reconciliation is on
    (§3.9.6).
    - C's era 32 with `WaitFor::InBlock` is rejected [J-CF concern 8]. A 128-block era already resolves `FateUnknown`
      within about 26 minutes, inside a 72-minute tempo.
    - Only one pending commit per `(hotkey, netuid_index)` may sit in the pool (provides tag `weight-commit`; [R] C1
      §2.2), so the scheduler never blind-resubmits. After `FateUnknown` it reconciles first.
12. **Reveal watcher.** During the commit's actual reveal epoch, watch for `TimelockedWeightsRevealed(netuid_index,
    who)`. After inclusion the watcher reads `TimelockedWeightCommits[netuid_index][e]` at the inclusion block for
    e ∈ {predicted, predicted + 1} to learn the actual epoch, and counts predicted ≠ actual in
    `rtao_weights_commit_epoch_slip_total`. If the epoch ends without the event, it increments
    `rtao_weights_reveal_missed_total{mech, reason}` and logs the round, the predicted block, `Drand.LastStoredRound`
    and the epoch events seen.
    - The pallet reveals through `do_set_mechanism_weights`, which re-runs the registration, minimum-stake, version-key,
      validator-permit, uid-validity and length checks at the reveal block (`subnets/weights.rs:1010-1053`), and drops a
      failing reveal with `log::trace` and no event (`coinbase/reveal_commits.rs:188-202`). On a miss the watcher
      therefore reads, at the first block of the reveal epoch, `ValidatorPermit[uid]`, the hotkey's stake against
      `StakeThreshold`, `WeightsVersionKey`, `SubnetworkN`, `MinAllowedWeights` and `Uids[netuid][hotkey]`, and sets
      `reason` to the first failing check, or to `drand_round` when `Drand.LastStoredRound < round`.
    - It warns before the reveal epoch when the permit was lost at the intervening epoch boundary.
    - Tests: a `FakeChain` case with finality lag 3 asserts that the predictor receives the best block's number and
      state; a `FakeChain` case whose inclusion slips past an epoch boundary asserts that the watcher follows the
      actual epoch and raises no miss.

**Version key policy**: `VersionKey::{Zero (v11 default, UP/docs/migration.mdx:400-404), Fixed(u64)}`. There is no
"follow the chain's `WeightsVersionKey`" mode, because that defeats the owner's forced-upgrade lever (C's open question
5). Being below the chain's key is a refusal with remediation (step 2).

### 9.4 Miner

```rust
pub trait Miner: Send + Sync + 'static {
    type State: Serialize + DeserializeOwned + Default + Send + Sync + 'static;
    fn router(&self, cx: &NeuronCx, state: StateCell<Self::State>) -> axum::Router;  // wrapped by the runtime in
                                                                 // BtAuthLayer + policy + limiter; called after on_start
    fn caller_policy(&self, cx: &NeuronCx) -> MetagraphPolicy { MetagraphPolicy::validators(cx.metagraph.clone()) }
    fn on_start(&self, cx: &NeuronCx, s: &StateCell<Self::State>) -> impl Future<Output = anyhow::Result<()>> + Send { async { Ok(()) } }
    fn on_metagraph(&self, _d: &MetagraphDiff, _s: &mut Self::State) {}
}
```

`StateCell` is `Clone` (an `Arc` inside), so handlers receive the miner's state through `.with_state(state)` or an
`Extension` and read and update the state that `StateStore` persists (§9.11). The runtime calls `on_start` before
`router`, so the cell is populated before the first request.

The runtime adds:

- the HTTP server with graceful shutdown;
- `BtAuthLayer`, `MetagraphPolicy` (registered + permit + stake floor) and `PerHotkeyLimiter`;
- `AxonPublisher`;
- `MetagraphSync` with the miner preset;
- the `PruneRisk` monitor (gauges `rtao_immunity_blocks_remaining` and `rtao_prune_risk`, computed with the pallet's
  pruning order, [R] C1 §4.1);
- `StateStore` and telemetry.

### 9.5 `AxonPublisher`

```rust
pub struct AxonConfig {
    pub port: u16, pub external_ip: Option<IpAddr>, pub external_port: Option<u16>,
    pub protocol: u8 /* 4 */, pub version: u32 /* 1 */, pub certificate: Option<NeuronCertificate>,
    pub ip_sources: Vec<Url> /* ipify-style, 5 s timeout */, pub allow_non_global: bool /* false; true on local */,
    pub recheck: Duration /* 300 s */, pub reset_on_shutdown: bool /* false */,
}
```

The chain rules come from `pallets/subtensor/src/subnets/serving.rs` ([R] C1 §3):

1. Resolve the IP: the override first, else detection. Loopback, RFC 1918, CGNAT and link-local IPv4 are rejected
   unless allowed. IPv6 is encoded (`ip_type = 6`) but not auto-detected.
2. Read `Axons[netuid, hotkey]` and, when a certificate is configured, `NeuronCertificates[netuid, hotkey]`. If
   `(ip, port, ip_type, protocol, version)` match and the stored `(algorithm, public_key)` equals the configured
   certificate, do nothing. A changed certificate alone triggers `ServeAxonTls`, subject to `ServingRateLimit`: the
   certificate is written only to `NeuronCertificates` and only by `serve_axon_tls` (`subnets/serving.rs:71-87`;
   `NeuronCertificate { algorithm, public_key }`, `lib.rs:339-355`), and the pallet never removes one, so without a
   configured certificate the stored one is ignored. Test: `cert_rotation_republishes`.
3. If `now − prev.block < ServingRateLimit` (default 50), schedule for the first allowed block.
4. Otherwise submit `ServeAxon`, or `ServeAxonTls` with a certificate, signed by the hotkey (`Pays::No`).

Pool rejections 11, 12 and 13 map to `InvalidIpAddress`, `ServingRateLimitExceeded` and `InvalidPort`. A certificate
over 65 bytes, which the chain would silently drop, is rejected client-side as an rtao strictness (D-017).

### 9.6 Registration, immunity, PoW

- Neuron runtimes refuse to start unregistered unless `--wait-for-registration` is set (then they wait for
  `NeuronRegistered`). Registration is a coldkey operation, done from the CLI or owner tools.
- **`burned_register`** is always shielded (`mev_shield_required`). It previews the burn/lock split from `Burn` and
  `CollateralLockShare` (`PY/intents/registration.py:27-44`). Its spend is unbounded, so any spend cap blocks it unless
  the operator states a maximum ([R] G §2.1).
- **`pow_register`** uses `PowSolver`, a port of `PY/pow_registration.py:18-253`:
  - the prefix is `b"subtensor-pow-register-v1" ‖ netuid LE ‖ block_hash ‖ hotkey ‖ coldkey` (123 bytes);
  - the core computes `pow::seal`/`mine_cpu`, or `GpuMiner` under `pow-gpu`;
  - the work block is refreshed before it is 5 blocks old, and the seal is re-verified on the CPU.
  It requires `host`, so it is an owner/CLI capability.
- **`root_register`**. Confirmation goes through the `uid` read and the `NeuronRegistered` event.

### 9.7 Owner operations

```rust
impl OwnerOps {
    pub async fn create_subnet(&self, hotkey: AccountId, wait: RegistrationWait) -> Result<SubnetRegistered, TxError>; // queued path (§3.13)
    pub async fn start(&self, netuid: NetUid) -> Result<ExtrinsicResult, TxError>;          // after subnet_start_schedule
    pub async fn set_hparam(&self, netuid: NetUid, name: &str, v: HparamArg, w: WindowWait) -> Result<ExtrinsicResult, TxError>;
    pub async fn set_identity(&self, netuid: NetUid, id: SubnetIdentity) -> Result<ExtrinsicResult, TxError>;
    pub async fn update_symbol(&self, netuid: NetUid, symbol: &str) -> Result<ExtrinsicResult, TxError>;
    pub async fn set_mechanism_count(&self, netuid: NetUid, n: u8) -> Result<ExtrinsicResult, TxError>;
    pub async fn trim(&self, netuid: NetUid, max_n: u16) -> Result<ExtrinsicResult, TxError>;
    pub async fn trim_null(&self, netuid: NetUid, target: u16, recovery: &Path) -> Result<TrimReport, TxError>; // resumable ≤ 64-uid batches
}
impl HyperparamPlanner {
    pub async fn plan<V: View>(&self, at: &V, netuid: NetUid, name: &str, v: HparamArg) -> Result<HparamPlan, TxError>;
}
pub struct HparamPlan { pub current: DynValue, pub raw: DynValue, pub call: CallBytes, pub earliest_block: u64, pub reasons: Vec<String> }
```

The planner encodes the owner rules ([R] G §1.5, [R] C1 §5.2):

- owner-settable versus root-only, from the extracted owner-setter map;
- unit conversion (§5.5) and bounds: tempo 360-50,400, activity factor 1,000-50,000, collateral share ≤ 62,258, …;
- the admin freeze window: `blocks_until_next_auto_epoch ≥ AdminFreezeWindow` and no pending trigger;
- per-setter rate limits, which apply to the owner origin only. `owner` is the signing owner coldkey, and a stored
  `last == 0` passes (`utils/rate_limiting.rs:64-67`):

  | Setter (tx-type code) | Last block | Limit |
  |---|---|---|
  | owner hyperparameters not listed below, keyed by the setter's `Hyperparameter` variant (`alpha_low`/`alpha_high` share `AlphaValues`; `activity_cutoff_factor` is `ActivityCutoffFactorMilli`) | `LastRateLimitedBlock[RateLimitKey::OwnerHyperparamUpdate(netuid, h)]` | `OwnerHyperparamRateLimit` (default 2) × `Tempo[netuid]` |
  | `sudo_set_owner_cut_enabled`, `sudo_set_owner_cut_auto_lock_enabled` | none | none (freeze window only, `UP/pallets/admin-utils/src/lib.rs:2511-2555`) |
  | `sudo_set_weights_version_key` (4) | `TransactionKeyLastBlock[(owner, netuid, 4)]` | `WeightsVersionKeyRateLimit` (default 5) × `Tempo[netuid]` |
  | `sudo_set_tempo` (11) | `TransactionKeyLastBlock[(owner, netuid, 11)]` | `MinTempo` = 360 blocks (a metadata constant) |
  | `sudo_set_mechanism_count` (7), `sudo_set_mechanism_emission_split` (8) | `TransactionKeyLastBlock[(owner, netuid, 7 or 8)]` | 7,200 blocks (fast-blocks: 1) |
  | `sudo_trim_to_max_allowed_uids`, and `sudo_trim_null_uids_batch` unless it continues the stored `NullPruningTarget` (9) | `TransactionKeyLastBlock[(owner, netuid, 9)]` | 216,000 blocks (fast-blocks: 1) |

  Sources: `UP/pallets/subtensor/src/utils/rate_limiting.rs:38-53, 89-131, 134-152`; `utils/misc.rs:38-53, 94-105`;
  `coinbase/tempo_control.rs:16-45`; `UP/pallets/admin-utils/src/lib.rs:444-472, 2094-2205, 2511-2555`. Only owner
  hyperparameters (and `SetSNOwnerHotkey`) use `LastRateLimitedBlock`; every other type uses
  `TransactionKeyLastBlock[(key, netuid, tx_type)]`.
  - Root bypasses the rate limits but not the freeze window: every owner setter calls `ensure_admin_window_open` after
    `ensure_sn_owner_or_root_with_limits`, root included (`UP/pallets/admin-utils/src/lib.rs:454` and 44 other sites;
    `tempo_control.rs:28`). The exception is `activity_cutoff_factor`, where root bypasses both
    (`tempo_control.rs:64-72`).
  - The 7,200- and 216,000-block limits are `#[pallet::type_value]`s absent from metadata
    (`pallets/subtensor/src/lib.rs:1128-1132, 3567-3577`). The chain-source layer extracts the `prod_or_fast!` pairs
    into `rtao_catalog::rt::rate_limits` (§4.9 layer 5), and the planner uses the fast value when
    `Aura.SlotDuration < 12_000` (`common/src/lib.rs:183`).
  - A `TxRateLimitExceeded` dispatch error re-plans once.
  - Tests: one `FakeChain` planner test per row, including `alpha_low` then `alpha_high` inside one window and the
    owner-cut toggles with no limit; localnet scenario 10 changes the mechanism count twice on one subnet and asserts
    that `earliest_block` equals the first block the chain accepts;
- the shared `alpha_low`/`alpha_high` setter, `epoch_consensus` as an enum, and fixed-point values as `{bits}`;
- never the deprecated no-op `set_tempo` or `set_activity_cutoff_factor`.

`WindowWait::UntilOpen { deadline }` waits for the computed block instead of failing.

`OwnerMonitor` streams owner events ([R] G §1.11) and exports:

- both "on" flags, `SubtokenEnabled` and `SubnetEmissionEnabled`, because a subnet can run with zero pool-side emission;
- the conviction leader against the 18% threshold;
- pending delayed-proxy announcements;
- pruning exposure.

### 9.8 Timelock helpers beyond weights

`rtao_tx::timelock` ports `PY/timelock.py`, its whole `__all__` (`:380-392`), over the core's `UserData` envelope, so
`btcli timelock` reads rtao output and the reverse:

- **`seal(data, Reveal) -> Result<Timelocked, TimelockError>`**, with `enum Reveal { In(Duration), At(SystemTime),
  Round(u64), InBlocks(u64) }`. Exactly one form is given, as in `encrypt` (`:254-304`). `In` also parses Python's
  duration grammar (`:103-129`), and `At` also parses ISO-8601, where no offset means UTC (`:132-152`). A target round
  at or below `current_round()` is refused with Python's message. `Reveal::InBlocks(n)` has no Python counterpart and
  is registered as an addition (D-033).
- **`Timelocked { ciphertext, reveal_round }`**, with `parse(bytes | hex)` (the trailer `AES_GCM_` plus a
  little-endian u64 round, `:192-207`), `reveal_at()`, `remaining()`, `revealed()`, `hex()` (the portable envelope)
  and `encrypted()` (the inner TLE bytes that `Commitments.set_commitment` takes, `:229-237`).
- **`open(&Timelocked, &DrandClient, OpenOptions { wait, timeout })`**, an async beacon fetch plus
  `decrypt_with_signature`. Before the reveal time it returns `TimelockError::NotReady { reveal_round, reveal_at,
  remaining }`; after it, it retries the beacon fetch every 1 s for up to 30 s (`:307-358`). The async `open` defaults
  to `wait: true`, like `decrypt_async` (`:361-377`); `rtao::blocking`'s `decrypt` defaults to `wait: false`, like the
  sync `decrypt`.
- **Pure helpers.** `current_round`, `round_at`, `reveal_time`, `parse_duration` and `format_duration` live in
  `rtao_primitives::drand`, over `rtao_core::timelock::constants::{GENESIS_TIME, DRAND_PERIOD}` (equal to Python's
  `QUICKNET_GENESIS` = 1,692,803,367 and `QUICKNET_PERIOD` = 3, `PY/timelock.py:53-62`). They land in M1, because
  `NeuronCommitment::reveals_at` needs `reveal_time` (§4.8), are re-exported here, and are pinned by oracle vectors.
  `current_round` accepts an injected Unix time. `round_at` preserves Python's truncation of elapsed milliseconds
  before its ceiling, so a target less than one millisecond after a pulse can select that pulse. ISO text uses
  Python's broader datetime grammar, with date-only and naive values treated as UTC. Round arithmetic checks the
  native u64 range; `reveal_time` preserves round zero's pre-genesis meaning and checks Python's datetime year bounds.
- `rtao timelock encrypt` keeps `--in`, `--at` and `--round` and Python's JSON keys: `ciphertext` (or `path` with an
  output file), `reveal_round`, `reveal_at`, `reveal_in_seconds` and `bytes` (`PY/cli/commands/timelock.py:135-146,
  208-228`).
- `commitments::set_timelocked(netuid, fields)` composes `Commitments.set_commitment` with
  `TimelockEncrypted { encrypted, reveal_round }`. It is raw-only in Python's coverage list, so it is exposed as a typed
  helper plus `submit_call` under `allow_raw_calls`.
- Reveal time is `GENESIS_TIME + (round − 1) × 3 s` ([R] A2 §2), computed by `rtao_primitives::drand::reveal_time`.

### 9.9 Configuration and CLI conventions

```rust
#[derive(clap::Args, serde::Deserialize)]
pub struct NeuronConfig {
    #[arg(short = 'n', long, env = "BT_NETWORK", alias = "subtensor.network", default_value = "finney")] pub network: String,
    #[arg(long, env = "BT_CHAIN_ENDPOINT", alias = "subtensor.chain_endpoint")] pub chain_endpoint: Option<Url>,
    #[arg(short = 'w', long, env = "BT_WALLET", alias = "wallet.name", default_value = "default")] pub wallet: String,
    #[arg(short = 'H', long = "wallet-hotkey", env = "BT_WALLET_HOTKEY", alias = "wallet.hotkey", default_value = "default")] pub hotkey: String,
    #[arg(long = "wallet-path", env = "BT_WALLET_PATH", alias = "wallet.path")] pub wallet_path: Option<PathBuf>,
    #[arg(long)] pub netuid: NetUid,
    #[arg(long, value_delimiter = ',')] pub fallback_endpoints: Option<Vec<String>>,   // "none" pins (Python)
    #[arg(long, value_delimiter = ',')] pub archive_endpoints: Option<Vec<String>>,
    #[arg(long, env = "BT_RETRY_FOREVER")] pub retry_forever: bool,
    #[arg(long = "axon.port", default_value_t = 8091)] pub axon_port: u16,
    #[arg(long = "axon.external-ip", env = "BT_AXON_EXTERNAL_IP")] pub axon_external_ip: Option<IpAddr>,
    #[arg(long, env = "RTAO_STATE_DIR")] pub state_dir: Option<PathBuf>,
    #[arg(long, env = "RTAO_METRICS_ADDR")] pub metrics_addr: Option<SocketAddr>,
    #[arg(long, env = "BITTENSOR_LOG", alias = "logging.level")] pub log: Option<LogLevel>,
}
```

- **Names**: v11 environment names (`UP/docs/migration.mdx:162-178`) and flag spellings are primary. v10 dotted names
  are hidden clap aliases, as SN2 does ([R] I §3.4).
- **Precedence**: flag > env > `~/.bittensor/btcli.json` (or `$BTCLI_CONFIG`) > default. The keys are Python's
  (`PY/config.py:12-59`), so one config file serves both CLIs.
- **Libraries never read `argv`.**
  An application parses its own flags and passes the matches to the async
  `NeuronConfig::resolve_matches`. `load_for(netuid)` resolves environment and btcli values for an application that
  supplies its subnet directly; it does not parse command-line arguments.
- **Framework-only knobs** use the `RTAO_` prefix, except `BT_AXON_EXTERNAL_IP`, which is accepted as SN2's convention.

### 9.10 Observability

- **Logging**: `tracing` to stderr. `Compact` (single-line `key=value`, the user's own style, [R] E §6), `Json` and
  `Pretty` formats. Data goes to stdout (CLI).
- **Prometheus** on `--metrics-addr`, off by default. Metrics:
  - chain and client: `rtao_chain_head{kind}`, `rtao_finality_lag_blocks`, `rtao_spec_version`,
    `rtao_surface_breaking_items`, the `rtao_rpc_*` family (§3.2.6), `rtao_clock_skew_seconds`;
  - metagraph: `rtao_metagraph_sync_seconds`, `rtao_metagraph_age_blocks`;
  - weights: `rtao_weights_submissions_total{mech,path,outcome}`, `rtao_blocks_since_last_update{mech}`,
    `rtao_weights_reveal_missed_total{mech}`;
  - serving and btauth: `rtao_axon_published`, `rtao_btauth_requests_total{outcome}`,
    `rtao_btauth_rejections_total{class}`;
  - lifecycle and state: `rtao_immunity_blocks_remaining`, `rtao_prune_risk`, `rtao_state_save_seconds`.
- **`/healthz`** reports the process alive. **`/readyz`** requires all of: connected, runtime verified, registered,
  metagraph younger than twice its cadence, the last weight attempt not failed, and fewer than 5 consecutive `step`
  failures (§9.2). Both are served on the metrics listener.

### 9.11 Persistence, shutdown, upgrades, deployment

- **`StateStore<T>`**
  - Location: `~/.bittensor/miners/<wallet>/<hotkey>/netuid<N>/<name>/` (the template layout, [R] I §3.1).
  - Format: the envelope `{schema_version, rtao_version, netuid, hotkey, block, saved_at, state}` as JSON.
  - Writes are atomic: temp file in the same directory, fsync, rename, fsync of the directory. Three generations are
    kept.
  - Saves happen every 300 s and at shutdown ([R] G §2.5).
- **Shutdown.**
  1. The first SIGTERM/SIGINT cancels the token and work stops.
  2. An in-flight weight submission gets up to 60 s.
  3. Final save, then optional `reset_axon`, then graceful HTTP close, then exit 0.
  A second signal exits immediately with status 130.
- **Runtime upgrades.** On `SurfaceChanged` for the weight calls, the scheduler pauses and alerts while everything else
  keeps running.
- **Binary updates (deferred by Q30).** Keep `auto-update` disabled. A future distribution task must preserve:
  release check, `SHA256SUMS`, **mandatory** verification of the GitHub build-provenance attestation that `release.yml`
  produces (an asset without a valid attestation from rtao's release workflow is refused; a `SHA256SUMS` match alone
  proves nothing, since both come from the same release), atomic replace, graceful restart ([R] G §2.8).
- **Deployment templates.**
  - A multi-stage Dockerfile whose **neuron images need no Perl, OpenSSL, libsodium or CMake** (no-host profile,
    §1.7; their native code is `ring` and `blake3`'s SIMD kernels, built by the C compiler rustc already uses as its
    linker), built `-p <binary>` with BuildKit cache mounts for the cargo git and registry directories (§2.6, §2.8);
    owner images add Perl and `make`.
  - A non-root runtime image with a read-only hotkey mount and a state volume.
  - A systemd unit with `Restart=on-failure`, `KillSignal=SIGTERM` and `TimeoutStopSec=90`.

### 9.12 Safety rails (defaults)

1. Neuron runtimes hold `HotkeyFile` only. Coldkey intents do not compile there (§4.2).
2. A neuron's `Policy` is `{ allowed_netuids: {netuid}, max_spend: 0, allow_raw_calls: false }`. Every mutation has a
   dry-run `Plan`.
3. Genesis is checked at connect. Signing is refused across spec or transaction-version changes, and when the
   extension guard trips.
4. Surface verification runs at connect and on every upgrade and fails closed per call.
5. The clock-skew guard (§9.3 step 10).
6. Preflights run at one pinned block in one RPC. They refuse only on certain rejections and warn on the rest.
7. Axon publishing is idempotent. Watches are bounded, reconciliation is definitive, and nothing is blind-resubmitted.
8. The hold-previous guard; reveal-miss, activity-cutoff and prune-risk alarms.
9. Spend caps block unbounded-spend intents (`register_subnet`, `burned_register`, `transfer_all`, …; [R] A1 §3.14)
   unless the operator states a maximum.
10. `burned_register` and `add_collateral` are always MEV-shielded.

---

## 10. CLI (`rtao-cli`, binary `rtao`)

The Python CLI is 23,083 lines [V]:

- `cli/commands/*.py`: 7,872 lines of hand-written groups (`addresses`, `axon`, `collateral`, `config`, `crowd`, `evm/`,
  `extension`, `lock`, `misc`, `multisig`, `proxy`, `root`, `root_move`, `root_swap`, `stake`, `subnets`, `sudo`,
  `timelock`, `upgrade`, `wallet`, `weights`);
- shared machinery: `context.py`, `globals.py`, `output.py`, `prompt.py`, the pickers and the views;
- the registry-generated `tx.py` and `query.py`.

The binary is `rtao` only. No opt-in `btcli` shim ships: the two CLIs share configuration files (§10.2), and a shim
could surprise users who have both installed (§15.2 Q3).

### 10.1 Generated from the registries, at run time

Python generates `btcli tx` from the intent registry and `btcli query` from the read registry (`PY/cli/tx.py:1-11,
486-498`, `PY/cli/query.py:1-6, 254-266`). rtao builds `tx`, `query`, `tools`, `explain` and `call` **at start-up with
clap's builder API** from the generated specs. They need no per-command Rust code and no proc-macros, which keeps
compile time down [J-PD graft 11]:

```rust
fn tx_command() -> clap::Command {
    let mut cmd = clap::Command::new("tx").about("Submit transactions (generated from intents).").subcommand_required(true);
    for spec in rtao_catalog::INTENTS.iter().filter(|s| compiled_in(s)) {          // 83 with `full`
        cmd = cmd.subcommand(
            clap::Command::new(spec.op.replace('_', "-")).about(spec.summary).long_about(spec.description)
                .next_help_heading(help_panel(spec))           // pallet; "Root (chain sudo)"; "<pallet> — subnet owner" (tx.py:470-484)
                .args(args_from_schema(spec))                   // one --flag per property; required set; money accepts "all" when allowed
                .args(tx_globals()));                           // --dry-run --json --yes --mev-shield/--no-mev-shield --proxy-for …;
                                                                // --period is an rtao addition (D-030)
    }
    cmd
}
// run: ArgMatches → serde_json::Value → AnyIntent::from_tool(op, json) → Executor::plan_dyn → review card
//      → confirm (or --yes) → execute_dyn → ExtrinsicResult::to_json / table
```

- **Addresses.** `*_ss58` flags accept an ss58 string, a wallet name (`-w name` or `wallet/hotkey`), an address-book
  entry (`$BTCLI_ADDRESSES_PATH` or `~/.bittensor/btcli_addresses.json`), a proxy-book entry or a saved multisig
  (`btcli_proxies.json`, `btcli_multisigs.json`; `PY/config.py:13-15, 212`). This is a port of `PY/cli/context.py`
  `address_cli_name` and its helpers. `--hotkey` and `--coldkey` default to the configured wallet, and destinations
  never default (`UP/docs/cli.mdx:61-67`).
- **Lists** accept CSV or JSON (`PY/cli/tx.py:62-82`). **Money** accepts exact decimals or `all`.
- **MEV shield** precedence is required → `--mev-shield`/`--no-mev-shield` → config `mev_shield` → the intent's
  `mev_shield_default`. Shielding is downgraded when free TAO cannot cover the carrier fee
  (`PY/cli/context.py:260-292, 1047-1080`).
- **`query`** builds one subcommand per read, grouped by category. Options come from `params`. Output is a table
  following the `render` hints (`Grouped`, `Matrix`), or `--json` (§4.8). `--block <n>` runs the read on a `Snapshot`.
  btcli has no block flag (`PY/cli/query.py` [J-PD §3 D#8]), so this is an rtao addition (D-014).
- **`tools`** prints the catalog projection in `list_tools` shape (`PY/intents/registry.py:43-66`, 9 keys), plus an rtao
  `rust` object per entry (§13; an addition, D-029; interop lane 8 compares only the upstream keys).
  **`explain <code|ChainErrorName>`** prints code, description, remediation and docs URL.
  **`call Pallet.function --args '{…}' [--sudo] [--multisig NAME]`** is a raw call that uses the same wrapper nesting
  as intents (`PY/cli/call.py`). The CLI builds its executor with no `Policy`, as btcli does
  (`PY/cli/context.py:1990-1994`), so a raw call needs no extra flag; the library's `Policy::allow_raw_calls` still
  guards `submit_call` whenever a policy is set (§4.6). `--signer coldkey|hotkey` (alias `--key`; `call` owns
  `--signer`, so it also accepts a signing backend, `PY/cli/call.py:186-193`) goes through `submit_call_dyn`, and
  `--multisig`/`--multisig-threshold`/`--signatories`/`--other-signatories` through the `Multisig` account object
  (§4.6).

### 10.2 Global flags, configuration, shared files

- **Global flags** are a port of `PY/cli/globals.py`: `-n/--network`, `-w/--wallet`, `-H/--wallet-hotkey`,
  `--wallet-path`, `--wallet-password-file`, `--json`, `-y/--yes` (`--no-prompt`), `--dry-run`, `-q`, `-v/-vv/-vvv`,
  `--proxy-for`, `--force-proxy-type`, `--signatory`, `--signer wallet|ledger|vault|extension` (plus `remote`, an
  rtao addition that lands with `RemoteSigner` in M10, D-031; Python offers wallet, extension, ledger and vault,
  `PY/cli/globals.py:276-286`),
  `--signer-address`, `--ledger`, `--ledger-account`, `--ledger-index`, `--extension-bridge|browser|source`,
  `--keychain-password`, `--macos-password`, `--mev-shield/--no-mev-shield`, `--fallback-endpoints`,
  `--archive-endpoints`, `--retry-forever`.
  - v9 shorthands (`w`, `st`, `s`, `su`, `wt`, `cr`, …) are hidden aliases (`PY/cli/main.py:118-135`).
  - Precedence is flag > env > `btcli.json` > default.
- **Shared files.** rtao reads and writes btcli's seven files with the same schemas and environment overrides
  (`PY/config.py:12-18, 59, 139, 212, 274, 327, 355, 416`):

  | File (in `~/.bittensor/`) | Override | Content |
  |---|---|---|
  | `btcli.json` | `BTCLI_CONFIG` | settable keys `network`, `wallet`, `wallet_hotkey`, `wallet_path`, `json`, `quiet`, `signer_address`, `extension_browser`, `fallback_endpoints`, `archive_endpoints`, `retry_forever`, `proxy_for`, `mev_shield` (`SETTABLE`, `:30-56`); unknown keys are ignored |
  | `btcli_addresses.json` | `BTCLI_ADDRESSES_PATH` | address book |
  | `btcli_proxies.json` | `BTCLI_PROXIES_PATH` | proxy book |
  | `btcli_multisigs.json` | `BTCLI_MULTISIGS_PATH` | saved multisigs |
  | `btcli_multisig_cache.json` | `BTCLI_MULTISIG_CACHE` | call specs keyed by call hash (`:326-352, 470-480`) |
  | `btcli_subnet_names.json` | `BTCLI_SUBNET_NAMES_CACHE` | subnet-name cache, 24 h |
  | `btcli_token_symbols.json` | `BTCLI_TOKEN_SYMBOLS_CACHE` | token-symbol cache, 7 d |

  Writes are atomic, and corrupt input is retained at a unique `.corrupt.<ts>[.<ordinal>]` path before replacement.
  D-045 records the shared multisig book/cache adapter's bounded regular-file reads, private new-file modes,
  atomic publication and hash-authenticated cache lookup. Cached labels and params never authorize signing.
  D-047 keeps named pure-proxy creation coordinates after a successful approval that has not executed the kill.
  Ordinary dispatch, threshold-one dispatch or a successful `MultisigExecuted` receipt establishes retirement;
  failed kills retain the entry. No automatic transfer or reclaim is added.
  D-048 displays an unknown sudo approval layer when proposal assets could not
  be authenticated and its state was not queried. It only calls a layer unopened
  after a successful lookup reports no operation; machine fields are unchanged.
  A pending multisig's call data resolves as in
  btcli: the shared cache, then `--call-data`, then the opening `as_multi` at the timepoint, through
  `Client::block_info` (`PY/cli/multisig_helpers.py:907-975`). Homes:
  - `rtao_client::config` (M1): the paths with their overrides; `SETTABLE`, extracted with the builtin type tags of
    §4.9 layer 4; `btcli.json` load and save; the token-symbol and subnet-name caches (used at connect, §3.3 step 6).
  - `rtao-cli`'s `books` module: readers in M4, for `*_ss58` resolution in `tx` and `call`. Writers ship with their
    commands: the multisig book and multisig cache in M5, the proxy and address books in M8.

  A user can alternate between `btcli` and `rtao` on one machine, and the interop lane proves it (§11.6).

### 10.3 Output contract

- **Non-interactive discipline** ([R] G §4.5; `PY/cli/context.py:2088-2103`):
  - without `--yes`, in `--json` mode or without a TTY, a mutation is declined with exit 1;
  - missing required options exit 2;
  - JSON goes to stdout and diagnostics to stderr;
  - `-v` is INFO, `-vv` DEBUG, `-vvv` raw frames; `BITTENSOR_LOG` overrides.
- **JSON is the parity contract.** It is **value-equal** to btcli's: parsed JSON equality, with `Balance` strings
  byte-equal through §5.3, enforced by the read oracle and the interop lanes (lane 9 runs every hand-written command
  against btcli at a pinned block, §11.6).
  - btcli pretty-prints through Rich's `print_json(json.dumps(payload, default=str))` (`PY/cli/output.py:562` [V]). In
    M8, rtao adds a `PyJsonFormatter` for `serde_json`: 2-space indent, Python float `repr`, non-ASCII preserved. Output
    is then byte-equal in practice, but only value equality is gated [J-CF concern 12].
- **Human tables** (Rich in Python) are not claimed byte-identical. Content and column sets match; layout may differ.
- **Interactive flows** (`stake wizard`, pickers, prompts) match prompts and flags, not terminal rendering (D's Q4).

### 10.4 rtao-only commands

- `rtao doctor` runs these checks:
  - `Cargo.lock` of a downstream workspace: it reads the expected reference from the workspace's own `bittensor-core`
    entry (`source = "git+https://github.com/RaoFoundation/subtensor.git?tag=<T>#<sha>"`) and requires exactly one
    `w3f-bls` with the identical source string. The reference kind must match: a `rev` of the very same commit is a
    different Cargo source and locks a second copy, which fails with E0277 (re-measured: `tag = "v477"` against
    `rev = "5c6e83e2…"` locks two `w3f-bls 0.1.3` sources). On failure it prints `w3f-bls = { git =
    "https://github.com/RaoFoundation/subtensor.git", tag = "<T>" }`. It warns, without failing, when `<T>` differs
    from the doctor's own `BITTENSOR_CORE_TAG` (a doctor from another rtao release);
  - toolchain against MSRV;
  - per-network endpoint reachability, genesis, spec against `compat.json`, and `rpc_methods` capabilities;
  - clock skew;
  - wallet file permissions;
  - docker availability for `localnet`.
- `rtao localnet up|down|fund|subnet` drives the documented local loop with the pinned image digest
  (`UP/docs/guides/local-development.mdx:132-146`).
- `rtao wallet save-password-env` prints the export line (§6.2).

### 10.5 Hand-written groups (ported from `PY/cli/commands/*.py`)

| Group | Python sources | Notes |
|---|---|---|
| `wallet` | `wallet.py` (1,784 lines) | create, new-coldkey, new-hotkey, regen-*, unlock, show, list, balance, overview, registrations, history, transfer, inspect, sign, verify, encrypt, decrypt, identity, associate-hotkey, swap-hotkey, swap-coldkey, swap-check, announce-coldkey-swap, keychain save/show/delete, save-password-env |
| `stake` | `stake.py`, `stake_exit.py`, `stake_picker.py` | intent aliases plus show, list, auto, set-auto, wizard, child, children |
| `root` | `root.py`, `root_move.py`, `root_swap.py`, `root_helpers.py` | list, claim, allocate, register, basket moves and swaps |
| `subnets` | `subnets.py`, `metagraph_view.py`, `hyperparams_view.py` | list, show, hyperparameters, burn-cost, create-cost, price, conviction, register, create, metagraph |
| `hparams` (alias `sudo`, `su`) | `sudo.py`, `null_pruning.py` | set, get, get-take, set-take, check-start, start, set-symbol, identity, trim, trim-null(-batch), stake-burn, mechanisms |
| `conviction` (alias `lock`) | `lock.py` | |
| `addr`, `proxy`, `multisig` | `addresses.py`, `proxy.py`, `multisig.py`, `multisig_helpers.py` (1,455) | books, saved multisigs, approvals |
| `crowd`, `collateral`, `timelock`, `axon`, `weights`, `extension`, `upgrade`, `misc`, `config` | same-named files | `misc` also hosts hidden top-level aliases (`PY/cli/main.py:118-135`) |
| `evm` | `commands/evm/*` | feature `evm` |

The Typer command tree is dumped at `parity_tag` by the oracle (`vendor/oracle/cli_tree.json`): commands, options,
aliases, hidden flags. The parity matrix tracks every command and flag (§4.9).

### 10.6 Scope per milestone

| Milestone | CLI scope |
|---|---|
| M4 | the CLI machinery, 8,389 Python lines: ports of `context.py`, `globals.py`, `main.py` (the command tree, the snake_case alias mechanism of `:449-477`, which then covers every later command including generated `tx`/`query`, and the hidden v9 aliases of `:116-135` for the groups shipped so far), `output.py`, `prompt.py`, `intent_prompts.py`, `call.py`, `call_names.py`, `secrets.py`, `logs.py`, `tx.py` and `query.py`; the prompt module that replaces `masked_input.py`/`_live.py` (`rpassword` hidden entry under `PromptPolicy::IfTty`, re-asking on empty or mismatched input as `PY/keyfiles.py:42-62` does; per-character `*` echo is rendering and not gated); `--keychain-password`/`--macos-password` over the `macos_password.py` port (`/usr/bin/security` with Python's exact argv, and the `osascript` dialog); globals, `config` (5), `doctor`, `explain`, `tools`, `call`; the node-managing `localnet` command is deferred under Q30; generated `tx`/`query` for every implemented op and read; `wallet` create, new-coldkey, new-hotkey, regen-coldkey, regen-hotkey, regen-coldkeypub, regen-hotkeypub, show, list, balance, inspect; `weights` set, commit, reveal; `axon` set, reset; `hparams` set, get, get-take, check-start, start, set-symbol, get-identity, set-identity, trim, trim-null-batch, trim-null, mechanisms count, mechanisms emissions; `subnets` register, create, metagraph (40 commands; parity-inventory §7.2); the rtao-only `wallet save-password-env` (§10.4) |
| M5 | generated `tx` complete (83); `stake` add, remove, move, move-swap, transfer, swap, burn; `hparams` set-take, stake-burn (their intents are M5); `proxy` create, add, remove, kill, list, execute; `multisig` add, list, show, remove, pending (20 commands); hidden v9 aliases of the groups first shipped here |
| M6 | generated `query` complete (102); `subnets` read views; `root`; `conviction`; `collateral` (24 commands); hidden v9 aliases of the groups first shipped here |
| M7 | `--signer ledger/vault/extension` (`remote` with `RemoteSigner` in M10); `extension`; `evm`; `timelock`; `wallet keychain` save/show/delete over the M4 Keychain code (34 commands); hidden v9 aliases of the groups first shipped here |
| M8 | everything remaining in the dumped tree (`wallet overview/history/sign/verify/encrypt/decrypt/…`, `stake wizard`, `upgrade`, `misc`, `crowd`, `addr`, the `proxy book` subgroup; 45 commands); hidden v9 aliases of the M8 groups, then a check of every group's hidden v9 and snake_case aliases against `cli_tree.json`; the interactive pickers (`stake_picker.py`); `PyJsonFormatter`; completions; the gate flips to zero missing |

---

## 11. Testing

### 11.1 Layers

Q30 governs execution: ordinary Rust checks run in hosted CI; container and device-related layers below are
optional targeted hosted evidence or deferred hardware coverage. Former default infrastructure schedules are
retired, and none of these layers authorizes local nodes, containers or duplicate build matrices.

| Layer | What it proves | Runs | Needs |
|---|---|---|---|
| unit + property (§11.7) | units, conform, btauth canonicalization, era, nonce leases, shape hashes, typed == dynamic | every push | nothing |
| upstream goldens (§11.2) | chain-defined bytes through **rtao's wrappers**, not only the core | every push | nothing (metadata embedded in fixtures) |
| oracle vectors (§11.5) | Python behavior: composed calls, plans, reads, pure functions | every push (committed JSON) | nothing; Python only when regenerating |
| `FakeNode` (§11.3) | the session supervisor: reconnect, resend, refusals, `FateUnknown` | every push | nothing (loopback socket) |
| `FakeChain` (§11.3) | client, executor, reads, scheduler with a paused clock | every push | nothing |
| localnet e2e (§11.4) | real chain behavior | optional targeted hosted job | hosted containers only |
| core-oracle (§11.8) | rtao agrees with `bittensor_core::Client` where they should | optional targeted hosted job | hosted containers only |
| Python interop (§11.6) | cross-language wire, wallets, btauth, JSON | optional targeted hosted job | hosted containers only |
| Ledger emulator (§11.9) | the Ledger path against the real Polkadot generic app under Speculos: address, clear signing with the core's proof (including a payload above 256 bytes), rejection and error mapping, and Ledger-signed extrinsics accepted by a localnet built with `metadata-hash` | optional targeted hosted job | hosted containers only |
| fuzz, bench (§11.7) | robustness, performance regressions | optional targeted hosted job | bounded free CI resources |
| live probes (§12.6) | public endpoints and finney shape | optional targeted hosted job | read-only CI network access |

Localnet, interop, core-oracle and Ledger emulator tests live in the `[[test]]` targets `e2e`, `interop`, `core_oracle`
and `ledger_emu` of `rtao-testkit`, each with `test = false` and `required-features` (`docker`, `interop`,
`core-oracle`, and `docker` plus `ledger-emu`), as upstream keeps its own suite out of ordinary runs
(`UP/sdk/bittensor-core/Cargo.toml:13-18`). `.config/nextest.toml` sets
`[profile.ci] default-filter = "not (binary(e2e) | binary(interop) | binary(core_oracle) | binary(ledger_emu))"`, so the
`--all-features` hosted `pr-gates` run never starts them; an optional targeted hosted lane
workflows select them with `--test`. `test = false` alone already keeps them out of that run; the filter guards against
a target that loses the flag. The four targets are declared from M0 on, as empty files until their milestones fill them,
because nextest rejects a `binary()` predicate that matches no target in the workspace (exit 96 with nextest 0.9.143,
checked while planning); a declared target satisfies it even when `test = false`, `required-features` or `-p` leave it
unbuilt.

### 11.2 Upstream fixtures reused (vendored at `parity_tag`, sha256-pinned, Apache-2.0 with NOTICE)

| Fixture | rtao assertions |
|---|---|
| `PYT/fixtures/golden.json` (1,508,346 B; spec 461; embeds raw V15 and V14 metadata; [R] A2 §6.1) | `calls` (10, including nested `Sudo`/`Utility.batch`/`Proxy.proxy`) re-encoded through the **typed builders** must equal `data_hex`; `storage_keys` (10) through typed descriptors plus `Runtime::storage_key` must equal `key_hex`, for items still present (`SubtensorModule.Alpha` was removed after 461, [R] H §0.4); `signature_payloads` (immortal; mortal period 64) and `extrinsics` (2; fake signature `1..=64`) through rtao's `TxParams` construction must be byte-equal; `multisig` (3) and `ss58` (4) through `rtao-primitives`; `constants` (3: `Balances.ExistentialDeposit` 500, `Aura.SlotDuration` 250, `System.SS58Prefix` 42; `record_golden.py:115-119, 337-340`) decoded through `ViewExt::constant` and the generated `constants::*` at the golden metadata must equal `decoded`; `storage_values` (incl. the `NetworksAdded[9999]` miss → default), `query_maps`, `runtime_calls`, `events`, `block`, `payment_info` and `receipt` are served by `FakeChain` at the golden metadata, and rtao's dynamic reads, typed models, receipt resolution (outcome and fee) and fee decode must reproduce them. Loader: strip the `Option<OpaqueMetadata>` wrapper, then `Runtime::parse(v15, 461, 1, 42)` |
| `PYT/fixtures/shape_corpus/corpus.json` (797/802 types, 2,899 samples) | the core replay (`decode_id` + `to_corpus_json`) re-run against `core_tag` as a pin-consistency check; rtao's newtype and fixed-point visitors on every sample of their type paths; every typed model on every sample of its type id, with typed→`Value` projection equal to `decoded` |
| `UP/sdk/bittensor-core/fixtures/ledger_proof_vector.json` | `LedgerSigner` request construction (parts, unhashed payload, proof) byte-equal, with the device mocked at rtao's `LedgerBackend` seam (the core's `LedgerDevice` is a concrete type, `CORE/signers/ledger.rs:51-54`); the same request is signed on the emulator (§11.9). The port of `PYT/unit/test_ledger_signer.py` maps its no-context test n/a, because `SigningRequest` always carries its context (§6.3), and adds a case with a payload above 256 bytes |
| `CORE/timelock/epoch_schedule_vectors.rs` (private module, transcribed to JSON by `xtask import-upstream`) | the staged pallet-faithful `epoch_schedule` agrees on every upstream vector; new safety-net vectors where it intentionally differs (§9.3 step 8) |
| digest `affbcc25…11c0` over the golden V15 with `ChainInfo(461, "node-subtensor", 42, 9, "TAO")` (`CORE/digest/mod.rs:184-201`) | `metadata_digest` through `SigningContext` |
| PoW seals (`CORE/pow/mod.rs:88-105`), `twox_128("System")` (`CORE/mlkem/mod.rs:118-124`) | `PowSolver`, shield key hash |
| btauth vectors (`PYT/unit/test_http_auth.py:19-36`, `signed-requests.mdx:252-289`) | §7.1 |
| `PYT/unit/test_keyfiles_golden.py` (`//Alice`, the `abandon…about` mnemonic, legacy Fernet parameters, 0600/0700) + **cross vectors produced once in the interop lane** and committed to `vendor/recorded/keyfiles/`: `$NACL` with a known password and plaintext hotkeys from btcli (btcli writes only `$NACL`, `CORE/keyfiles/mod.rs:86-95`), legacy Fernet from `cryptography.fernet` with the salt and iteration count of `test_keyfiles_golden.py:72-77`, and Ansible from `ansible-vault` (§11.6) | the keyfile loaders, and the staged `keyfile_json` vs the core differential (§1.7). Upstream has no committed encrypted vectors ([R] B §8); PR-U13 proposes them |
| `PYT/unit/test_null_weights.py:13-45` and the other table-shaped Python unit tests (`test_slippage_protection`, `test_root_claim_fee`, `test_lock_ownership_gate`, `test_hyperparams_units`, `test_fee_filters`, `test_multisig_safety`, `test_prepare_attach`, `test_wallet_compat`, `test_balance_properties`, `test_rpc`, `test_http_auth`), and the behavior tests the oracle does not reach (`test_v446_accounting_and_commitments`, `test_pow_registration`, `test_cli_null_pruning`, `test_natural_name_key`, `test_facades`, and in M7 `test_ledger_signer`, `test_extension_bridge`, `test_extension_signer`, `test_evm`; 21 of the 34 files, python-module-map.md "Other watched roots") | ported test by test as Rust tests named after the Python test (`// pyt:` provenance), each with the milestone of the code it tests; table-shaped inputs are **extracted**, not copied |
| `UP/sdk/bittensor-core/tests/e2e-manifest.json` (112 = 73 `intent_*` + 39 `test_*` [V]) and the three `PYT/e2e/*.py` tests | `parity/e2e_map.toml` maps each manifest name and each Python e2e test to an rtao localnet test or n/a (§4.9); the 72 live intent-plan tests collapse into one table-driven test over `AnyIntent` (M5), which covers all 83 ops: it iterates the extracted `INTENT_SAMPLES` (`PYT/harness/samples.py:35`), rewrites `netuid` fields to the scenario's subnet and `*_hotkey_ss58` fields to the test's slot hotkey, and asserts that `plan()` returns `Ok`, that `plan.op` equals the op, that the call data is non-empty, and that the `(pallet, call)` decoded from its first two bytes at the live metadata is one of the op's catalog `wraps`; an op whose `plan()` needs chain state gets a seed step, never an exemption, and call-tree equality with Python stays in the offline oracle tests on the unrewritten samples; `intent_set_root_claim_type` (`e2e.rs:158-169`) asserts a retired core constructor with no catalog op (absent from spec-477 metadata) and is mapped n/a |
| `PY/_generated/{calls,storage,constants,runtime_apis,errors}.py` | **codegen cross-check**: rtao's generated descriptors agree with Python's generated ones on pallet indices, call and argument names and idents, storage names and value idents, and the `(pallet_index, error_index) → name` table. Both derive from the same metadata, so any difference is an rtao codegen bug |

**Spec-current fixtures.** The upstream goldens are frozen at spec 461 but stay valid because their metadata is
embedded. Current-spec fixtures come from **upstream's own recorders** ([R] A2 §6.1-6.2, D §12.3), which cannot run
unchanged: `record_golden.py` hard-codes `SubtensorModule.Alpha` (`:85`), which spec 477 no longer has (only `AlphaV2`,
`UP/pallets/subtensor/src/lib.rs:1975`), and `record()` has no try/except around `storage_entry` (`:169-181`;
`PY/_transport/codec.py:257-261` raises `StorageFunctionNotFound`); it refuses non-localhost endpoints (`:41-49`); and
`record_shape_corpus.py` runs offline over the registry in `tests/fixtures/golden.json` (`:12, 34, 152`). So
`xtask record` runs rtao's `ci/oracle/record.py` in the oracle container, joined to the localnet's network namespace
(`docker run --network container:<localnet>`), so the endpoint is `ws://127.0.0.1:9944` and the localhost guard
holds. `record.py` imports upstream's `scripts/record_golden.py` as a module, drops every `*_CASES` entry whose pallet,
item, call or runtime API is absent from the live metadata (listing them in `vendor/recorded/<spec>/dropped.json`;
today `SubtensorModule.Alpha`), calls `record()`, and writes the checkout's `tests/fixtures/golden.json`. It then runs
`record_shape_corpus.py`, which builds its registry offline from that file. Both outputs are copied to
`vendor/recorded/<spec>/`. The formats are upstream's, so rtao's loaders run unchanged on both generations. rtao files
an upstream fix that replaces `Alpha` with `AlphaV2` in `STORAGE_KEY_CASES` (Appendix D), and interop lane 4 uses the
same wrapper. A read-only finney pass records large metagraph payloads for `vendor/bench/` only.

### 11.3 Fakes at the right layers [J-PD concern 5]

D's `FakeChain` (a `MockRpcClient`) and C's (implementing `RpcClientT`) both sit **above** the layer that holds the
reconnect, resend and `FateUnknown` logic, so they cannot test it. rtao has two fakes over one shared state:

```rust
pub struct FakeState { /* metadata (pinned V15 or golden), runtime versions, scripted heads (advance(n), forks),
                          storage by key, runtime-API responses, submit scripts, event scripts */ }

/// A real JSON-RPC server on 127.0.0.1:0 (tokio-tungstenite). Exercises rtao-rpc end to end.
pub struct FakeNode { state: Arc<Mutex<FakeState>>, faults: FaultPlan }
pub enum Fault {
    HandshakeRefuse { status: u16, headers: Vec<(String, String)>, body: String },  // 403/429 + X-RateLimit-*, Retry-After
    DropAfterFrames(usize), Close { code: u16, reason: String },                      // e.g. 1008
    JsonRpcError { method: String, code: i64, message: String, data: Option<serde_json::Value> }, // -32004/-32005
    Silence(Duration), StateDiscarded { for_hash: H256 }, SlowResponse(Duration),
}
/// Implements subxt_rpcs::client::RpcClientT directly over FakeState: fast, for client, executor, reads, scheduler.
/// A Client runs over it through Client::from_rpc(RpcClient::new(fake_chain), cfg) (§3.3).
pub struct FakeChain { state: Arc<Mutex<FakeState>> }
```

- **`FakeNode`** mirrors `PYT/harness/fake_node.py` and runs the port of `PYT/unit/test_rpc.py` plus the §3.15
  checklist.
- **`FakeChain`** mirrors `fake_substrate.py`, one layer lower: rtao's runtime cache, key building, decoding and
  submission state machine run for real.
- **Seeding.**
  - Typed seeds use descriptors, for example `seed(&TEMPO, (NetUid(1),), 360u16)`.
  - Python-shaped JSON is encoded through the core's lenient encoder (ss58 strings for accounts, variant names for
    enums; `CORE/codec/encode.rs:1-5`).
  - Partial dict seeds go through `fill_defaults(type_id, Value)`, which walks the registry and fills absent composite
    fields with zero values (A §11.3).
  - The **extracted** `DEFAULT_STORAGE`, `DEFAULT_CONSTANTS` and `DEFAULT_RUNTIME` of `fake_substrate.py:34-77` load
    unchanged (their `10**9` and `10**7` resolve through §4.9 layer 4), so oracle inputs and rtao inputs are identical.
  - Per-op and per-read seeds come from the oracle dump, not from hand copies of Python test code. The oracle's seeds
    are imperative Python (`PYT/unit/test_intents_table.py:27-31, 44-60, 99-105`), and every plan's fee is
    `FakeSubstrate`'s constructor attribute `Balance.from_rao(124_414)` (`PYT/harness/fake_substrate.py:147`). So each
    dump records, per sample, the `FakeSubstrate` state it ran with (storage seeds including `seed_default`, constants,
    runtime-API results and the fee) as Python-shaped JSON in `vendor/oracle/seeds/<op|read>.json` (§11.5).
    `FakeState::from_python_seed` loads it through the core's lenient encoder, and `FakeChain` answers
    `TransactionPaymentApi_query_info` with the dumped fee.
- **Scripts** cover the submit lifecycle (`InBlockThenFinalized { events }`, `Invalid(reason)`, `Usurped`, `Dropped`,
  `Retracted`, `FinalityTimeout`), finality lag (best − finalized) for the weight and shield tests, and
  `tokio::time::pause` drives schedulers deterministically.
- **`RecordingView`** wraps any view and records every descriptor touched and every block hash used. Two tests use it:
  It attaches a `ReadObserver` to pinned `Snapshot` clones, so `pin()` and dynamic dispatch retain tracing. Storage
  observations resolve raw keys and map prefixes through the core's metadata key builder, including empty scans;
  constants are observed where decoded, and runtime calls retain their API/method identity. Each `ReadTouch` records
  its `ReadKind`, pallet or API, item and optional pinned hash. Unpinned metadata reads retain `None` as their block
  identity.
  - *exact hits*: every recorded run of a read touches a subset of its hit set H (extracted ∪ reviewed extras, §4.8),
    and the union of the runs over the read's samples (including per-branch samples) equals H;
  - *single block*: every RPC of a multi-RPC read used one hash (D-002).

### 11.4 Localnet e2e (docker)

Reference for optional targeted hosted checks under Q30. No node or container runs locally, and the former
every-push/nightly/weekly lane schedule is retired. Use a bounded hosted job only for a concrete client question.

- **Harness.** `rtao-testkit::Localnet` uses the plain `docker` CLI, as upstream does
  (`UP/sdk/bittensor-core/tests/e2e_support/mod.rs:23-147`):
  - `docker run -d --rm -p 127.0.0.1::9944 -p 127.0.0.1::9945 <image> [True|False]`, then `docker port`;
  - readiness is the image healthcheck turning healthy **or** `Imported #1` in the logs, with a 180 s cap, **and**
    `Drand.LastStoredRound > 0` within 30 s, as upstream's e2e waits (`UP/sdk/bittensor-core/tests/e2e.rs:548-563`;
    `PYT/e2e/test_timelock_commitment.py:38-43`). Commit-reveal and timelock scenarios depend on pallet-drand's
    offchain worker fetching quicknet pulses from public HTTPS endpoints (`UP/pallets/drand/src/lib.rs:81-92, 275`).
    On timeout the harness raises `LocalnetError::DrandUnreachable`; the job retries once with a fresh container, then
    fails with an infrastructure annotation. The e2e job first checks egress with
    `curl -fsS https://api.drand.sh/52db9ba70e0cc0f6eaf7803dd07447a1f5477735fd3f661792ba94600c84e971/public/latest`,
    and the book's local-development guide notes that a localnet needs outbound HTTPS to the drand endpoints;
  - `docker rm -f` on drop.
  The env contract is copied verbatim so upstream docs apply: `E2E_ENDPOINT` reuses a running node, plus
  `LOCALNET_IMAGE` and `SKIP_PULL`. The default image is `upstream.toml [localnet].pinned` by digest, never upstream's
  non-existent `monorepo-sdk` tag ([R] H §6.1).
- **Lanes:**

  | Lane | Image | Status |
  |---|---|---|
  | `pinned` | `:v<parity spec>` | optional targeted hosted fast/slow check (Q30) |
  | `mainnet` | `:mainnet` | optional targeted hosted check (Q30) |
  | `testnet` | `:testnet` | deferred default campaign; optional targeted hosted check (Q30) |
  | `devnet` | `:devnet` | optional targeted hosted check (Q30) |
  | `main` | `:main` | optional targeted hosted check (Q30) |
  | `wasm-override` | latest node + a released `subtensor.wasm` via `SUBTENSOR_LOCALNET_RUNTIME_WASM` (`UP/node/src/chain_spec/localnet.rs:6-26`, [R] H §1.3) | a manual fallback, not a CI lane. A released wasm is the production srtool build (`--features=metadata-hash` only, `UP/.github/workflows/release-train.yml:83-90`), so it boots with 12 s slots, tempo 360 and a 7,200-block network rate limit (`UP/runtime/src/lib.rs:904, 967`; `UP/pallets/subtensor/src/lib.rs:1022-1027`). It runs only the 12 s profile, with the bootstrap knobs, `sudo_set_network_rate_limit(0)` and `sudo_set_tempo` per subnet. Upstream publishes `:v<spec>` on every mainnet mirror move (`UP/.github/workflows/docker-localnet.yml:7-9`; GHCR has v432, v473, v475, v477), so it is rarely needed |

  Fast blocks (`True`: 250 ms, tempo 10) are the default. The 12 s lane (`False`) runs timing-sensitive tests: reveal
  rounds, rate-limit math, metagraph cadence, `wait_for_epoch`. Commit-reveal scenarios on the fast lane set root
  `sudo_set_tempo(netuid, 48)`, which makes the reveal epoch 12 s, or 4 drand periods, so one late pulse cannot expire
  a commit and "every reveal observed" stays a strict assertion. At tempo 10 the reveal epoch is 2.5 s, shorter than
  one 3 s drand period (`CORE/timelock/constants.rs:20`), and a missing pulse is retried only within the reveal epoch
  (`coinbase/reveal_commits.rs:52-58, 72-88, 204-206`).
- **Isolation and `Scenario` builder** (C).
  - `e2e.yml` starts the containers itself: one per nextest partition plus one `global` container. nextest runs every
    test in its own process and test groups only cap concurrency (setup scripts are still experimental), so a
    container held "per test binary" could not be shared; upstream's harness starts one per `TestContext::new()`
    (`UP/sdk/bittensor-core/tests/e2e_support/mod.rs:33-50, 62-139`).
  - Per container, `cargo xtask localnet bootstrap --endpoint <ws>` runs once, as Alice through `Sudo.sudo`:
    - the global knobs `sudo_set_admin_freeze_window(0)`, `sudo_set_owner_hparam_rate_limit(0)`,
      `sudo_set_network_rate_limit(0)`, `sudo_set_tx_rate_limit(0)`, `sudo_set_start_call_delay(0)`,
      `sudo_set_network_min_lock_cost(1 TAO)`, `sudo_set_lock_reduction_interval(1)` and
      `sudo_set_subnet_limit(u16::MAX)`. With the default freeze window of 10 blocks
      (`UP/pallets/subtensor/src/lib.rs:1436-1438`) and the fast tempo of 10 (`UP/runtime/src/lib.rs:904`), owner and
      root setters would be refused in every block but one per epoch (`utils/misc.rs:59-71`;
      `UP/pallets/admin-utils/src/lib.rs:444-454`), and `SubnetLimit` 128 with a 1,296,000-block network immunity
      (`lib.rs:1454-1456`; `runtime/src/lib.rs:962`) would make registration 129 in a shared container fail with
      `SubnetLimitReached`. Upstream's own e2e zeroes the same knobs (`PYT/e2e/test_null_consensus.py:39-55`,
      `test_pow_registration.py:33-38`). Every setter exists in spec-477 metadata (AdminUtils calls 2, 3, 7, 14, 29,
      30, 35-38, 74, 75, 85);
    - one `Utility.batch_all` from Alice that funds 64 slot accounts (`//rtao-e2e//slot/<n>` and
      `//rtao-e2e//slot/<n>//hot`).
  - The workflow then runs `cargo nextest run -p rtao-testkit --features docker --test e2e --profile e2e-<blocks>
    --partition hash:<k>/<n>` with `E2E_ENDPOINT` set. A test signs its own transactions only with the accounts of its
    `NEXTEST_TEST_GLOBAL_SLOT` (nextest ≥ 0.9.90), and each test gets a fresh subnet.
  - Per subnet, the `Scenario` builder sets `sudo_set_weights_set_rate_limit(netuid, 0)`,
    `sudo_set_serving_rate_limit(netuid, 0)` and `sudo_set_min_allowed_weights(netuid, 0)`, sent by the subnet owner
    where the setting is owner-settable and by Alice otherwise (`sudo_set_weights_set_rate_limit` is root-only,
    `admin-utils/src/lib.rs:479-484`). On the 12 s lane it also sends root `sudo_set_tempo(netuid, 10)`, so 3 epochs
    plus the reveal take about 9 minutes instead of about 4.8 hours at tempo 360.
  - Every Alice (Sudo) submission a test still needs, including those root-only per-subnet knobs, goes through
    `Localnet::sudo()`. It holds an exclusive advisory file lock (`target/e2e/<container>.alice.lock`) from the
    `account_nextIndex` fetch until inclusion, so concurrent test processes never share Alice's nonce (a collision
    would fail outright: Python's `execute` defaults to `retries=0`, `PY/executor.py:785, 832`).
  - Tests that change global state (scenarios 6, 8, 10's queued path and planner, and 16) are in test group `global`
    (`max-threads = 1`) and run in a second invocation against the `global` container. There, scenario 10's planner
    test sets `sudo_set_admin_freeze_window(5)` and `sudo_set_tempo(netuid, 40)`, so waiting for the window is
    exercised.
  - `rtao localnet up` gives developers the same setup.
  - `Scenario::new(&ln).owner().subnet().start().validator().miner().stake(tao(1_000)).build().await`. `Scenario` builds
    its setup calls with the generated typed builders (`rtao_runtime::calls::{subtensor_module, admin_utils}`), wraps
    root-origin calls with `rtao_client::compose::sudo` signed by Alice through `Localnet::sudo()`, and submits through
    `Client::submit`. It never goes through intents: `rtao-testkit` does not depend on `rtao-intents`, and most setup
    intents land in M4 and M5. Its calls are `SubtensorModule.register_network`, `start_call`, unshielded
    `burned_register` and `add_stake`, and the per-subnet `AdminUtils` knobs (`sudo_set_weights_set_rate_limit`,
    `sudo_set_serving_rate_limit`, `sudo_set_min_allowed_weights`, `sudo_set_commit_reveal_weights_enabled` for
    scenario 2, `sudo_set_mechanism_count` for 3, `sudo_set_weights_version_key` for 5, `sudo_set_commit_reveal_version`
    for 6); the global knobs come from the bootstrap.
- **Inventory.** Every upstream e2e manifest name and every `PYT/e2e` test is mapped in `parity/e2e_map.toml` (§4.9):
  `test_null_consensus.py::test_null_yuma_null_emissions_via_python_sdk` → scenario 4,
  `test_pow_registration.py::test_pow_register_unfunded_coldkey_and_owner_toggle` → scenario 14,
  `test_timelock_commitment.py::test_timelocked_commitment_reveals_via_python_sdk` → scenario 19 (where the manifest's
  `test_timelocked_commitment_reveals_plaintext` also maps). A key stays `pending = "M<k>"` until its rtao counterpart
  exists (§4.9). The rtao-specific tests are:
  1. a validator with CR on for 20 fast epochs and 3 slow ones: a commit per epoch, **every reveal observed**
     (`TimelockedWeightsRevealed`), weights read back, incentives move;
  2. a validator with CR off: direct sets respect `WeightsSetRateLimit`;
  3. two mechanisms: per-mechanism commits and `LastUpdate[idx]`;
  4. Null consensus: raw u16 rows, sole permit holder, trim-null batches;
  5. the owner bumps `WeightsVersionKey` mid-run: the validator refuses with remediation, is reconfigured, resumes;
  6. **the CR version gate**: sudo sets `CommitRevealWeightsVersion = 5`; the framework refuses with
     `IncorrectCommitRevealVersion` before submitting, while the `SetWeights` intent (Python parity) gets the dispatch
     error from the chain;
  7. a lost watch: the session test hook drops the socket mid-commit; reconciliation is definitive and later nonces
     are correct;
  8. a **runtime upgrade mid-session** (M2; the offline-payload assertion joins in M5 with `prepare_call`): while a
     validator runs, sudo `System.set_code` with the fast-runtime wasm
     (`/target/fast-runtime/release/node_subtensor_runtime.compact.compressed.wasm`, `UP/Dockerfile-localnet:103, 115`)
     copied out of the `[localnet].main` image by digest. Expected: the codec switches within about a block, the surface
     is re-verified, the `WeightScheduler` pauses on `SurfaceChanged` for the weight calls and resumes (§9.11),
     `execute` re-plans once (D-013), and offline payloads prepared before the upgrade are refused (D-008).
     `frame_system` refuses a `spec_version` that does not increase (`can_set_code`,
     `substrate/frame/system/src/lib.rs:2320-2349` in the polkadot-sdk fork checkout `cacb431` that the v477 build
     resolves), so when the `:main` runtime's spec is not above the pinned one the test is skipped with that reason
     printed, never passed silently;
  9. a miner: idempotent `serve_axon`, rate-limit scheduling, the 401/403/413/429 paths;
  10. an owner: create (immediate **and** queued; the queued path runs in the `global` container: root
      `sudo_set_network_immunity_period(0)` (AdminUtils call 35), then
      `sudo_set_subnet_limit(<current non-root subnet count>)` (call 37), so the next `register_network` prunes the
      lowest-priced subnet and emits `NetworkRegistrationQueued`, and the follower waits for that registration's
      `NetworkAdded`; `UP/pallets/admin-utils/src/lib.rs:1215-1228, 1250-1258`,
      `UP/pallets/subtensor/src/subnets/subnet.rs:204-213, 233-260`, `coinbase/root.rs:358-361`; §15.2 Q10), start, the
      hyperparameter planner waiting for the admin window, mechanisms, identity, symbol, trim. The mechanism count is
      changed twice on one subnet, and `earliest_block` must equal the first block the chain accepts (§9.7);
  11. a shielded `burned_register` (M4) and `add_collateral` (from M5), resolved through the inner extrinsic;
  12. multisig 2-of-3, pure proxy, announced proxy, `batch_all` atomic revert, and raw-call policy refusal;
  13. offline prepare → sign → `submit_signature`, refused across a runtime upgrade;
  14. PoW registration with lowered difficulty (12 s lane);
  15. **D-001 sanity check**: an unregistered hotkey's `Owner` decodes to 32 zero bytes on a live chain. The override
      itself is applied by rule over the pinned metadata and pinned by an offline test (§5.1);
  16. **PR-U4 evidence** (`ci/pr-u4-probe/`). Inside rtao's workspace a crates.io-`w3f-bls` build is impossible: the
      root `[patch.crates-io] w3f-bls` redirects every crates.io request, `lock.rs` and `deps-check` rule 6 require
      exactly one `w3f-bls`, and the core inherits `w3f-bls` from the monorepo workspace
      (`UP/sdk/bittensor-core/Cargo.toml:61-62`). So the probe is a separate Cargo workspace with its own `[workspace]`
      table, no `[patch]` and its own lock, listed in the root `exclude`. `cargo xtask pr-u4-proof prepare` creates a
      git worktree of the monorepo at `core.commit` and rewrites the core's `Cargo.toml:62` to
      `w3f-bls = { version = "=0.1.3", features = ["std"] }`, PR-U4's change; the probe depends on that core by path.
      For deterministic bytes it also reproduces `WeightsTlockPayload` and `encrypt_and_compress`
      (`CORE/timelock/mod.rs:53-58, 104-140`) over `tle` (`ideal-lab5/timelock` rev `5416406c`, `std`) with an
      injectable ephemeral key and RNG, because the core draws both from `OsRng` (`:124-130`). It writes commits for
      chosen past and future rounds to `vendor/recorded/pr-u4/`.
      (a) Offline, rtao's workspace decrypts the past-round commits through `decrypt_with_signature` with committed
      quicknet signatures (`vendor/recorded/drand/quicknet-<round>.json`), and checks byte-equality against the
      vendored path under the same fixed key and seeded RNG.
      (b) On localnet, the probe writes a `commit_timelocked_weights` ciphertext for a future round, rtao submits it,
      and the test asserts `TimelockedWeightsRevealed`.
      `e2e.yml` builds the probe in a step of its own. This is the precondition for dropping the downstream patch
      (§1.3);
  17. a pipeline of 50 transfers from one account, split across two `Executor`s built from clones of one `Client`,
      with no nonce gap or collision (§3.9.1);
  18. **residual ops** (M5, `pinned` lane): submit `move_swap_stake`, `stake_into_basket`, `swap_basket`,
      `swap_basket_many`, `set_reject_locked_alpha`, `set_min_collateral` (on a subnet whose owner set
      `collateral_lock_share`) and, signed by Alice as sudo, `set_root_claim_threshold` and
      `set_subnet_emission_enabled`. Assert the chain effect through the op's `verify` read where one is declared
      (`root_claim_threshold`, `subnet_emission_enabled`) and through the touched storage item otherwise. These 8 ops
      have no upstream e2e test, and the two Sudo-wrapped ones exercise the `Sudid` receipt path;
  19. **timelocked commitment** (M7): a port of `PYT/e2e/test_timelock_commitment.py` through `rtao_tx::timelock`
      (`seal(.., Reveal::Round(r))`, `Timelocked::encrypted()`) and `submit_call_dyn` with the hotkey role
      (`Commitments.set_commitment`), then polling `RevealedCommitments`.

### 11.5 The Python oracle (CI-only; outputs committed)

The pinned Python SDK is the executed specification. Python never gates a build of rtao or of a subnet binary
[J-PD concern 13]. The user accepted Python in these CI-only lanes, and nowhere else (§15.2 Q2).

- **Build.** `ci/oracle/Dockerfile` checks out the monorepo at `parity.commit` and runs `uv sync` in `sdk/python`. That
  builds `bittensor-core-py` from the same commit, through maturin, OpenSSL and Perl (a `[tool.uv.sources]` path
  dependency, [R] B §7). The image is cached per tag in GHCR. There is no PyPI fallback: if the image cannot be built,
  the bump is marked `oracle-stale` and the committed vectors from the previous tag stay authoritative (§12.3). No
  PyPI version reliably identifies a tag: `sdk/python` stayed at `11.3.0.dev0` across v464-v476, whose Python trees
  differ, so a `.dev0` → stable mapping would run v475's Python as v473's specification; and v477's `11.3.1` (with
  `bittensor-core` 0.1.6) is not on PyPI, because stable publication waits for a manual `mainnet` environment approval
  (`UP/.github/workflows/watch-mainnet-release.yml:19-21, 370-377`). (`bittensor==11.3.0` does equal v475's tree file
  for file, but only because it was published from that commit.)
- **Dumps** (`ci/oracle/dump_*.py`, committed to `vendor/oracle/`):
  - **intents**: for each of the 83 ops, from `INTENT_SAMPLES` plus the table test's per-op seeds
    (`PYT/unit/test_intents_table.py:28-60`), plus `vendor/oracle/samples_extra.json` (M5): root-subnet samples, dumped
    like upstream's, for `remove_stake` and `remove_stake_limit` with `netuid: 0` and `move_stake`, `swap_stake`,
    `move_swap_stake` and `transfer_stake` with `origin_netuid: 0`, each with a number and, where allowed, `"all"`, and
    `move_stake {origin_netuid: 0, claim: true}` and `unstake_all {claim: true}` for the root-claim branches
    (`PY/intents/staking.py:306-345`). Every upstream staking sample uses netuid 1 or 2 (`PYT/harness/samples.py:43-76`),
    so without these the root branches would go unchecked. For each sample: the composed call tree
    (`FakeSubstrate.compose` returns `ComposedCall(module, function, params)`, `PYT/harness/fake_substrate.py:314-318` [J-CF §3.2]), effects, warnings,
    blocks, violations, fee, spend, `required_free`, extras, the `plan()` JSON, the JSON schema, and the `FakeSubstrate`
    state the sample ran with (storage seeds including `seed_default`, constants, runtime-API results and the fee) in
    `vendor/oracle/seeds/<op>.json`, which `FakeChain` loads (§11.3);
  - **reads**: for each of the 102 `READ_SAMPLES`, results through `_jsonable`, at head and pinned at block 90
    (`PYT/unit/test_reads_table.py:100-110`), with the seeds in `vendor/oracle/seeds/<read>.json` and the chain items
    `FakeSubstrate` served (`touched`, the cross-check of §4.8);
  - **pure functions**: weights `normalize`, `clip_to_max_weight` and `_conform` over seeded and edge inputs (including
    `.5` ties and the cutoff path); `hyperparams.to_raw/normalized/annotate`; `str(Balance)` over 10,000 rao values
    (§5.3); `classify_error` over a message corpus; `chain_error_from_substrate_request` and `classify_rpc_error` over
    recorded payloads (policy, state-discarded, custom codes); `fee_filters.charges_coldkey_fee`;
    `timelock.round_at/reveal_time`; `pow_registration.registration_prefix/seal`; `metagraph._build` over recorded
    payloads; `utf8_text`; btauth negative messages; from M7, the `vault.uos` framing functions `sign_tx_payload`,
    `sign_frame`, `multipart_frames`, `transaction_frames` and `transaction_frames_with_proof`
    (`PY/vault/uos.py:54-123`), and the device calls of Python's `LedgerSigner`: `ci/oracle/dump_ledger.py` swaps its
    device class for a recording fake, as `PYT/unit/test_ledger_signer.py:34-54` does, and dumps the
    `(payload, proof, account, index)` it sends for the golden calls with an immortal and a mortal era and for a batch
    above 256 bytes (`vendor/oracle/ledger.json`, compared by `ledger_request_parity`, §11.9);
  - **CLI**: the Typer command tree (`cli_tree.json`).
- **Rust side** (ordinary `cargo test`, offline): `FakeChain` is seeded identically, then rtao's plans, preflights,
  reads and pure functions are compared:
  - composed call trees exactly, after normalization (accounts as ss58, nested calls recursively), excluding timelock
    ciphertexts and reveal rounds, which are randomized or clock-derived;
  - violations, effects, warnings, spend and fee exactly;
  - reads as **parsed JSON values**;
  - u16 weight rows bit for bit.
  A mismatch prints both sides plus the `parity/map.toml` row and the Python source path.
- **Regeneration** happens only in pin bumps. The diff of `vendor/oracle/*.json` between tags is then a readable
  changelog of Python behavior changes, and it feeds the auto-bless rule (§4.9).

<a id="116-python-interop-e2e-docker-compose-nightly-and-on-pin-bumps"></a>

### 11.6 Python interop e2e (optional hosted)

An optional bounded hosted job can use `ci/interop/compose.yml` with a pinned localnet image and oracle (`btcli` and a
Python package; the drivers are scripts because the package has no `__main__`) plus rtao test binaries. Select
only the case needed by the changed client. No automatic schedule or milestone acceptance depends on this job.
The available scenarios:

The oracle image does not have what the lanes need: `uv sync` of `sdk/python` installs no `fastapi`, `uvicorn`, `httpx`,
`cryptography` or `ansible-vault` (none is in `sdk/python/uv.lock`), v11 ships no miner or validator runtime (`PY/`
has no Axon, Miner or Validator class), and btcli writes only `$NACL` keyfiles (`CORE/keyfiles/mod.rs:86-95`). So
`ci/interop/pyproject.toml` with a committed `uv.lock` pins `fastapi`, `uvicorn`, `httpx`, `cryptography` and
`ansible-vault`, installed as an extra layer on the oracle image. `ci/interop/py/{miner,client}.py` are generated at
`parity_tag` from the FastAPI and httpx code blocks of `signed-requests.mdx` (`:111-141`, `:169-197`), and
`validator.py` is rtao-authored (metagraph through `bt.Client`, weights through `execute_tool("set_weights", …)`).

1. **btauth/1 in both directions**, for sr25519 and ed25519. An rtao `SignedClient` talks to the FastAPI recipe miner
   generated from `signed-requests.mdx:111-141`, and the generated httpx client (`:169-197`) talks to an rtao axum
   `BtAuthLayer`.
   Cases: GET with an empty body, POST JSON, a percent-encoded target, an unbound receiver with and without opt-out,
   stale, future, replay, a tampered body. Status codes and `detail` strings are asserted.
2. **A v11 Python miner and an rtao validator**, and **a Python validator and an rtao miner**, on a localnet subnet.
   Weights are set and observed. Python's `subnets.metagraph(netuid)` JSON equals `Metagraph::to_json()`.
3. **Wallets.** btcli creates wallets (`--n-words 12`, with `BT_WALLET_PASSWORD` and with `--no-password`) and rtao
   loads and signs with them. rtao-created wallets are read by `btcli wallet show/inspect/sign/verify`. Legacy Fernet
   files (from `cryptography.fernet` with the salt and iteration count of `PYT/unit/test_keyfiles_golden.py:72-77`) and
   Ansible files (from `ansible-vault`) decrypt. Keychain service/argv, password timing and failures use injected
   process tests in hosted CI. The extraction layer pins `PY/macos_password.py`'s service name and argv. Real
   Keychain compatibility and temporary-default-keychain rehearsals are deferred by Q30; no host settings change.
4. **Wire bytes at the current spec.** Python emits call data, signature payloads and fake-signed extrinsics for a fixed
   call set (`record_golden.py` through the `ci/oracle/record.py` wrapper of §11.2), and rtao byte-compares.
5. **Reads at a pinned block.** A Python driver calls `(await client.at(b)).read(name, **params)` for every sample and
   renders it with `_jsonable`. rtao runs `rtao query <read> --block b --json`. The results must be value-equal.
6. **Prepared extrinsics**: Python `prepare_call` → rtao `submit_signature`, and the reverse.
7. **Shared state**: the runtime disk cache, the token-symbol cache, `btcli.json`, the address/proxy/multisig books and
   the multisig call cache written by one side are read by the other. A raw-call 2-of-3 multisig opened by btcli is
   approved and executed by rtao, through the shared cache and again through the timepoint fallback with the cache
   removed (§10.2), and the reverse.
8. **CLI**: `btcli tools` vs `rtao tools` on the 9 upstream keys (`tools_json_equals_python_on_upstream_fields`, D-029);
   `--dry-run --json` plans for every state-independent intent sample; `cli_tree` parity.
9. **CLI JSON at a pinned block.** `--json` is a global flag (§10.2), so every hand-written command has a JSON
   contract, and lanes 5 and 8 cover only reads, `tools` and `tx` plans. Both CLIs connect (`--network local` with
   `BT_CHAIN_ENDPOINT`, `PY/settings.py:38`) to `rtao-testkit::PinProxy`, a JSON-RPC WebSocket proxy built on the
   `FakeNode` server code that forwards to the localnet but answers head queries (`chain_getHead`,
   `chain_getFinalizedHead`, `chain_getHeader` and `chain_getBlockHash` without arguments) with one pinned block H,
   adds H to every hash-less `state_*` request (Python queries the head without a hash,
   `PY/client.py:206-207`, `PY/_transport/interface.py:179-193`), and refuses `author_*`. Every read-only command of
   `cli_tree.json` runs with `--json` on both sides, and every mutating one with `--dry-run --json`; the outputs are
   compared with the oracle comparator of §4.8. Fields derived from the wall clock are listed with a Python citation in
   `parity/cli_json_volatile.toml` and skipped. Each command joins the lane with its milestone (M4-M8); M8 exits with
   every command green or registered as a divergence.

Lanes 1-4 land in M3; lane 8, the first reads of lane 5 and the first commands of lane 9 in M4; lanes 6 and 7 in M5
(the address and proxy books join lane 7 in M8); lane 5 completes in M6 and lane 9 in M8 (PLAN.md §12).

Ledger request parity is not an interop lane: Python's `LedgerSigner` reaches only HID (`PY/ledger.py:44-51, 82`), so
the oracle dumps what it sends to the device instead (§11.5), and the emulator lane proves that the app accepts those
bytes (§11.9).

### 11.7 Property tests, fuzzing, benchmarks

Q30 retains property tests in ordinary hosted tests. Fuzz/performance campaigns are optional bounded hosted
checks for a specific change; no local campaigns, default nightly matrices or M9 time escalation.

- **`proptest`:**
  - `Balance` parsing and same-unit arithmetic; a mixed netuid returns `UnitMismatch`;
  - `pyfmt` against oracle vectors;
  - fixed-point `to_f64` against `bits / 2^F`;
  - the `NetUidStorageIndex` round trip;
  - era encoding against the core's `era_birth`;
  - `IntoCoreValue` round trips through `encode_id`/`decode_id`;
  - `conform` invariants: max = 65535, no zeros, order preserved, clipped max/sum ≤ limit, the self-weight exemption,
    and determinism;
  - nonce leases: random interleavings of lease, settle and drop never reuse a nonce or leave a gap;
  - shape-hash soundness: mutations (field rename, reorder, compact flip, new variant, array length) change exact
    hashes, and path- or doc-only edits do not; appended fields leave projection hashes unchanged;
  - typed == dynamic over `ByteGen` (§8.1);
  - btauth canonicalization at the boundaries (exactly `max_age` and `allowed_skew`);
  - the scheduler never double-submits inside a rate limit under random head sequences.
- **`cargo fuzz`** (optional bounded hosted checks): btauth header parsing and `verify`, JSON-RPC frame parsing in
  `rtao-rpc`, `MetagraphView` (arbitrary bytes and mutations of real payloads), event splitting, `UnsignedExtrinsic`
  JSON import, keyfile detection and `keyfile_json`, and `decode_extrinsic`/`decode_id` wrapped at the boundary to
  catch panics. Keep ordinary failure regressions in hosted Rust tests as each parser lands. A targeted fuzz run
  uses a free hosted job and explicit time limit; no default campaign or longer M9 run is required.
- **`criterion`** baselines on `vendor/bench/` (B's committed finney netuid 1, 3 and 64 payloads plus the synthetic
  2,500-UID payload):
  - metagraph decode via core `Value`, `DecodeAsType` and `MetagraphView`;
  - `Runtime::parse` (V15);
  - `fetch` key building for 10k keys;
  - `query_map` page decode;
  - sign + assemble (the core spec reports 0.03 ms);
  - btauth sign and verify throughput;
  - `conform` at 256 and 2,500 entries.

  Shared-runner timing comparisons are informational. Live benchmark campaigns are outside current work (Q30).

### 11.8 `core-oracle` differential lane

On one localnet block, rtao and `bittensor_core::Client` (in `spawn_blocking`, under
`#[allow(clippy::disallowed_methods)]`) must agree on storage values, runtime-API results (`metagraph`, `neurons`,
`subnet_hyperparameters`, `stake`), `compose_call` bytes for 21 of the 22 core constructors (paired as in §4.6;
`set_root_claim_type` is retired and excluded), and `Policy::check` violation sets. Fees are compared on identical
extrinsic bytes. The core's `estimate_fee` signs with a 64-block mortal era while rtao uses Python's immortal probe, so the lane passes rtao-built bytes through the core's `rpc_value("payment_queryInfo",
…)` escape hatch (P-A §1.4).

### 11.9 Ledger emulator lane (Speculos)

Q30 defers hardware acceptance and makes emulator execution optional hosted evidence. The researched shim is Speculos, Ledger's device emulator, running the Polkadot generic app headless. While planning, it clear-signed
requests built by `bittensor-core` in two independent runs on three device models, and every signature verified
(`RES/J-ledger-ci-shim.md` §1). The core's `LedgerDevice` can only open USB HID, which Speculos lacks (G16), so PR-U17
adds a transport seam, and rtao's workspace takes it from the core patch until PR-U17 ships (§1.11).

- **Harness** (`rtao-testkit::ledger_emu`, feature `ledger-emu`). `ci/ledger/pins.toml` holds the app source
  (repository, tag, commit), the builder image and Speculos (`v0.27.1` or later, for API level 27) by digest, the ELF
  sha256 per model and runner architecture, and the seed, Zondax's public test mnemonic
  (`DOTAPP/tests_zemu/tests/common.ts:4`). `cargo xtask ledger-emu build-app` clones the pinned tag, checks the commit,
  builds the ELF in the builder image and checks its sha256. Nothing is vendored: release assets are installer scripts,
  not ELFs, and a source build is deterministic (`RES/J-ledger-ci-shim.md` §1.2, §5). Each test starts its own Speculos
  container with the plain `docker` CLI, like `Localnet` (§11.4): headless, ports published on 127.0.0.1 and read back
  with `docker port`, because the APDU server serves one client at a time. An approver task drives Speculos's REST API
  while the device call blocks in `spawn_blocking`: on `nanosp` it presses right until `APPROVE` (or `REJECT`), then both
  buttons; on `flex` it swipes until `Hold to sign` and holds. The screen strings are pinned per model, and on failure
  the harness keeps Speculos's log, `/events` and `/screenshot`.
- **Transport.** From the lane's first run, `LedgerSigner` runs on the core's own `LedgerDevice`, built by
  `LedgerDevice::with_transport` over `rtao-testkit`'s Speculos TCP transport (request `u32 BE length ‖ APDU`, reply
  `u32 BE length ‖ data ‖ SW`, the length excluding the status word), so the core's `ledger.rs` (APDU building, 250-byte
  chunking, status-word mapping) meets the app. `with_transport` and `ApduTransport` come from the core patch (§1.11),
  and only `rtao-testkit::ledger_emu` may name them, so no shipped crate depends on them. A prototype of this transport,
  about 35 lines, drove the core's `LedgerDevice` against the app (`RES/J-ledger-ci-shim.md` §8.7). The transport only
  frames the core's bytes; what the lane does not exercise (the HID transport and OS device access) is what the device
  run covers.
- **Live tests** (the `ledger_emu` target):
  1. `app_version` equals the pinned app version;
  2. `address(0, 0, 42, false)` equals the pinned public key and address of the test mnemonic (`e1b4d72d…8419`,
     `5HAeMNeM…TrKbn`), and the address equals `rtao_core::keys::ss58_from_public(pk, 42)`. A Ledger key comes from
     BIP32-Ed25519 (`DOTAPP/app/src/crypto.c:129-132`), which `Keypair::from_mnemonic` cannot derive, so the key is
     pinned, not derived. The displayed address is approved over REST;
  3. the `ledger_proof_vector.json` request signed through `LedgerSigner`: reply byte 0 is `0x00`, the signature
     verifies with the core's `keys::verify`, and it equals the golden `00bc772a…d80e` (ed25519 is deterministic);
  4. requests the core builds for golden calls (`transfer_keep_alive`, `add_stake`, `set_weights`, `burned_register`)
     verify and equal their goldens;
  5. a `Utility.batch` whose payload exceeds 256 bytes verifies over the core's `signature_payload`, its blake2-256
     hash;
  6. a rejection on the device maps to the core's rejected message (`0x6986`);
  7. a tampered proof and a tampered metadata hash give `0x6984` with the app's reason, before any review screen;
  8. `CheckMetadataHash` mode Disabled gives `0x6984` (`DOTAPP/app/src/parser_impl.c:91-110`), and payload ‖ proof
     above 16 KiB gives `0x6983`, surfaced verbatim;
  9. on `flex`, tests 3 and 6 with touch navigation;
  10. on the pinned localnet image, which upstream builds with `metadata-hash` (`UP/scripts/localnet.sh:40, 47`):
      after the bootstrap (§11.4), Alice funds the emulator's address, and a transfer signed through `LedgerSigner` and
      submitted by `Executor` is included with `ExtrinsicSuccess`;
  11. on a 12 s localnet, a shielded transfer completes with both approvals inside the 8-block shield era (eight fast
      blocks are shorter than one emulated approval flow, `RES/J-ledger-ci-shim.md` §1.2).

  The device side of tests 1-7 and of test 9's signing was exercised while planning, through a probe that framed APDUs
  as the core's `ledger.rs` does (`RES/J-ledger-ci-shim.md` §1); tests 8, 10 and 11 and the rejection on `flex` were not
  run. The device side of tests 1-3 and 6 also ran through the core's own `ledger.rs` on a prototype of the core patch,
  and a replay transport reproduced their 35 recorded exchanges (`RES/J-ledger-ci-shim.md` §8.7).
- **Recordings and replay.** `cargo xtask ledger-emu record` runs tests 3-7 and 9 and writes their APDU exchanges to
  `vendor/recorded/ledger/<app version>/<model>/<case>.apdu` in Ledger's `=>`/`<=` transcript format, with the pins in an
  `index.toml`. `ledger_replay`, an ordinary Rust test that `pr-gates` runs, replays them through a replay transport
  handed to `LedgerDevice::with_transport`: rtao's request bytes must equal the recorded ones, and the recorded
  replies must verify, equal the goldens and map to the same errors. A change to what rtao or the core sends to a Ledger
  therefore fails `pr-gates` until a re-recording proves that the app accepts it. The cases use the golden metadata
  (spec 461), so recordings change only with the core, rtao's request construction or the app pin.
- **Python request parity.** `ledger_request_parity` compares rtao's `(payload, proof, account, index)` with the
  oracle's dump of Python's `LedgerSigner` for the same requests (§11.5, §11.6).
- **When it runs.** Optional targeted hosted dispatch only (Q30), with explicit timeouts. Existing transcripts can
  be replayed as ordinary Rust tests. No local Docker host, nightly campaign, automatic recording or bot setup is
  required. Report stale or missing transcripts without initiating local emulator builds.
- **Finney.** `live-probe` (§12.6) checks that finney's runtime carries the compiled metadata hash, so a repeat of
  [subtensor#3159](https://github.com/RaoFoundation/subtensor/issues/3159), where runtimes 456 and 457 shipped without it
  and every Ledger transaction failed with `CannotLookup`, raises an alert without a Ledger.
- **Deferred hardware coverage.** HID transport and OS access, installed app/firmware and real-screen review
  remain unverified by emulators. Q30 defers device rehearsals; do not describe injected tests as hardware evidence.

---

## 12. Upstream tracking

### 12.1 What actually changes, and how often

From six consecutive finalized release pairs (`gh api …/compare`, [J-MA §1]):

| Pair | files | `sdk/bittensor-core/**` | `sdk/python/**` | `intents/*.py` | catalogs |
|---|---:|---:|---:|---|---|
| v466→v467 | 32 | 0 | 7 | `_root_claim_fee.py` (142 lines; catalog unchanged) | `reads.json` |
| v467→v468 | 146 | 0 | 9 | 2 files | all three |
| v468→v469 | 97 | 0 | 6 | `staking.py` | intents |
| v469→v473 | 282 | 1 (tests) | 11 | 2 files | all three |
| v473→v475 | 263 | 5 | 36 (incl. `cli/` ~716 lines) | 5 files | all three |
| v475→v477 | 93 | 0 | 3 | none | intents, reads |

Cadence was 9 finalized runtimes in 20 days ([R] H §0.3). Spec 478 changes no client-visible metadata, only the
`System.Version` constant (CORRECTIONS #2). The consequences:

1. **Python is the high-churn upstream** and often changes behavior without touching the catalogs. Tracking is
   therefore source-level (blob SHAs + symbol diff, §4.9) plus oracle-level (§11.5), never catalog-only. That was B's
   and C's gap [J-MA concern 2].
2. **The core rarely changes**, so `core_tag` moves only on a core-input fingerprint change (§1.2), and downstream patch
   lines change only then [J-MA concern 1].
3. **"Spec bumped, surface unchanged" is the common runtime case.** Headers omit `spec_version`, so it regenerates zero
   lines; a reviewed metadata/index update records the change. Automatic merging is deferred.

### 12.2 Jobs

Q30 replaces the broad scheduled infrastructure campaign with client-focused hosted validation. See
[PLAN.md §9.7](../../PLAN.md#97-ci-lanes-and-what-each-catches) for active lanes. `pr-gates` covers source/pin integrity,
units, goldens, committed oracle vectors, doctests, relevant features, MSRV, lint and supply checks on free
`ubuntu-latest`. Use timeouts and cancel superseded pushes. Do not repeat the hosted matrix locally.

Optional targeted hosted jobs may answer a specific oracle, interop, emulator, localnet, fuzz or performance question.
Missing results are recorded as coverage gaps. There is no mainnet-clone job, scheduled multi-network campaign,
mandatory device run or local macOS build requirement. Release and automatic-upkeep setup are deferred;
`RTAO_UPKEEP` remains off. Never add or toggle the `CI` label to trigger a run.

### 12.3 What a pin bump does (`xtask pin --parity vN`)

Q30 retains this pipeline as a technical reference. Costly oracle/recording/emulator/container stages, if needed,
run only in optional targeted free hosted jobs. No milestone forces a local pipeline run; automatic upkeep is
deferred and blocked refreshes are recorded.

Review a pin update when implemented client work needs it, then inspect applicable hosted source/pin checks for
that commit. Record blocked refreshes (`oracle-stale`, `recordings-stale`, `ledger-stale` or `core-patch-conflict`)
without forcing a pipeline replay. Bot PRs, milestone-driven bumps and full local gates are not requirements.

1. **Finality check** (§1.2 step 1), and the `:vN` localnet image is published. `xtask pin --parity vN --check`
   requests an anonymous pull token
   (`GET https://ghcr.io/token?scope=repository:raofoundation/subtensor-localnet:pull`), then
   `GET https://ghcr.io/v2/raofoundation/subtensor-localnet/manifests/vN` with that bearer token and
   `Accept: application/vnd.oci.image.index.v1+json` (an unauthenticated GET returns 401). On 200 it writes the
   `Docker-Content-Digest` to `[localnet].pinned`; otherwise record `bump=false` and the unavailable image; no polling campaign is required. Upstream
   builds the image after the mainnet mirror push, which the release job does right before promoting the release
   (`UP/.github/workflows/watch-mainnet-release.yml:9-14, 266, 275`; `docker-localnet.yml:15, 124, 193`): for v477 the
   release was promoted at 14:33Z and `:v477` finished building at 15:05Z.
2. **Re-vendor `[artifacts]`** from `vN`. The catalogs come from the monorepo `website/.../catalog`, not from the live
   site.
3. **`xtask extract`** (Rust parser) refreshes `vendor/upstream/extracted/*.json`. **`xtask regen`** refreshes catalogs,
   interfaces, dispatch, descriptors from `vendor/metadata/finney/<N>.v15.scale`, docs and the reference pages. `xtask
   pin` fetches that metadata itself at finney's finalized block (`Metadata_metadata_at_version(15)` and `(16)`) and
   commits it in the bump with the `vendor/metadata/finney/index.toml` row. Repeated source updates must not duplicate an existing `(spec, :code)` row.
4. **Optional hosted refresh.** If new behavior needs fresh vectors, a targeted bounded hosted job can run
   `xtask oracle` or upstream recorders. Preserve pins and review the resulting JSON/source diff. Speculos
   re-recording is likewise optional hosted work for a changed Ledger request. Missing evidence is recorded as
   stale; it does not start local images, nodes or emulator builds.
5. **Core-input fingerprint.** If it changed, move `core_tag` and run
   `cargo update -p bittensor-core -p w3f-bls -p subtensor-macros -p subtensor-linting` and `xtask core-surface`.
   While `[core.patch]` exists, `xtask core-patch rebase vN` first moves the core patch to the new tag (a conflict is
   `core-patch-conflict`), or, when vN contains PR-U17, the bump deletes both core-patch tables and `[core.patch]` and
   adapts `rtao-testkit` to the API as merged (§1.11).
6. **`xtask parity report`** writes the porting report: changed blobs per `map.toml` row with diffs, the symbol diff
   joined to Rust items, new ops and reads (which go to `pending.toml`), oracle-vector diffs, and the auto-bless
   candidates.
7. **Review and commit** the source update with its report, then inspect applicable hosted checks. Bot PR setup is deferred. Tests that fail because Python behavior changed point at the exact op via the oracle diff.

`xtask pin --parity <parity_tag> --force` re-runs steps 2-6 at the tag already pinned. It must leave the tree unchanged
(`git diff --exit-code`) with an empty porting report; M2 exits on that check, which proves the pipeline reproducible
before the first real bump depends on it.

If the oracle image cannot be built (a flaky maturin, OpenSSL or Perl toolchain), the bump is `oracle-stale`: held
during the goal loop, and after it a PR labelled `oracle-stale` that merges only with human sign-off. The committed
vectors from the previous tag stay authoritative until the image builds [J-PD concern 13].

### 12.4 Dependency hygiene

- **`deny.toml`** `[bans]` with `multiple-versions = "deny"` for the shared SCALE stack and transport:
  `parity-scale-codec`, `scale-info`, `scale-decode`, `scale-encode`, `scale-type-resolver`, `frame-metadata`,
  `sp-core`, `tokio`, `tungstenite`. A core bump that splits the stack (making `DecodeAsType` over `Runtime.types` stop
  type-checking) then fails in `pr-gates` [J-MA G10]. `jsonrpsee` and `subxt` are banned from the default-feature graph
  (checked by `xtask deps-check`, because cargo-deny bans are graph-wide). `aws-lc-rs`, `aws-lc-sys` and `native-tls`
  are banned outright: one rustls provider (§2.7).
- **Licenses**: an allowlist (MIT, Apache-2.0, BSD-*, ISC, Unicode-3.0, Zlib, MPL-2.0 for the transitive TLS stack).
- **Advisories**: `cargo xtask supply` retains unfiltered cargo-audit 0.22.2 terminal/JSON output and nonzero
  raw statuses for both workspace and fuzz locks. Q27 permits only RUSTSEC-2026-0253 on dormant lru 0.16.4,
  crates.io checksum `7f66e8d5d03f609abc3a39e6f08e4164ebf1447a732906d39eb9b99b7919ef39`; Q28 permits only
  RUSTSEC-2026-0344 on dormant ruzstd 0.8.3, crates.io checksum
  `a7c1c839d570d835527c9a5e4db7cb2198683a988cb9d7293fc8674e6bd58fc8`. Patched ranges must remain >=0.18.2
  and >=0.9.1 respectively, with no unaffected ranges. All-feature/all-target workspace, fuzz and shipped
  normal/build/dev profiles must contain neither affected package. Activation, unexpected identities/advisories,
  changed ranges, filtering and tool/schema/status failures block. The exact existing maintenance notices stay
  visible and informational; cargo-deny's advisory/license/source/ban checks remain blocking and unchanged.
- **Dependency update automation is deferred.** Review relevant third-party updates as client work requires.
  `bittensor-core` and `w3f-bls` are excluded, because only `xtask pin` (and, for the core patch, `xtask core-patch`)
  moves them.
- **Ledger emulator.** The lane adds no crate: it uses the plain `docker` CLI and the workspace's tokio and reqwest
  (built as §2.7 requires). Speculos (LGPL-3.0) is pulled by digest and the app (Apache-2.0) is built from a pinned tag
  in CI; neither is vendored or linked, so `deny.toml` and `NOTICE` do not change. Their pins live in
  `ci/ledger/pins.toml` and move only in a change that re-records the transcripts (Renovate excludes the file) (§11.9).

### 12.5 Drift classification

`xtask drift diff old new` does the semantic diff that `subxt diff` cannot, since `subxt diff` reports no events,
errors or indices ([R] H §2.3; its output is still attached verbatim). It compares:

- pallets (index, name);
- calls (index, name, argument names and exact shape hashes, V16 deprecation);
- events (index, name, field shapes);
- errors (index, name);
- storage (hashers, key and value shapes, modifier, default bytes);
- constants (type, value);
- runtime APIs (name, version, method shapes);
- transaction extensions (identifiers, `ty`/`implicit` shapes);
- extrinsic versions;
- `System.Version`.

| Class | Examples | Effect |
|---|---|---|
| `surface-unchanged` | only `System.Version` (478) | reviewed `index.toml`/`SPEC_RANGE` update; automatic merging deferred |
| `additive` | new calls, events, errors, storage, APIs; a new call in any pallet stays additive, because wrapper-call hashes treat `RuntimeCall` as opaque (§3.7; a drift unit test adds one synthetic SubtensorModule call to the committed 477 V15 metadata and expects `additive`) | regenerated descriptors; the coverage gate forces wrap-or-raw-only decisions |
| `behavioural` | moved event index; storage type or default change; runtime-API version bump; changed projection hash on a typed model | review models; typed paths fall back automatically on live networks meanwhile |
| `breaking` | moved pallet, call or error index; changed args of a wrapped call; removed wrapped call or used storage; extension with payload; `transaction_version` change; V15 no longer offered; `spec_name ≠ "node-subtensor"` | the extension guard and `verify_surface` refuse signing per call on live networks until the regenerated descriptors ship; major/minor bump per §12.8 |

**Extrinsic v5 alert** [J-CF concern 13]. finney advertises versions `[4, 5]`. The core signs only v4
(`CORE/codec/extrinsic.rs:265-273` [V]), and Python uses the core. The drift job raises a `breaking` alert if any of
these happens:

- V15 `extrinsic.version` stops being 4;
- the V16 version list drops 4;
- the core's source gains v5 assembly. `xtask parity report` flags changes to `sdk/bittensor-core/src/codec/extrinsic.rs`
  explicitly.

rtao signs v4 until upstream moves.

### 12.6 Live probes

These are optional bounded read-only hosted probes for a concrete client concern; there is no periodic monitoring requirement:

- finality lag under 10 minutes, the threshold upstream's smoke tests assert
  (`UP/ts-tests/suites/smoke/test-block-finalization.ts:16-29`);
- `spec_version` and the `:code` hash;
- V15 still served, and the needle not firing;
- the recomputed V15 RFC-0078 digest against the pin;
- `get_metagraph` decodes of netuid 1 and the largest subnet through all three paths (view, typed, dynamic) with equal
  `to_json()`;
- the events of the last 10 blocks decode;
- the public endpoints' policy behavior, observed passively;
- the compiled metadata hash: `TaggedTransactionQueue_validate_transaction` of a `CheckMetadataHash`-Enabled transfer,
  signed by an ephemeral ed25519 key over rtao's digest (§3.9.2), must return neither
  `UnknownTransaction::CannotLookup`, which a runtime built without the hash returns for mode Enabled (polkadot-sdk
  `cacb431`, `substrate/frame/metadata-hash-extension/src/lib.rs:150-155`; finney runtimes 456 and 457,
  [subtensor#3159](https://github.com/RaoFoundation/subtensor/issues/3159)), nor `InvalidTransaction::BadProof`, which
  means a different hash. Any other answer passes, since the account is unfunded (§11.9).

### 12.7 Compatibility matrix and runtime guard

`xtask compat` can generate `compat.json` and `COMPAT.md` from recorded source/test evidence. Release production is deferred:

| rtao | core tag (commit) | parity tag (source version; PyPI release if one was published from that commit) | validated spec range | metadata pins | localnet digests | btauth | interop |
|---|---|---|---|---|---|---|---|
| 0.5.0 | v477 (`5c6e83e2`) | v477 (11.3.1.dev0) | 477-478 | finney 477, testnet 477, devnet 478 | `v477@sha256:…`, `testnet@…`, `devnet@…` | btauth/1 | `bittensor` @ v477 |

"Validated" means two things: the drift diffs across the range contain no `breaking` class, and e2e and interop passed
on the listed digests ([R] H §3.9).

At run time `Client` compares the connected `(spec, metadata hash)` with the compiled-in `compat.json`. Outside the
range it only warns. Per-call `verify_surface` decides what still works, because most bumps are behavior-only.

### 12.8 Release process

Deferred by Q30.

Current work delivers reviewed client source, docs and applicable hosted results. It does not cut milestone tags,
publish binaries, install upkeep bots or run local release/hardware rehearsals.

A future distribution task can decide artifact targets, provenance and publishing. Linux x64 hosted tests do not
establish native ARM, macOS, Keychain or Ledger HID coverage. Record those gaps, retain relevant APIs and injected
tests, and do not create local Mac Studio or laptop build requirements to fill them. Documentation builds can be
downloaded as CI artifacts; no Pages environment is required. Keep `rtao-cli`'s binary `doc = false` to avoid its
`rtao` rustdoc path colliding with the facade library.

### 12.9 Maintenance playbook

Use the following signals to review relevant upstream source changes. Expensive refresh stages, if needed, are
optional targeted hosted jobs. Automatic monitoring/PRs/upkeep and deployment remain deferred.

| Upstream event | Source/test signal | Client work |
|---|---|---|
| runtime bump, surface unchanged | reviewed metadata/source diff; no generated interface change | none, or review the porting report for Python changes |
| additive surface | regenerated descriptors and coverage report list new calls | per new call: wrap it as an intent, type a read, or list it in `raw_only_extra.toml` |
| behavioural surface | metadata/model diff; live typed paths fall back | review and fix the flagged models |
| breaking surface | reviewed metadata change; runtime guards refuse affected calls | fix the affected calls and descriptors; usually upstream changed the core too, so the fix arrives as a core bump |
| Python adds an intent or read | source diff, pending/interface changes and any available oracle vector diff | port each new intent or read until its `pending.toml` entry can go |
| Python changes behavior only | blob change in `map.toml`, symbol diff and oracle diff name the ops | port the diff; cosmetic edits auto-bless |
| core API change | optional hosted core-source compilation can detect it; `core-surface` names the symbol | adapt `rtao-core`; no other crate changes |
| a staged PR ships in `core_tag` | `staged --check` fails because the module's probe (a symbol at the ungated target, or the vector suite) now passes (§1.6) | flip the `pub use`, delete the staged module |
| a chain rule mirrored by the framework changes (serving, rate limits, freeze window, tempo control, weights checks) | blob change on a chain-rule row of `map.toml`, or a diff of the extracted `chain_rules.json` (§4.9) | update `AxonPublisher`, `HyperparamPlanner` or `WeightScheduler` and their tests |
| btauth/1 → btauth/2 | blob change on `http_auth.py` or `signed-requests.mdx`; vectors fail | verify both versions, sign the configured one, keep v1 through the deprecation window |
| third-party breaking release (tungstenite, axum, reqwest) | reviewed dependency diff or hosted failure | adapt the one affected crate |
| a pin bump moves `core_tag` while the core patch exists | `xtask core-patch rebase` cherry-picks PR-U17 onto the new tag (§1.11) | on `core-patch-conflict`, resolve the conflict like any upstream change and log it |
| PR-U17 ships in `core_tag` | `xtask pin --check` fails with "PR-U17 shipped: drop the core patch" (§1.11) | delete both core-patch tables and `[core.patch]`; adapt `rtao-testkit::ledger_emu` to the API as merged |
| Ledger Live ships a new Polkadot app, or Speculos a new release | review app/core source changes; optional bounded hosted emulator comparison | bump `ci/ledger/pins.toml` in a change that re-records the transcripts |

Track changes relevant to implemented client behavior from M2 onward. Record stale or missing refresh evidence; no milestone-driven pipeline run or bot handover is required.

---

## 13. Documentation

- **rustdoc.**
  - `#![deny(missing_docs)]` on every library crate, and `#![doc = include_str!("../README.md")]` at crate roots.
  - Feature-gated items carry `doc(cfg)`. Hosted `cargo doc` output can be retained with the book in downloadable
    GitHub release assets on every `rtao-v*` tag (§12.8, Q23).
  - Every mirrored public item carries `#[doc(alias = "bittensor.<dotted.path>")]`, so searching rustdoc for
    `SetWeights` or `bittensor.reads.neurons.neurons` lands on the Rust item [J-PD graft 16].
- **docify.** Every example is a real `#[docify::export]` item in `examples/` or `tests/`.
  - `docify::embed_run!` is used when the example runs offline: units, `pyfmt`, btauth vectors, `FakeChain` reads,
    intent plans.
  - `docify::embed!` is used when it needs a node. Those examples are `#[docify::export]` items in
    `crates/rtao-testkit/tests/` (the `e2e` target), so the e2e lane, which runs only `-p rtao-testkit`, executes
    them.
  - READMEs are compiled from `README.docify.md` with `docify::compile_markdown!`. Examples therefore appear on the doc
    site, in the book and in the README, and none of them can rot ([R] F §7).
- **mdBook (`book/`)** mirrors the bittensor.com taxonomy ([R] I §5.6), so an agent that knows upstream's `llms.txt`
  can navigate rtao's by analogy:
  - `index`, `quickstart`: the template, the patch line, connect, a typed read, plan and execute on localnet;
  - `sdk`: `Client`/`View`/`Snapshot`, `Wallet`/`Signer`/`KeySource`, `Balance<U>`, intents, `ExtrinsicResult`/`ErrorCode`;
  - `agents`: upstream's eight-step flow with `rtao tools`, `execute_tool`, `Policy`, `--dry-run` and error codes
    (`UP/docs/agents.mdx`);
  - `cli`;
  - `guides`: validator, miner with btauth, owner tooling, commit-reveal and mechanisms, Null consensus, MEV shield,
    timelock, wallets and signers, Ledger, local development, deployment, observability, upgrades and compatibility,
    testing your subnet;
  - `concepts`: **links** to upstream raw markdown (`https://www.bittensor.com/llms.mdx/docs/concepts/<slug>/content.md`)
    rather than copies;
  - `migration`, three tables:
    1. **v11 Python → rtao**, generated from `parity/map.toml` plus the alias index, so it is always complete [J-PD
       graft 16];
    2. v10 Python → rtao (Axon/Dendrite/Synapse → `MinerRuntime`/`SignedClient`/btauth);
    3. bittensor-core's blocking client → rtao.
    Plus the divergence register (Appendix C), rendered.
  - `reference` (generated, below), `parity`, `compat`.
- **Generated reference** (`xtask regen` writes `book/src/reference/`):
  - `tx/<op>.md`: summary, signer, origin, wraps, a parameter table from the schema, the Rust struct and `Executor`
    usage, the `rtao tx` CLI line, the upstream page link and the chain `sources`. Pages cite a source as path plus
    `container::name` and link `raw_url` without a line anchor, so line shifts in runtime or pallet files change no
    generated page (the catalog's `line`/`end_line`/`url` move on every such edit, §4.9);
  - `query/<read>.md`, `errors/<code>.md`, `errors/chain/<Name>.md`, `hyperparameters/<name>.md`.
  These follow upstream's page templates ([R] I §5.2), with `## Rust` replacing `## Python`, and `regen --check` keeps
  them current.
- **Catalogs.** `catalog/{intents,reads,errors}.json` use upstream's schema verbatim plus a `rust` object per entry, for
  example
  `{"type": "rtao::intents::SetWeights", "role": "Hotkey", "output": "WeightsSubmitted", "cli": "rtao tx set-weights …"}`
  or `{"method": "client.epochs().epoch_status(netuid)", "returns": "EpochStatus"}`. `hits` are exact (§4.8). Tools that
  consume upstream catalogs read rtao's unchanged.
- **`llms.txt` and `llms-full.txt`** follow upstream's conventions ([R] I §5.1):
  - a search preamble, catalog URLs, and a cheat sheet for `rtao tools`, `rtao query` and `rtao explain`;
  - the page tree;
  - a **mapping block** pairing each `tx/<op>` and `query/<read>` with its upstream raw page.
  A short `SKILL.md`-style card for coding agents is generated from the CLI tree.
- **Upstream log.** Every pin bump appends its porting-report summary and oracle diff to
  `book/src/upstream-log.md`. That gives users a readable record of which upstream changes reached rtao and when.

---

## 14. Milestones

Milestones organize client source dependencies and applicable hosted checks. They do not require releases,
deployments or local build campaigns. API details remain in the preceding sections and per-item assignments in
parity-inventory.md; PLAN.md §12 defines active steps and C1-C7. Security reviews inspect source and hosted evidence,
with findings recorded and resolved or explicitly accepted. Device/native-platform gaps stay visible.

| Milestone | Client source | Required development evidence |
|---|---|---|
| M0 | workspace/pins, core wrappers and staging guards, primitives, btauth, fixture loaders, doctor | hosted units/goldens/supply/no-host/source guards; reviewed licenses and attribution |
| M1 | async RPC/TLS, runtime/cache/views, generated descriptors, metagraph/commitment reads, config | hosted FakeNode/FakeChain, transport/model/metadata fixtures and selected MSRV/features |
| M2 | signing/wallets/nonces/submission/receipts, intent/read dispatch, weights/scheduler, validator APIs/example | hosted nonce/receipt/scheduling/restart/conform tests, no-host/source guards and source security review |
| M3 | btauth server/admission/limits, miner APIs/example, serving and metagraph persistence, template source | hosted verifier/publisher/runtime/fixture tests and template profile compilation |
| M4 | owner/planner/shield/multisig/raw APIs, registration/hyperparameter reads/intents, CLI machinery | hosted owner/shield/CLI/process tests, injected Keychain tests and source security review |
| M5 | remaining intent source, offline payload import/signing, proxy/multisig/books | hosted table-driven plan/encoding/policy and malformed/stale payload tests; accurate source parity |
| M6 | remaining read/model/namespace source and blocking facade | hosted exact-hits/single-block/model tests and existing vector replay |
| M7 | Ledger/Vault/extension source, EVM/timelock/GPU feature source, signer CLI | hosted injected/replay/proof/framing/bridge/keystore tests and source security review; devices/native platform deferred |
| M8 | remaining CLI commands/aliases/pickers/completions | hosted tree/argument/JSON fixtures, strict source/catalog/parity checks; unexecuted interop coverage recorded |
| M9 | client hardening, docs and filing inventory | applicable hosted client checks, security review and honest coverage report; release/upkeep deferred |
| M10 optional | compatibility crates and RemoteSigner, only if requested | scoped client/protocol tests; optional hosted interop as needed |

Oracle refresh, localnet/interop/image and emulator checks are optional bounded hosted evidence for a concrete
client change. Mainnet cloning and full-node operation are removed. Automatic multi-network schedules, release
artifacts, native-platform build rehearsals and device runs are not milestone requirements. Missing evidence is
recorded without claiming that these checks passed.

Dependency order remains M0 -> M1 -> M2 -> M4 -> M5 -> M8 -> M9. M3, M6 and M7 follow their API dependencies;
logical independence does not authorize concurrent local builds or duplicate targets. Track relevant source/pin
changes as development needs them; no forced bump, full-pipeline replay or bot handover is required.


**Why not B's or A's milestone plan.** B under-scopes the 23,083-line CLI and leaves its type generator out of the plan
[J-MA concern 9]. A's plan is realistic in scope but delivers transactions last, because its M1 is all 102 reads
[J-PD §4 A].

---

## 15. Risks and open questions

### 15.1 Risks

| # | Risk | Likelihood / impact [I] | Mitigation |
|---|---|---|---|
| R1 | `bittensor-core` is internal and unversioned (`publish = false`, `0.1.0`), and its API follows the Python binding | high / medium | one seam crate; `core_surface` probe names the broken symbol; optional hosted `core-canary`; `core_tag` moves rarely (§1.2); upstream PRs make the seam designed |
| R2 | release cadence (9 finalized runtimes in 20 days) | certain / low after automation | the drift and pin-bump pipelines, run by the agent during the goal loop and by bots after it; `surface-unchanged` regenerates zero lines; auto-bless for cosmetic Python changes |
| R3 | downstream `w3f-bls` patch friction | certain until PR-U4 / low | template, `rtao doctor`, patch changes only on core bumps (§1.2), PR-U4 with localnet evidence |
| R4 | C toolchain for owner and CLI builds (libsodium, vendored OpenSSL needing Perl, secp256k1-sys) | certain for those profiles / low | the neuron profiles are no-host (§1.7); a builder image for owner/CLI; PR-U6 splits `host` |
| R5 | the Python oracle infrastructure (maturin, uv, OpenSSL, Perl) is slow or flaky | medium / medium | CI-only, image cached per tag, an `oracle-stale` bump held with committed vectors authoritative (no PyPI fallback: no PyPI version reliably identifies a tag, §11.5); never gates a build |
| R6 | extraction breaks when upstream restructures a table | medium / low | the extractor fails loudly with file and line; tables are few and literal; PR-U3 and PR-U11 move them into catalogs |
| R7 | the parity long tail (23k-line CLI, intricate preflights such as `_root_claim_fee.py`) holds more porting work than its line counts show | medium / medium | generated `tx`/`query`; JSON parity before human formatting; `pending.toml` makes the gap a number; the CLI machinery and 118 of the 163 commands land in M4-M7, so M8 closes 45 commands and the strict gate |
| R8 | owning the WebSocket session (about 1.2k lines of reconnect, resend and subscription logic) | medium / high | port of `rpc.py` **with** `test_rpc.py`; `FakeNode` fault injection at the socket; one supervisor task; the `ReadMethod` allowlist makes resending a submit unrepresentable |
| R9 | reveal-round prediction: owner `trigger_epoch` after a commit, deferral, clock skew | medium / high for validators | pallet-faithful predicate, `ClockGuard`, reveal watcher with event attribution, 12 s localnet lane, PR-U5; deferral analysis (§9.3 step 9) |
| R10 | a typed model silently mis-reads a semantic change behind an unchanged field name | low / medium | projection hashes check types, not only names; `freeze_struct` tripwires; oracle read vectors; typed == dynamic proptests |
| R11 | a bug in the shape-hash function or the view parser mis-decodes | low / high | exact hashes for the positional parser; fuzzing; three-path differential tests; `RTAO_METAGRAPH_VIEW=0` escape hatch |
| R12 | public endpoint quotas and per-endpoint differences | high / medium | coalesced reads, token bucket, legacy namespace only, live probes, own-node guidance |
| R13 | btauth/1 adoption is still zero among surveyed subnets ([R] D §6) | high / medium | the `legacy-v10` bridge feature now; compat crates post-parity (M10) |
| R14 | licensing: Apache-2.0 ported and vendored material inside rtao | low / low | dual `MIT OR Apache-2.0` (decided, §15.2 Q1), `NOTICE`, per-file attribution on staged and ported code, `(MIT OR Apache-2.0) AND Apache-2.0` on crates with derived files (§2.5) |
| R15 | the preferred Ruff parser conflicts with the core Unicode dependency | low / low | xtask-only, pinned rev; fallback `rustpython-parser`; the JSON contract between extractor and generator does not change |
| R16 | upstream declines the staged PRs (its spec excludes a Rust transport and intent layer from the core) | medium / low | staged code stays isolated (about 2k lines); the transport and intent layers were never proposed for the core |
| R17 | fixtures/localnet differ from mainnet | certain / medium | pinned metadata and source review; optional targeted hosted checks; mainnet clone removed by Q30 |
| R18 | secrets: offloaded KDF threads, the keychain, nonce-store memory | low / high | `Zeroizing`/`secrecy`; no `Debug` on secrets; a bounded in-memory store; Redis for multi-process; per-milestone security passes by local cold readers (M2, M3, M4, M7; §15.2 Q19) and the full review in M9 |
| R19 | `serde_json/arbitrary_precision`, forced on by the core, breaks `#[serde(untagged)]`/`#[serde(flatten)]` parsing of non-integer numbers from text and makes `serde_json::Value` equality spelling-sensitive, in rtao and in subnet authors' code | certain / medium | rtao enables the feature itself; `Value`-first parsing at every text entry point and in rtao's number-or-string types; `rtao::btauth::server::Json<T>` for subnet authors; the oracle comparator by token kind (§2.7, §4.8, §7.2); an M0 test pins all of it |
| R20 | upstream churn moves the parity target during development | certain / medium | upstream tracking as standing work from M2 (§14); pin bumps from M2, by the agent during the goal loop and by the bot after it; the porting report sizes each bump; auto-bless covers cosmetic edits; the M8 strict gate is evaluated at the `parity_tag` current at M8 |
| R21 | the Ledger emulator lane is not the production path: Speculos never runs the core's HID transport or the binary Ledger Live installs; until PR-U17 ships, the rtao workspace, and with it the device-run build and the release binaries, uses the patched core, whose `open()` path differs from upstream only by the seam; and the lane's tooling moves (API levels, image tags, removed flags) | certain until PR-U17 / medium | PR-U17 filed in M0; `[core.patch] paths` bounds the difference, and `xtask pin --check` fails once the patch is obsolete (§1.11); `core-unpatched` proves that shipped crates build without it; hardware/HID coverage remains deferred and unverified under Q30; pins by digest and a source-built ELF checked by sha256; the REST API instead of the deprecated `--automation` |
| R22 | runtime/app changes can break clear signing, as seen in finney runtimes 456/457 (subtensor#3159) | low / high for Ledger users | source/pin guards and existing request/proof fixtures; optional bounded hosted probe/emulator evidence; hardware coverage remains unverified |

### 15.2 Decisions and open questions

**Decided by the user on 2026-10-13** (PLAN.md §14.1 quotes each answer and lists its consequences):

- **Q1. License.** Dual `MIT OR Apache-2.0`, as recommended (§2.5). M0 relicenses the skeleton first: `LICENSE-MIT`,
  `LICENSE-APACHE`, `NOTICE`, the attribution header on every file derived from upstream, and
  `(MIT OR Apache-2.0) AND Apache-2.0` for the crates that contain such files.
- **Q2. Python in CI.** Accepted as stated: the 100%-Rust requirement applies to what subnet owners build and run.
  Python appears only in CI lanes (oracle, interop, upstream's recorders, and the Ledger emulator lane, whose Speculos
  runs Python inside its image, §11.9), never in regeneration, PR gates, or any build
  of rtao or a subnet binary (§11.5).
- **Q3. Binary name.** `rtao` only, with no opt-in `btcli` shim: with shared config files (§10.2), a shim could
  surprise users who have both installed (§10, §17).
- **Q4. btauth `<Bytes>` wrapper.** Verification accepts plain and wrapped signatures by default
  (`WrappedPolicy::AcceptLikePython`, Python parity); `WrappedPolicy::Strict` stays available; rtao never produces the
  wrapper (§7.1).
- **Q5. Repository location.** `github.com/sam0x17/rtao`.
- **Q17. Ledger in CI.** The original device checklist and emulator research remain historical context. Q30
  defers device acceptance and permits optional targeted hosted emulator checks. Existing transcripts and injected
  signer tests run as ordinary Rust tests; no local hardware or Docker requirement remains.
- **Q18. Upstream engagement.** Each upstream PR and issue is filed as soon as its evidence exists, not held for M9
  (Appendix D names the step); PR-U5 and PR-U7 fix live defects for the core's other users. Filing in the monorepo is
  pre-authorized; the one item outside it, for the Ledger app, was filed with Zondax on 2026-10-13 at the user's request
  (Q22, Appendix D).
- **Q19. Security passes.** Local cold readers (§14): a fresh-context reviewer that took no part in the implementation
  receives only the repository, the milestone diff and a security checklist (PLAN.md §12.1), and writes
  `docs/security/m<k>.md`; every finding is fixed or explicitly accepted before the milestone exits.
- **Time estimates.** None: the plan states order, dependencies and objective exit checks (§14), and an AI coding agent
  executes it step by step (PLAN.md "Executing this plan").
- **Q20. PR-U17 as a core patch.** On the v2 summary's note that "a test-only copy of the core's Ledger message code
  sits in `rtao-testkit`", the user decided: "ok let's just add to the plan that we will cargo patch bittensor-core with
  that support until it merges". Until a `core_tag` contains PR-U17, the rtao workspace builds
  `bittensor-core` through a Cargo `[patch]` that adds exactly PR-U17 on top of `core_tag` (§1.11). Only
  `rtao-testkit::ledger_emu` names PR-U17's items; subnet builds never see the patch, and the `core-unpatched` job
  proves that the shipped crates build without it. The test-only transcription survives only as a fallback if upstream
  declines PR-U17, which is a stop-and-ask point.
- **Q21. One goal loop, no PRs.** Direct client-development commits to `main` remain authorized. Q30 replaces
  full local prepush gates with applicable hosted checks and defers milestone tags and automatic-upkeep setup.
- **Q22. The Ledger app fix.** Told that the core patch cannot change how the Ledger app displays amounts (stake
  amounts typed `TaoBalance` appear as raw integers, `RES/J-ledger-ci-shim.md` §4.6), the user answered "for zondax we
  should just submit a PR or issue", and later the same day "yeah go ahead and open it now, ideally with a PR fixing it
  if it is an easy fix". Filed with `Zondax/ledger-polkadot` on 2026-10-13, before the plan runs: issue
  [Zondax/ledger-polkadot#295](https://github.com/Zondax/ledger-polkadot/issues/295),
  PR [#296](https://github.com/Zondax/ledger-polkadot/pull/296), which adds `TaoBalance` to the app's balance type
  names (`AlphaBalance`, a per-subnet token, stays unformatted). The executing agent only follows it up (answering
  maintainer questions and pushing the changes they ask for to the PR branch); any other filing outside the monorepo
  stays a stop-and-ask point.

- **Q23. Prerequisites and hosted CI, 2026-10-14.** The user answered: "I can provide testnet tao to whatever wallet
  address you need; just use my github; we will skip sophisticated github runners, I want to just use ubuntu-latest
  free ones; I don't see why we need github pages, that seems overkill; I have a ledger we can use". Use `sam0x17`
  GitHub/GHCR and only free Linux x64 `ubuntu-latest` hosted jobs. Downloadable rustdoc/book release assets replace
  Pages. Q30 later removes local Mac builds, cross-platform/hardware rehearsals, mainnet clones and local
  infrastructure prerequisites. The design retains platform behavior; actual native platform coverage remains
  unverified and is reported explicitly (PLAN.md §12.13).

- **Q29. Testnet soak waived, 2026-10-14.** The user instructed: "we can probably skip a testnet soak, let's just make sure all tests are passing". M2-24 and its dedicated funding, subnet setup and host prerequisites are waived; no soak run is claimed. Q30 later makes costly container checks optional hosted evidence and defers release acceptance. Stop soak preparation and deployment, preserve the prepared wallet/recovery state, and clean completed scratch after dependency checks.

- **Q30. Client development and bounded hosted CI, 2026-10-14.** After hundreds of gigabytes of local disk use,
  the user removed costly local work. The current execution scope governs: no local node/container/testnet work,
  duplicate Cargo builds/toolchains, hardware/Keychain/release rehearsals or fuzz/bench campaigns. Mainnet cloning
  is removed; releases and automatic upkeep are deferred. Retain client behavior and honest coverage records.

**To resolve by measurement or with upstream:**

6. Real public-endpoint quotas, to tune the default token bucket (§3.2.5).
7. Will RaoFoundation accept PR-U1…U17, especially U4 (`w3f-bls` manifest), U5 (epoch predicate) and U6 (`host`
   split)? Each is filed as soon as its evidence exists (Q18).
8. Does a crates.io `w3f-bls 0.1.3` std build produce ciphertexts that pallet-drand decrypts (§11.4 test 16)? This is
   PR-U4's precondition. `ci/pr-u4-probe/` answers it; the four-file source diff (§1.3) predicts yes, because neither
   `tle` nor the core's timelock code touches the one behavioral change (`SecretKey::check`).
9. Should upstream account for `MaxEpochsPerBlock` deferral in `get_next_epoch_start_block` and the reveal predictor
   (§9.3 step 9)?
10. **Resolved from source.** Root `sudo_set_network_immunity_period(0)` (AdminUtils call 35), then
    `sudo_set_subnet_limit(<current non-root subnet count>)` (call 37), make the next `register_network` prune the
    lowest-priced subnet and queue (`UP/pallets/admin-utils/src/lib.rs:1215-1228, 1250-1258`;
    `UP/pallets/subtensor/src/subnets/subnet.rs:204-213, 233-260`; `coinbase/root.rs:358-361`); scenario 10 runs it in
    the `global` container (§11.4).
11. When will the runtime or the SDKs move to extrinsic v5 (§12.5)?
12. Will `SelectiveMetagraphIndex` be exposed through metadata or a runtime API, removing the pallet-source extraction?
13. **Resolved.** Upstream's `docker-localnet.yml` publishes `:v<spec>` on every mainnet mirror move
    (`UP/.github/workflows/docker-localnet.yml:7-9`; GHCR has v432, v473, v475, v477). `xtask pin --check` waits for
    the image (§12.3), and `wasm-override` is only a manual fallback (§11.4).
14. Is `Metagraph.total_stake` intentionally declared `TaoBalance` while carrying stake weight?
15. Does `subxt` 0.51 build with `default-features = false` over a custom `RpcClientT` without pulling jsonrpsee (M7)?
16. Is the `tokio-tungstenite` 0.30 handshake error API identical to 0.28's `Error::Http(Box<Response<…>>)`? This is
    verified by a `FakeNode` test in M1.

---

## 16. Decision log

| # | Decision | Alternatives considered | Rationale | Evidence |
|---|---|---|---|---|
| 1 | Base on D; graft C's operator runtime and seams, A's transport and fidelity rules, B's types and measurements | A (chain-fidelity winner), C (performance/DX winner), B | D has the highest score sum (119) and best mean rank (1.83); its anti-drift machinery is the hardest part to retrofit; the grafts fill exactly the gaps each judge named | §0.1; J-CF §1, J-MA §0, J-PD §0 |
| 2 | Seam = `rtao-core` (only importer) + `rtao-core-ext` (staged upstream code) | D: many crates import the core, plus a staging crate; A, B, C: one seam, no staging | a core API change is absorbed in one crate; an upstream merge is a one-line `pub use` flip; `disallowed-methods` lives in one place | J-CF graft 2; C §2.2; D §1.4 |
| 3 | Two pins: `core_tag` (moves on a core-input fingerprint change) and `parity_tag` (every finalized release) | one tag bumped every release (all four proposals) | the downstream `[patch]` must match `core_tag`; core sources changed in 1 of 6 release pairs (tests in 1 more), while Python changed in 6 of 6 | J-MA §1, §4.1, G6 |
| 4 | Keep `[patch.crates-io] w3f-bls` mandatory; `rtao doctor` + lock test; upstream PR-U4 with a localnet decrypt proof | A M0 / D Q1 "build without the patch" | without the patch Cargo locks two `w3f-bls 0.1.3` sources and the core fails with E0277 | `SP/design-bench-nopatch/Cargo.lock:2377-2404` [V]; `target-nopatch/…/output-lib-bittensor_core` E0277 [V]; `UP/sdk/bittensor-core/Cargo.toml:62` [V] |
| 5 | No-`host` neuron profile with a staged plaintext keyfile reader, differential-tested | `host` always on (all four); pure-Rust NaCl reimplementation ([R] F §0.8) | neurons need only a plaintext hotkey; the core's JSON codec uses only non-host constructors; 207 vs 330 crates; no Perl, OpenSSL, libsodium or CMake in neuron images (native code limited to `ring` and `blake3` SIMD) | `CORE/lib.rs:10-25` [V]; `keyfiles/mod.rs:179-370` [V]; J-PD concern 1 |
| 6 | Transport = own tokio-tungstenite session exposed via `subxt_rpcs::client::RpcClientT`, TLS from one explicitly built `ring` rustls config | jsonrpsee `WsClient` (C, D); subxt `ReconnectingRpcClient`; raw session without `RpcClientT` (A, B); the process-default provider | Python's refusal contract needs handshake headers and close 1008; `RpcClientT` keeps `LegacyRpcMethods`, mocks and an optional `OnlineClient`; with zero or two rustls providers `ClientConfig::builder()` panics, and the core's `host` (ring) plus reqwest 0.13's default (aws-lc-rs) make two | jsonrpsee `ws/mod.rs:207,522-524` [V]; tungstenite `error.rs:71`, `coding.rs:154,214` [V]; `rpc_client_t.rs:26-52` [V]; `REG/rustls-0.23.45/src/crypto/mod.rs:243-286`; §2.7 |
| 7 | Resend allowlist (`ReadMethod`) plus a per-frame `sent` flag; a queued frame whose caller has gone is never transmitted | Python's blocklist only | a resent submit is structurally impossible; parity for every method Python uses; untransmitted submits still go out unless their caller was cancelled, whose dropped `NonceLease` already cleared the nonce cache | `PY/_transport/rpc.py:66, 495-504, 549, 686-717`; B §3.2 |
| 8 | Classify policy refusals exactly as `rpc.py` does, from extracted constants | treat any `-32004`/`-32005` as terminal (C) | parity; an ordinary `-32005` must not become a terminal refusal | `rpc.py:52-59, 84-135, 190-241` [V] |
| 9 | Coalesced reads (`fetch`) + a client-side token bucket on public presets | per-item reads (A, B, D); a bucket without coalescing (C) | about 1 r/s public endpoints and terminal `-32004`; a weights preflight goes from about 18 RPCs to 1 | `UP/docs/guides/local-development.mdx:160` [V]; J-PD concern 6 |
| 10 | V15 always; V14 only on Python's missing-method needle; V14 is degraded (no signing; runtime APIs only through the ported legacy table) | fall back on any error (D, core); unrestricted fallback (A, B); V14 always an error (J-PD) | archive parity with Python and no silent V14 against a V15-capable chain | `runtime.py:34, 292-312` [V]; `runtime/mod.rs:172-181` [V]; `runtime_api.py:95-171` [V]; `meta_getMetadata.json` (V14) [J-CF §7 #10] |
| 11 | Name-based descriptors; storage keys always from the live entry; hashers recorded only for drift | B's baked hashers and positional codec types | archive reads across hasher changes stay correct; no second encoding engine | P-B line 746; J-CF concern 11; [R] A2 §8 |
| 12 | Exact shape hashes for calls and the positional view; projection hashes for by-name models; the outer `RuntimeCall`/`RuntimeEvent`/`RuntimeError` enums opaque; `verify_surface` fail-closed per call at connect and on every upgrade | B: exact for every type; C: ident checks; D: connect-time checks only; fully structural hashes through `RuntimeCall` | appended runtime fields stay harmless for by-name models; signing and positional parsing stay safe; a call added in any pallet does not make every older build refuse the 12 intents that wrap `batch_all`, `as_multi`, `proxy_announced` or `sudo` | J-MA G4, G9; J-PD concern 10; `scale-decode-derive lib.rs:222-236` [J]; `CORE/codec/encode.rs:528-532`; §3.7 |
| 13 | One encoder (`Runtime::compose_call`) | B's `[p, c] ++ args.encode()` | the golden vectors pin a single encoder | J-CF §3.4 |
| 14 | `DecodeAsType` for typed models, `MetagraphView` for the metagraph, core `Value` for JSON; no "typed is faster" claim; criterion gates | B: positional everywhere; A/C/D: `DecodeAsType` for the metagraph | measured: `DecodeAsType` 1,380 µs, `Value` 330 µs, view 6.5 µs at 256 UIDs; RPC 0.7-0.9 s dominates | `SP/design-bench/bench-run-3.txt` [V] |
| 15 | Generated intent interfaces, 46 generated pass-through bodies (the generator lands in M2), hand-ported behavior; `AnyIntent` + shrink-only `pending.toml`; `WRAPS` as typed descriptor references | hand-written structs (A); scaffolds only (B, C); generator in M5 with 10 bodies hand-written first | a new upstream op is a compile error; a removed wrapped call breaks the build; regen already emits interfaces, `AnyIntent` and `defaults.rs` in M2, and 10 of the 46 are M2/M4 ops | J-MA §3.4, G7; D §4.2; parity-inventory §4.1 |
| 16 | Compile-time roles via `KeySource<Role>`; `dyn` path keeps Python's runtime check | `dyn` only (A, C, D) | "neurons never load a coldkey" becomes a type error | J-CF graft 14; J-PD graft 2 |
| 17 | `ExtrinsicResult<O>`: Python's `data` map plus a typed `output` | Python map only; B's typed-only output | JSON parity and a typed API at once | `PY/result.py:697-749`; B §4.4 |
| 18 | serde + ported `validate()` instead of the `jsonschema` crate | D's `jsonschema` in the default graph | Python validates through dataclasses; less compile weight; the catalog schema is still emitted verbatim | J-PD §4 D |
| 19 | `commit_reveal_version` = 4 (supported client format); the framework refuses when storage differs; intents unchanged from Python | echo `CommitRevealWeightsVersion` (B, C, D) | the chain uses it to gate client upgrades; echoing could tag a v4 payload with a newer version and turn a loud error into a silent reveal loss | `weights.rs:318-337` [V]; `dispatches.rs:1946-1948` [V]; `PY/intents/weights.py:33, 411, 472-477` [V] |
| 20 | Pallet-faithful epoch predicate (staged), with core-agreement tests and PR-U5; the schedule is read at the best head, as Python does | inherit the core's predicate for Python parity (A); read at the finalized head | the core predicts one epoch too late after an `EpochSkipped` slot or a tempo lowered below the step counter (deferral does not diverge: both predicates honor `PendingEpochAt`), which can silently lose commits; the core predictor assumes inclusion at `current_block + 1` relative to the head the SDK queried | `epoch_schedule.rs:54` [V]; `run_coinbase.rs:1232, 413-415` [V]; `CORE/timelock/constants.rs:41-44`; `PY/intents/weights.py:283-306` |
| 21 | Do not model `MaxEpochsPerBlock` deferral; observe `EpochDeferred`/`Skipped`/`Triggered` | model deferral across all subnets | deferral moves reveals later (the safe direction, given per-block retries); its inputs are unpredictable; the pallet's own predictor ignores it | `run_coinbase.rs:356-431, 1256-1262` [V]; [R] C1 §2.3 |
| 22 | `SystemClock` default + `ClockGuard` (warn above 2 s, refuse above 12 s) + optional `ChainClock` | unguarded wall clock (Python, A); chain time only | parity by default, with a loud failure instead of a silent reveal loss on skewed hosts | `CORE/timelock/mod.rs:169-187`; C §1.6 item 3 |
| 23 | Nonces: Python semantics via an RAII `NonceLease`; one `NonceManager` per `Client`; `lease_pair` leaves `next = n + 2` (D-026); reconciliation layered on top | C's reservation until reconciliation; a manager per executor; Python's `pin(n+1)`, `pin(n)` | the pool-aware `account_nextIndex` makes clearing safe; reservation strands later nonces when a transaction was dropped; two managers hand out the same nonce; Python's cache is left at n after a shielded submission | `extrinsics.py:55-84` [V]; `_substrate.py:753-772` [V]; `PY/_transport/interface.py:134`; `PY/executor.py:1037-1052`; J-CF concern 8 |
| 24 | Python era defaults (128, finalized anchor) everywhere, weight commits included | C's era 32 + `InBlock` | parity; 128 blocks already bound reconciliation well inside a tempo | `PY/settings.py:22`; `interface.py:485-499` [J] |
| 25 | Python receipt semantics (6 wrappers, MevShield, fee fallback), staged and upstreamed (PR-U2); `retracted` and `finalityTimeout` resolved, not failures (D-024) | the core's 3-wrapper check; Python's five fatal watch statuses | parity; the core misses failures inside `SudoAsDone` and `Utility` batches; a retracted extrinsic may still be included, and a finality timeout means it is in a block | `receipt.py:8-16` [V]; `client.rs:1018-1048` [V]; polkadot-sdk `transaction-pool/api/src/lib.rs:95-125, 143-172` |
| 26 | Typed `PoolRejection`; shielded submissions never retried, as in Python | — (D-011, which assumed Python retries carriers, is withdrawn) | Python routes shielded intents around its retry loop; with `ShieldPeriod` a carrier `Stale` is `CheckNonce`'s outdated nonce, surfaced as `PoolRejection::Stale` | `PY/executor.py:812-845, 897-909`; `UP/runtime/src/check_nonce.rs:145-146`; `executor.py:52-56` [V] |
| 27 | `ShieldPeriod { P4, P8 }`; `ShieldCiphertext` ≤ 8,192 bytes; refuse when the finality lag would expire the era before inclusion (B + 2 ≥ F + p, D-025) | runtime checks; submit and fail (Python) | chain limits as types; carrier and inner apply together in B + 2, so `P4` fails at finney's usual lag of 2-3 blocks | `check_mortality.rs:19, 94-100` [V]; `PY/executor.py:61`; `UP/pallets/shield/src/lib.rs:324, 616-620`; `era.rs:86-99` |
| 28 | `CallBytes` stamped with spec and transaction version; `submit_signature` refuses on either change; `execute` re-plans once | Python's spec-only check | a dry run and the signed bytes cannot diverge across an upgrade | `interface.py:617-623`; [R] A1 §10 item 16 |
| 29 | Constants from the snapshot's runtime (D-007) | the head codec (Python, A) | correct across upgrades | `PY/snapshot.py:66-68`; J-CF graft 12 |
| 30 | Pin every multi-RPC read (D-002) | Python's four unpinned gather reads | results never straddle blocks | [R] A2 §1.3 |
| 31 | Zero-account defaults: a typed `DefaultAccount` accessor, applied by rule over the pinned metadata (every `Default` `AccountId32` entry whose default is 32 zero bytes); JSON unchanged | bake `None` (B, C); keep the zero account; a localnet gate first | the metadata settles A2's inference offline; a typed-only divergence keeps the oracle green | `meta/finney.json` (`Owner`, `SubnetOwner`, `SubnetOwnerHotkey`, `Keys`); [R] A2 §9 Q9; J-CF concern 9 |
| 32 | `round_ties_even` + Python operation order; oracle u16 vectors with `.5` ties | `f64::round` (C, D unspecified) | Python `round()` is half-to-even | `PY/intents/weights.py:54-114` [V] |
| 33 | Emulate Python's `str(Balance)` exactly; JSON value-equality is the contract; human tables not byte-identical | narrow the claim to JSON (J-CF option); exact decimals (D) | read JSON embeds `str(Balance)`; emulation is about 100 lines with an oracle | `PY/balance.py:113-149` [V]; `PY/cli/query.py:104-116` [V]; `PY/cli/output.py:562` [V] |
| 34 | btauth: accept `<Bytes>` by default (strict configurable; decided by the user, §15.2 Q4); retention checked at construction; core verify `Err` → `MalformedAuth`; async `NonceStore` | strict by default ([R] D); D's sync store | parity; construction-time config errors; Redis must not block a worker | `http_auth.py:324-341` [V]; `keys/mod.rs:457-469` [V]; J-CF concern 6; J-PD concern 7 |
| 35 | Python only in CI (accepted by the user, §15.2 Q2); Rust-native extraction and regeneration | D's Python extractor in regeneration | 100%-Rust builds; Python failures cannot block builds | J-MA §4.3; J-PD concern 13 |
| 36 | Parity via `map.toml` blob SHAs + Rust symbol diff + `doc(alias)` + bless and auto-bless; catalogs vendored from the monorepo | D's line-range provenance; B's live catalogs; C's short watch list | catches behavior-only Python changes; catalogs match the pinned tag | J-MA G1, §4.7, concern 2 |
| 37 | Two fakes: `FakeNode` (socket) and `FakeChain` (`RpcClientT`), plus `RecordingView` | one fake at `RpcClientT` (C, D) | the supervisor logic lives below `RpcClientT` | J-PD concern 5; `mock_rpc_client.rs:50-70` [J] |
| 38 | Use upstream's own recorders (Python, CI-only) for spec-current fixtures, through a wrapper that drops entries absent from the live metadata | A's Rust ports of the recorders; running them unchanged | identical formats, less code to maintain; `record_golden.py` hard-codes `SubtensorModule.Alpha`, which 477 no longer has | D §12.3; [R] H §5; `record_golden.py:41-49, 85, 169-181` |
| 39 | C's framework, with Python era defaults and preflights that refuse only on certain rejections | C as-is (hard stake/permit refusals) | client-side stake-weight recomputation risks false negatives | J-CF §3.3 defect 5 |
| 40 | Version key: `Zero` or `Fixed`, refuse below the chain's key; no follow-the-chain mode | B's `FromChain` | following defeats the owner's forced-upgrade lever; the check happens at reveal | C open question 5; [R] C1 §2.3 |
| 41 | 19 crates: btauth in one crate with features; tx/intents split; separate `rtao-runtime` and `rtao-rpc` | D (14), C (18+4), B (23), A (near-monolith) | modular compile units without GAT-heavy sprawl | J-PD §4 A, §4 B |
| 42 | `disallowed-methods` everywhere; the monorepo deny set only for money and `rtao-core-ext`; `subtensor-linting` only on `rtao-core-ext` | D's whole-workspace monorepo lints | keeps staged code upstream-ready without slowing contributors | J-PD §4 D |
| 43 | CLI `tx`/`query` built at run time with clap's builder API | derive macros (B, C) | no per-command code; lower compile cost | J-PD graft 11; A §10.1 |
| 44 | `rtao::blocking` (the facade) mirrors Python's `SyncClient`/`Subtensor`/`set_weights`, driven by `rtao_client::blocking::BlockingRuntime` | re-export the core's blocking `Client` (D); facades inside `rtao-client` | parity; the core client's product behavior diverges; the facades wrap reads, the executor and intents, which sit above `rtao-client` | `PY/sync.py:24-25, 401-476`; `PY/_subtensor.py:36, 167-205`; P-A §1.4 table; §2.3 |
| 45 | Release distribution deferred by Q30 | local multi-target artifact/rehearsal campaign | current task is client development; native platform coverage remains unverified | §12.8 |
| 46 | Sign extrinsic v4 only; drift alert on any move toward v5 | adopt v5 | the core and Python sign v4 | `CORE/codec/extrinsic.rs:265-273` [V] |
| 47 | Dual `MIT OR Apache-2.0` (decided by the user, §15.2 Q1) | keep MIT | vendored and ported Apache-2.0 material; keeps a monorepo move open | `UP/LICENSE`; D §2.6 |
| 48 | MSRV 1.89, equal to the monorepo toolchain at `core_tag`, checked with `cargo +1.89` on the facade and `rtao-cli` | 1.88 (subxt floor); `--workspace` on 1.89 | the core is CI-tested there; `rust-toolchain.toml` outranks `rustup default`, and tooling follows stable | `UP/rust-toolchain.toml` [J-MA §3.4]; §2.5 |
| 49 | Head streams resubscribe and backfill by default (D-012); `blocks_strict` keeps Python semantics | Python's dying stream | operators need gap-free streams | `rpc.py:582-602` |
| 50 | The library never prompts (D-003); the CLI prompts only on a TTY | Python's prompt fallback | libraries must not block on stdin | [R] E §4.2.9; A §6.2 |
| 51 | `RuntimeHandle`, `RuntimeEntry`, `MetadataLevel`, `CallBytes`, `ComposeError` in `rtao-runtime` (no I/O); `RuntimeCache` in `rtao-client` | the handle in `rtao-client` | `Call::compose` and `verify_surface` in `rtao-runtime` take them, and `rtao-client` depends on `rtao-runtime`: otherwise a crate cycle | §3.7 |
| 52 | `ClientError` for every `rtao-client` API, `ReadError` for reads, `SubmitFailure` for `Client::submit`; `TxError: From<ClientError>`; `ChainError` stays Python-shaped in `rtao-tx` | `ChainError` everywhere | a `View` (L4) cannot return an `rtao-tx` (L5) type, and reads must surface policy refusals typed so a neuron can honor `retry_after` | §3.5, §3.9, §4.7, §4.8 |
| 53 | Intent money is `Amount { Exact(Money), All }`, resolved per field by a port of `call_amount` with the unit from the call argument and the netuid from a sibling field | `Amount<Tao>`/`Amount<Alpha>` by name suffix | Python accepts alpha on netuid 0 (TAO 1:1) and the root branches need it; three money fields have no suffix | `PY/intents/_money.py:85-165`; `PY/intents/staking.py:306-345`; §5.2 |
| 54 | Commitments in `rtao_reads::commitments`; the curated `metagraph()` as `rtao_metagraph::SubnetsMetagraphExt` in the prelude, with no inherent `Subnets::metagraph`; drand round math in `rtao_primitives::drand` | commitments in `rtao-metagraph` (a cycle); moving the `Metagraph` model into `rtao-reads` | keeps the three-path decoder, its fuzz target and persistence in `rtao-metagraph`; both alternatives compile, and every example already imports the prelude | `PY/namespaces.py:161-188`; `PY/reads/identity.py:139-162`; §4.8, §8.2 |
| 55 | `StateCell` over `std::sync::RwLock` | `tokio::sync::RwLock` plus a clippy lint; `parking_lot` | std guards are `!Send`, so a guard across `.await` in a `Send` callback is a compile error; clippy does not flag tokio guards, and `parking_lot/send_guard` makes its guards `Send` | §9.1 (reproduced with rustc/clippy 1.99) |
| 56 | btauth server order authenticate → policy → commit → limit; a bounded nonce store that answers 503 at the bound and never evicts | Python's order (verify, then policy); limiter before commit | unregistered keys must not fill the replay store; a captured request must not drain a validator's rate-limit tokens (D-032) | `PY/http_auth.py:126-154`; `signed-requests.mdx:143-156`; §7.2 |
| 57 | Owner rate limits per setter: `LastRateLimitedBlock` for owner hyperparameters, `TransactionKeyLastBlock` for version key, tempo, mechanisms and trims, with the `prod_or_fast!` limits extracted | one `LastRateLimitedBlock` rule | the chain stores those setters' last block elsewhere, and the 7,200/216,000-block limits are absent from metadata and 1 on fast-blocks runtimes | `utils/rate_limiting.rs:38-53, 89-152`; `lib.rs:1128-1132, 3567-3577`; §9.7 |
| 58 | The extension bridge server and client live in `rtao-signer` (feature `extension`); the WebSocket upgrade goes through hyper and tokio-tungstenite 0.30; `ensure_bridge` spawns `rtao extension bridge` | the bridge in `rtao-cli`; axum's `ws` feature | Python exports the bridge as library API; axum 0.8.9's `ws` requires tokio-tungstenite 0.29, a second `tungstenite` that `deny.toml` bans | `PY/__init__.py:47-57, 198-206`; §2.3 rule 2; §6.5 |
| 59 | Bounded hosted client CI, optional targeted infrastructure checks (Q30) | every-push node/container campaigns | units, fixtures, FakeNode and FakeChain provide ordinary development acceptance | §12.2; Appendix E |
| 60 | Binary `rtao` only (decided by the user, §15.2 Q3) | an opt-in `btcli` shim | btcli and rtao share configuration files (§10.2), so a shim could surprise users who have both | §10 |
| 61 | File each upstream PR and issue as soon as its evidence exists (decided by the user, §15.2 Q18) | file all of them in M9 | PR-U5 and PR-U7 fix live defects for the core's other users, and evidence exists from M0 on (PR-U17) | Appendix D |
| 62 | Per-milestone security passes by local cold readers (decided by the user, §15.2 Q19) | a review by the author, by the user, or by an external reviewer | a fresh context sees what the author's context hides, and a local pass needs no outside reviewer | §14; PLAN.md §12.1 |
| 63 | Device acceptance deferred by Q30 | per-release local hardware rehearsal | record missing HID/firmware/screen evidence without blocking client development | §12.8; §11.9 |
| 64 | No time estimates in the plan (decided by the user, §15.2) | engineer-week budgets, calendars and staffing timelines | an AI coding agent executes the plan step by step; order, dependencies and objective exit checks decide progress | §14; PLAN.md "Executing this plan" |
| 65 | Ledger in CI through Speculos (§11.9): a CI-only live lane, recorded transcripts replayed in `pr-gates`, Python request parity through the oracle, and, until PR-U17 ships, the core patch of #66 | a Linux uhid virtual-HID bridge to the emulator, which would run the core's unmodified HID path; a staged copy of the core's Ledger module in `rtao-core-ext`, used in production; an interop lane running Python's `LedgerSigner` on the emulator | the bridge is unverified on hosted runners (it needs the `uhid` module and root) and impossible on Docker Desktop; the staged copy would move production off the core's code for test convenience; Python's device class reaches only HID; the emulator side was executed while planning | `RES/J-ledger-ci-shim.md` §3-§4; G16 |
| 66 | Carry PR-U17 as a Cargo `[patch]` of the core in rtao's own workspace until a `core_tag` contains it (decided by the user, §15.2 Q20); only `rtao-testkit::ledger_emu` names its items | a test-only transcription of `CORE/signers/ledger.rs` in `rtao-testkit` (plan v2; now the fallback); a staged copy in `rtao-core-ext`; a uhid bridge | the emulator lane runs the core's own `ledger.rs` from its first run; Cargo ignores `[patch]` in dependencies, so subnets keep building the upstream tag; `xtask pin --check` bounds the patch to `[core.patch] paths` and fails once it is obsolete | §1.11, §11.9, Appendix D |
| 67 | Direct client commits with applicable hosted checks (Q21/Q30); release/upkeep deferred | full local prepush matrix and milestone release campaign | user limits laptop resources and scope to development | PLAN.md execution rules; Appendix E |
| 68 | File the Ledger app fix with `Zondax/ledger-polkadot` before the plan runs (decided by the user, §15.2 Q22): an issue and a PR that adds `TaoBalance` to the app's balance type names; the agent only follows it up | ask the user before filing, with evidence from M7-09 (plan v2) | the user's decision; the core patch cannot change how the app shows amounts, and the fix is one entry in the app's list of balance type names | Appendix D; `RES/J-ledger-ci-shim.md` §4.6 |
| 69 | Free ubuntu-latest hosted CI and downloadable docs (Q23/Q30) | paid/large runners, Pages and local Mac build requirements | macOS/ARM/hardware coverage stays explicitly unverified | §12.8; §15.2; PLAN.md §12.13 |

---

## 17. What we deliberately do not do (yet)

| # | Not doing | Why | Revisit when |
|---|---|---|---|
| 1 | subxt codegen or `OnlineClient` on any runtime path (only the opt-in `subxt` escape hatch) | a second metadata engine, encoder and signer next to the core's; 8 breaking minors in 23 months ([R] F §1.7) | never for core paths |
| 2 | `chainHead_v1`, `archive_v1`, `transaction_v1` backends; custom `subnetInfo_*`/`neuronInfo_*` RPCs | v11 uses the legacy namespace; `CombinedBackend` fails on finney WS ([R] feasibility-subxt); the custom RPCs lag the runtime APIs | public endpoints deprecate the legacy namespace, or Python moves |
| 3 | Extrinsic v5 signing | the core and Python sign v4 | the core or Python adopts v5 (drift alert, §12.5) |
| 4 | A second encoder, positional codec types beyond `MetagraphView`, or baked hashers | drift surface ([J-CF §3.4]) | never |
| 5 | Parsing V16 at run time | the core rejects V16; V15 carries everything and is what RFC-0078 uses | the core accepts V16 (A's U9 and C's U5 propose it; not pursued here) or the runtime stops serving V15 |
| 6 | Modeling `MaxEpochsPerBlock` deferral | safe direction; unpredictable inputs (§9.3 step 9) | reveal-miss metrics attribute misses to deferral, or upstream models it |
| 7 | Dropping the downstream `w3f-bls` patch | two sources → E0277 | PR-U4 merges **and** the localnet decrypt proof (§11.4 test 16) passes |
| 8 | Pure-Rust reimplementation of NaCl, Fernet or Ansible keyfile crypto | the core owns COMPAT bytes; no-host neurons only need plaintext hotkeys | PR-U6 is declined **and** no-host coldkey unlock becomes a requirement |
| 9 | Following `CommitRevealWeightsVersion` or `WeightsVersionKey` automatically | both are upgrade levers | never (new client formats ship as rtao releases) |
| 10 | Full v10 Synapse/Axon/Dendrite emulation, Epistula, btlightning QUIC | outside the v11 parity target; btlightning pulls crates.io `sp-core 34` next to the fork's 37 ([P] C §7.6) | post-parity M10, or earlier if a migrating subnet needs it |
| 11 | Light client (smoldot) | finney's chain spec lacks a `lightSyncState` and needs a custom warp checkpoint ([R] F §0.7) | upstream ships a light-client-ready chain spec |
| 12 | crates.io publication | user decision; git dependencies and `[patch]` | never (unless the user changes the constraint) |
| 13 | Python at build or run time, or in PR gates | 100%-Rust goal | never |
| 14 | `jsonschema` validation | Python validates via dataclasses | never |
| 15 | Re-exporting the core's blocking `Client` as `rtao::blocking` | it diverges from Python's product behavior | never (it stays a test oracle) |
| 16 | Rich-identical human tables; byte-identical JSON as a gate | rendering libraries differ; value equality is the contract | user demand for byte-identical text |
| 17 | Reading v10 `.pt` metagraph pickles or template `state.npz` | not part of v11 | a migrating subnet asks (an `import-npz` tool would be small) |
| 18 | Auto-update on by default | supply-chain risk; when enabled, an asset without a valid build-provenance attestation from rtao's release workflow is refused (§9.11) | never by default |
| 19 | Dense weight or bond matrices | 6.25M cells at 2,500 UIDs | never |
| 20 | Client-side stake-weight recomputation for refusals | false-negative risk | upstream exposes a stake-weight runtime API with the pallet's exact semantics |
| 21 | Proposing `sdk/bittensor-rs` upstream before parity | the proposal needs a faithful twin first; the path is kept open by the dual license and lint-clean staged code | after M8 |
| 22 | `RemoteSigner` (`--signer remote`) before parity | an rtao addition with no Python counterpart; it needs a wire schema, a reference server that policy-checks before signing, and replay tests before it can ship | M10 (§14) |
| 23 | A `btcli` shim next to the `rtao` binary | the user chose `rtao` only (§15.2 Q3); shared configuration files (§10.2) would let a shim surprise users who have both | the user asks for one |
| 24 | A uhid virtual-HID bridge that runs the core's unmodified HID path against Speculos | unverified on GitHub-hosted runners (it needs the `uhid` module and root), impossible on Docker Desktop, Linux-only, and it adds only the HID framing that the device run covers on real hardware (§11.9) | deferred; any future transport redesign requires separate scope (§1.11) |

---

## Appendix A. Every judge concern, and how it is resolved

"Accepted" means the design implements the concern's remedy. "Accepted, adapted" means a different but equivalent
remedy. "Rejected" means the concern does not hold or the remedy is worse, with evidence. No concern is rejected
outright: two are narrowed.

### A.1 Chain-fidelity judge (J-CF §5)

| ID | Concern | Resolution | Where |
|---|---|---|---|
| CF-1 | Commit-reveal version gate: B, C and D echo storage | **Accepted.** Constant 4 per supported tlock payload format; intents unchanged from Python (`SetWeights` tags 4, `CommitWeights` field default 4); the framework refuses early when storage ≠ 4; e2e scenario 6 | §9.3 step 3, §11.4 |
| CF-2 | Epoch predicate conflict; `MaxEpochsPerBlock` unmodeled; +3 offset empirical | **Accepted** for the predicate (pallet-faithful shim, agreement tests, D-006, PR-U5). **Deferral: deliberately not modeled**, with the direction analysis (it delays reveals, which per-block retries absorb) and event attribution; question Q9 goes upstream. The +3 offset is kept and its margin monitored by the reveal watcher | §1.6 G4, §9.3 steps 7-12, §15.2 Q9 |
| CF-3 | `state_getMetadata` is V14 on finney; V14 has no runtime APIs | **Accepted.** V15 always; V14 only on Python's needle; `MetadataLevel::V14Degraded` refuses signing and serves runtime APIs only through the ported frozen legacy table; V16 blobs stay outside `md*.bin` | §3.4 |
| CF-4 | Policy-refusal parity depends on the WebSocket library | **Accepted.** tokio-tungstenite session behind `RpcClientT`; Python's classification ported from extracted constants; `FakeNode` header tests | §3.2.3, §3.15 |
| CF-5 | `w3f-bls` patch: the drop experiments cannot work; ciphertext equivalence unproven | **Accepted.** Patch mandatory; lock test and `rtao doctor`; PR-U4 gated on a localnet pallet-drand decrypt test | §1.3, §11.4 test 16 |
| CF-6 | btauth retention check moved; verify-error mapping; wrapper default configurable | **Accepted.** Retention check at `Verifier::new` registered as D-009; core `Err` → `MalformedAuth("invalid hotkey address or signature encoding")`; `WrappedPolicy` configurable, default parity | §7.1 |
| CF-7 | Weight quantization must be bit-identical | **Accepted.** `round_ties_even`, Python operation order, oracle vectors with `.5` ties and the cutoff path | §9.3 step 4, §11.5 |
| CF-8 | Nonce on `FateUnknown` (C reserves); C's 32-block weight era | **Accepted.** Python semantics through RAII `NonceLease`, reconciliation layered on top; Python's 128-block era for weight commits | §3.9.1, §3.9.6, §9.3 step 11 |
| CF-9 | Zero-account `Default` maps baked to `None` without verification | **Accepted, adapted.** The pinned metadata settles it offline (the four `Default` `AccountId32` entries default to 32 zero bytes); a typed-only `DefaultAccount` accessor applied by rule; JSON unchanged (D-001); localnet scenario 15 is a sanity check | §5.1 |
| CF-10 | Typed decode slower than `Value`; the "faster" rationale is wrong | **Accepted.** The claim is withdrawn; a shape-gated `MetagraphView` covers hot paths; criterion gates | §8.2, §8.6 |
| CF-11 | B's baked hashers | **Accepted.** Keys always come from the live entry; hashers are recorded only for drift | §3.6, §3.7 |
| CF-12 | Balance text is float-formatted in Python | **Accepted, stronger remedy.** Exact emulation of `str(Balance)` (`pyfmt`), pinned by 10,000 oracle vectors, so read JSON is value-equal with byte-equal balance strings; Rich tables are not claimed | §5.3, §10.3 |
| CF-13 | Extrinsic v5 advertised but unsupported | **Accepted.** v4 only; the drift job alerts on any move by the runtime, the core or Python | §12.5 |
| CF-14 | e2e manifest is 73 + 39, not 72 + 40 | **Accepted.** Re-verified: 112 = 73 `intent_*` + 39 `test_*` | §4 intro, §11.2 |

### A.2 Maintainability judge (J-MA §4 and verdict concerns)

| ID | Concern | Resolution | Where |
|---|---|---|---|
| MA-1 | Downstream patch lockstep with per-release core bumps | **Accepted.** `core_tag`/`parity_tag` split with a core-input fingerprint; the patch line moves only on core bumps | §1.2 |
| MA-2 | Behavior-only Python changes are invisible to catalogs | **Accepted.** Blob-SHA file layer, Rust symbol diff, and oracle diffs per op on every pin bump | §4.9, §11.5, §12.3 |
| MA-3 | jsonrpsee loses handshake headers | **Accepted** (same as CF-4) | §3.2 |
| MA-4 | V14 fallback hazard | **Accepted** (same as CF-3) | §3.4 |
| MA-5 | D's regeneration needs Python | **Accepted.** Extraction is Rust-native (`rustpython-parser`, `syn`); Python stays in CI lanes only | §4.9, §11.5, §12.2 |
| MA-6 | Epoch predicate and unmodeled deferral | **Accepted** (same as CF-2) | §9.3 |
| MA-7 | `DecodeAsType` is slowest; miner admission must not decode per request | **Accepted.** `ArcSwap` handle, O(1) index built per refresh, view parser | §8.2-8.4 |
| MA-8 | D's "46 pass-through" overstates savings | **Accepted.** Bodies are generated for 46, but hooks and post-processing stay hand-written; M5 is scoped on 83 reviewed intents | §4.3, §14 |
| MA-9 | B's unbudgeted type generator; optimistic estimate | **Accepted.** No positional generator beyond the hand view; the milestones are scoped by the parity inventory and closed by exit checks, with no effort figures (§14) | §3.7, §14 |
| MA-10 | B diffs live website catalogs | **Accepted.** Catalogs are vendored from the monorepo at `parity_tag`; early warning reads the monorepo `main`/`testnet` | §4.9, §12.2 |
| MA-11 | Unversioned core needs a compile probe and a canary | **Accepted.** `core_surface` probe, optional hosted `core-canary`, `staged --check` | §1.9, §12.2 |

### A.3 Performance/DX judge (J-PD §7)

| ID | Concern | Resolution | Where |
|---|---|---|---|
| PD-1 | Every neuron binary builds the C toolchain | **Accepted.** No-host profile with a staged plaintext keyfile reader, a differential test, a `deps-check` profile assertion, and PR-U6 | §1.7, §2.4 |
| PD-2 | The `w3f-bls` patch is mandatory; template and doctor required | **Accepted** | §1.3, §2.6, §10.4 |
| PD-3 | Decode-cost claims unsound | **Accepted** (same as CF-10); a hand-visitor measurement is planned, not assumed | §8.6 |
| PD-4 | Policy-refusal metadata over jsonrpsee | **Accepted** (same as CF-4) | §3.2 |
| PD-5 | Fakes at the `RpcClientT` level cannot test the supervisor | **Accepted.** `FakeNode` (socket) plus `FakeChain` (`RpcClientT`) over one `FakeState` | §11.3 |
| PD-6 | Rate-limited public endpoints; preflights issue 10-20 reads | **Accepted.** `fetch` coalescing (one `state_queryStorageAt` per pinned block, ≤256 keys per chunk), token bucket, own-node guidance | §3.2.5, §3.6, §9.3 step 2 |
| PD-7 | D's synchronous `NonceStore` with Redis | **Accepted.** `BoxFuture` async trait (D-005) | §7.1 |
| PD-8 | Weight quantization byte parity | **Accepted** (same as CF-7) | §9.3 |
| PD-9 | `limit_price` typed as a TAO amount | **Accepted.** `PriceRao` override on all seven calls (`Option<PriceRao>` for `remove_stake_full_limit` and for `add_stake_burn`'s `limit`), plus a regen gate on price-named balance arguments | §3.7 |
| PD-10 | Positional types turn appended fields into errors | **Accepted.** Projection hashes for by-name models; positional only for the view, always with fallbacks | §3.7, §8.2 |
| PD-11 | No silent drop to V14 | **Accepted** (same as CF-3). V14 is permitted only when the runtime lacks the V15 method, which is Python's rule and needed for archive parity, not an error in that case | §3.4 |
| PD-12 | Effort realism | **Accepted, adapted.** The plan carries no time estimates (user decision, §15.2); realism comes from operator-first ordering, scope fixed per milestone by the parity inventory, and objective exit checks (§14) | §14 |
| PD-13 | Python-in-CI fragility | **Accepted.** CI-only, cached per tag, `oracle-stale` path with the previous vectors authoritative (no PyPI fallback, §11.5); never gates builds | §11.5, §12.3 |

## Appendix B. Graft ledger

Status: **Adopted** as proposed; **Adapted** in spirit with a stated change; **Rejected** with reason.

### B.1 Grafts requested by the chain-fidelity judge (J-CF §6)

| ID | Graft | Status | Where / note |
|---|---|---|---|
| CF-G1 | Extraction over hand-copy; CI asserts Python mirrors equal runtime sources (fee filters; `PROXY_TYPES` vs metadata) | Adopted | §4.1, §4.9 layers 4-5 |
| CF-G2 | `rtao-core-ext` with `UPSTREAM-TARGET`, `staged --check`, `core_surface`, optional hosted `core-canary` | Adopted | §1.6, §1.9, §12.2 |
| CF-G3 | Python-oracle semantics (composed-call trees, violations, effects, warnings, fees; reads as parsed values); upstream recorders | Adopted | §11.5, §11.2 |
| CF-G4 | Receipt parity staged as `ops::receipt`, PR-U2 | Adopted | §3.9.4 |
| CF-G5 | Always V15; V14 degraded; needle-only fallback; V16 blobs outside the prune glob | Adopted | §3.4 |
| CF-G6 | `clippy` `disallowed-methods` | Adopted (plus `should_run_epoch`) | §1.8 |
| CF-G7 | Per-call fail-closed `verify_surface` at connect and on every upgrade | Adopted | §3.7 |
| CF-G8 | Pallet-faithful epoch shim with injectable clock and agreement tests; delete on U5 | Adopted | §1.6, §9.3 |
| CF-G9 | Clock-skew check; reveal-miss watcher; operator-first milestones | Adopted (`ClockGuard` with `ChainClock` option) | §9.3, §14 |
| CF-G10 | Drop the no-patch experiment; one-line upstream manifest change; `rtao doctor` | Adopted (plus a localnet decrypt proof as the PR's precondition) | §1.3 |
| CF-G11 | Type-level transaction safety (`ShieldPeriod`, `ShieldCiphertext`, spec-stamped `CallBytes`, `NonceLease`, tx-version refusal) | Adopted | §3.8, §3.9, §3.11 |
| CF-G12 | Constants from the snapshot's runtime (divergence) | Adopted (D-007) | §3.5 |
| CF-G13 | Criterion baselines; borrowed view as an opt-in hot path behind differential and fuzz tests; no `DecodeAsType` speed claim | Adapted: the view is the **default** for full metagraphs because the exact shape gate plus a three-path differential test make it safe, and it can be switched off at run time | §8.2, §8.6 |
| CF-G14 | Optional typed `KeySource<Role>` over the Python-shaped executor; `dyn` keeps the runtime check | Adopted | §4.2, §6.4 |

### B.2 Grafts requested by the maintainability judge (J-MA §5)

| ID | Graft | Status | Where / note |
|---|---|---|---|
| MA-G1 | `parity/map.toml` with blob SHAs, Rust symbol diff joined via `doc(alias)`, `bless`, unmapped files fail | Adopted (plus a conservative auto-bless rule) | §4.9 |
| MA-G2 | Rust-native extraction; Python only in oracle, interop and recorder lanes | Adopted; the Ledger emulator lane joined these CI-only lanes later (§15.2 Q2, Q17) | §4.9, §11.5, §11.9 |
| MA-G3 | Own WebSocket layer; `ReadMethod` so submits are never resent | Adopted (exposed via `RpcClientT`) | §3.2 |
| MA-G4 | Structural shape hash per model and descriptor; lazy per live spec; fallback for reads, refusal for signing; drift classes; typed == dynamic proptests | Adapted: exact hashes for calls and the view, projection hashes for by-name models (so appended fields do not trigger fallbacks) | §3.7, §8.1 |
| MA-G5 | Drop the no-patch experiment; upstream manifest change; `rtao doctor` + in-repo lock test | Adopted | §1.3 |
| MA-G6 | Split `core_tag` / `parity_tag` | Adopted (with a precise core-input fingerprint) | §1.2 |
| MA-G7 | `WRAPS` as typed descriptor references; `RecordingView` exact hits | Adopted | §4.2, §4.8, §11.3 |
| MA-G8 | `disallowed-methods` list | Adopted | §1.8 |
| MA-G9 | `verify_surface` at connect and on every upgrade; V14 fallback for reads only | Adopted | §3.4, §3.7 |
| MA-G10 | `deny.toml` stack bans; optional hosted core-source comparison | Scoped by Q30; no scheduled campaign | §12.2, §12.4 |
| MA-G11 | `NonceLease`, `ShieldPeriod`, typed `PoolRejection` | Adopted | §3.9, §3.11 |
| MA-G12 | Shape-gated borrowed view or indexed owned model for miner admission; benchmark before committing to `DecodeAsType` | Adopted (both: view + O(1) index) | §8.2-8.4 |

### B.3 Grafts requested by the performance/DX judge (J-PD §6, as listed in the synthesis brief)

| ID | Graft | Status | Where / note |
|---|---|---|---|
| PD-G1 | Borrowed `MetagraphView` (`Columns::{Borrowed, Owned}`), shape-hash gate, automatic fallback, O(1) `by_hotkey`; `DecodeAsType` elsewhere | Adopted | §8.2 |
| PD-G2 | Compile-time signer roles; `DynIntent`/`DynKeySource` for the CLI and agents | Adopted (without GATs) | §4.2, §6.4 |
| PD-G3 | Static TAO vs alpha; no panicking `Add`/`Sub`; checked `to_wire_for` | Adopted (plus `DynBalance` for Python-shaped data) | §5.2 |
| PD-G4 | Codegen semantic overrides (`PriceRao`, `StakeWeight`, `ZeroAccountIsNone`, text, typenum `Frac`) | Adopted; `ZeroAccountIsNone` adapted to a typed-only `DefaultAccount` applied by rule over the pinned metadata | §3.7, §5.1 |
| PD-G5 | Transaction-safety types, `ReadMethod`, spec-stamped `CallBytes` | Adopted | §3.2, §3.8, §3.9 |
| PD-G6 | `rtao doctor` and the one-line upstream manifest PR | Adopted | §1.3, §10.4 |
| PD-G7 | Socket-level fakes with fault injection; port of `test_rpc.py`; `RecordingView` | Adopted | §11.3 |
| PD-G8 | Session on tokio-tungstenite exposed through `RpcClientT` (`LegacyRpcMethods`, `MockRpcClient`, optional `OnlineClient`) | Adopted | §3.2 |
| PD-G9 | Criterion baselines on B's payloads with a > 30% gate | Adopted (payloads vendored in `vendor/bench/`) | §8.6, §11.7 |
| PD-G10 | One pin file driving core tag, patch, artifacts, digests, toolchain; `xtask pin`; `core_surface`; optional hosted core-source check | Adopted (with two tags) | §1.2, §1.9 |
| PD-G11 | `rtao-core-ext` with `UPSTREAM-TARGET` and forced deletion | Adopted | §1.6 |
| PD-G12 | xtask-generated committed intent interfaces (no proc-macros); generated pass-through bodies; `AnyIntent`; `pending.toml` | Adopted | §4.2-4.5 |
| PD-G13 | Extract literal tables; CI-only Python oracle per pin | Adopted | §4.1, §11.5 |
| PD-G14 | Chunk `state_queryStorageAt` at one pinned hash; `rtao query --block`; dual license from day one | Adopted (the dual license was decided by the user, §15.2 Q1) | §3.2.5, §10.1, §2.5 |
| PD-G15 | `round_ties_even` and Python's summation order with oracle vectors | Adopted | §9.3 |
| PD-G16 | `#[doc(alias = "bittensor.…")]` and a generated v11 → rtao migration table | Adopted | §4.4, §13 |

---

## Appendix C. Divergence register (initial `parity/divergences.toml`)

Every entry needs a pinning test and, where upstream is wrong, an upstream issue or PR. The oracle accepts only listed
divergences. Classes:

- `stricter`: rtao refuses something Python allows;
- `improved`: a different and better result;
- `kept`: a Python quirk reproduced on purpose;
- `addition`: no Python counterpart, outside the parity surface;
- `typed-only`: the Rust type differs, the JSON is identical.

| ID | Upstream symbol | Python behavior | rtao behavior | Class | Test | Upstream |
|---|---|---|---|---|---|---|
| D-001 | `reads.neurons.hotkey_owner`, `reads.collateral.miner_collateral` (+ `Keys`, `SubnetOwner`, `SubnetOwnerHotkey`) | zero-account default rendered as an ss58 address; `None` check never fires (the metadata defaults are 32 zero bytes) | `DefaultAccount::get()` returns `None` for the zero account; JSON unchanged; applied by rule over the pinned metadata | typed-only | `zero_account_default_decodes_to_zero_bytes` (offline, vendored metadata), `default_account_json_parity` (e2e 15 is a sanity check) | issue: dead `None` check |
| D-002 | `delegate_take`, `coldkey_swap_announcement`, `hotkey_identities`, `miner_collateral` | gather at head without pinning (may straddle blocks) | pinned to one block | improved | `RecordingView` single-block test | issue |
| D-003 | `wallets.resolve_wallet_password` / keyfile prompting | the library may prompt | the library never prompts (`PromptPolicy::Never`); the CLI prompts on a TTY | improved | `library_never_prompts` | — |
| D-004 | connect | no genesis check | signing refused on a genesis mismatch for named networks | stricter | `genesis_mismatch_disables_signing` | — |
| D-005 | `http_auth.NonceStore` | synchronous `check_and_store` | async (`BoxFuture`) | addition (API) | `redis_store_does_not_block` | — |
| D-006 | `timelock.get_encrypted_commit_v2` → core `epoch_schedule` | safety net `> MAX_TEMPO` | pallet predicate `> tempo`, with the counter cap | improved | `epoch_schedule_agrees_except_safety_net` | PR-U5 |
| D-007 | `snapshot.Snapshot.constant` | read from the head codec | read from the snapshot's runtime | improved | `constants_follow_snapshot_spec` | issue |
| D-008 | `_transport.interface.attach_signature` | refuses on a `spec_version` change only | also refuses on a `transaction_version` change | stricter | `submit_signature_refuses_tx_version_change` | issue |
| D-009 | `http_auth.verify` retention check | `ValueError` after a verified signature | `ConfigError` at `Verifier::new` | stricter | `verifier_rejects_short_retention` | — |
| D-010 | `_transport.runtime` V14 path | signs with V14 metadata if ever selected | V14 runtimes are read-only (runtime APIs via the legacy table only) | stricter | `v14_degraded_refuses_signing` | — |
| D-011 | *withdrawn* | Python never retries shielded submissions (`PY/executor.py:812-824` returns before the retry loop at `:833-845`; `submit_shielded` has no `retries`, `:897-909`) | the same: shielded submissions are not retried (§3.9.6) | — | — | — |
| D-012 | `Client.blocks` / subscriptions | the stream dies on reconnect | resubscribes and backfills by number (`blocks_strict` keeps Python semantics) | improved | `blocks_gap_free_across_reconnect` | — |
| D-013 | `Executor.execute` | composes and submits, even across a runtime upgrade | refuses spec-mismatched `CallBytes` and re-plans once | stricter | `execute_replans_on_spec_change` | — |
| D-014 | `btcli query` | no block flag | `rtao query <read> --block <n>` | addition | `query_block_flag` | — |
| D-015 | `RpcSession` | no client-side rate limit | token bucket on public presets (timing only) | addition | `ratelimit_paces_public_presets` | — |
| D-016 | `SetWeights` path, framework only | n/a (no framework) | `WeightScheduler` refuses when `CommitRevealWeightsVersion ≠ 4`; the intents are unchanged | addition | e2e scenario 6 | — |
| D-017 | `ServeAxonTls` | a certificate over 65 bytes is silently dropped on chain | rejected client-side with an explanation | stricter | `tls_certificate_length_checked` | issue |
| D-018 | docs `advanced.mdx:146-150` | the docs claim a hotkey-shield refusal that the code lacks | follows the code (no refusal) | kept | `hotkey_shield_not_refused` | issue: docs drift |
| D-019 | `Metagraph.total_stake` | alpha-tagged `Balance` | `StakeWeight` type; JSON unchanged | typed-only | `stake_weight_json_parity` | question (§15.2 Q14) |
| D-020 | `execute_proxy_announced` | builds the inner intent against the signing wallet ([R] A1 §10 item 6) | same | kept | oracle vector | — |
| D-021 | `Policy.allowed_netuids` | intents without a `netuid` field pass any allowlist ([R] A1 §10 item 20) | same | kept | oracle vector | — |
| D-022 | generated call builders: limit-price arguments (`limit_price` of 88, 89, 90, 103, 144, 149; `limit` of `add_stake_burn`, 132) | `TaoBalance` declared; a price in practice | `PriceRao` / `Option<PriceRao>` | typed-only | `limit_price_is_price` | issue: type annotation |
| D-023 | `ExtrinsicResult` | no inclusion level; fate unknown visible only in the message | adds `inclusion` and `fate_unknown` fields; `to_json()` unchanged | addition | `extrinsic_result_json_parity` | — |
| D-024 | `_transport.extrinsics` watch statuses | `retracted`, `finalityTimeout` and `dropped` are fatal, like `usurped` and `invalid` (`PY/_transport/extrinsics.py:352, 364-369`) | `retracted` keeps watching, then reconciles; `finalityTimeout(h)` resolves from block h; `dropped` reconciles when `reconcile` is on (§3.9.3) | improved | `retracted_then_finalized_is_success`, `finality_timeout_resolves_from_block` | issue |
| D-025 | shielded submission era | submits a carrier whose era the finality lag has already expired, and fails | refuses with `TxError::ShieldEraExpired` when B + 2 ≥ F + p; `ShieldPeriod` accepts only 4 and 8 (§3.11 step 1) | stricter | `shield_era_refused_at_lag` (`P8` at lags 5 and 6, `P4` at lags 1 and 2) | issue |
| D-026 | `executor._submit_encrypted_call` nonces | takes n, pins n+1 then n; the cache is left at n, so the next submission reuses n+1 and gets `Stale` (`PY/executor.py:1037-1052`) | `lease_pair` leaves `next = n + 2`; settle rules per lease (§3.11 step 2) | improved | `shielded_then_plain_uses_n_plus_2`, `shield_inner_missing_clears_nonce_cache`, `shield_wait_pool_keeps_n_plus_2` | issue |
| D-027 | `sync.SyncClient` namespaces | 13 attributes, `collateral` absent (`PY/sync.py:186-241`) | all 14 namespaces | improved | `sync_client_has_all_namespaces` | issue |
| D-028 | `result.chain_error_from_dispatch` (nested module errors) | resolved through the package's generated table (`PY/result.py:636-656`) | resolved through the block's runtime, as top-level errors are (§4.7) | improved | `nested_module_error_follows_block_runtime` | issue |
| D-029 | `btcli tools` / `list_tools` | 9 keys per tool (`PY/intents/registry.py:43-66`) | adds `rust {type, role, output, cli}` | addition | `tools_json_equals_python_on_upstream_fields` | — |
| D-030 | `btcli tx` | no era flag (period 128, `PY/settings.py:22`) | `--period <n>` | addition | `tx_period_flag` | — |
| D-031 | `--signer` | wallet, extension, ledger, vault (`PY/cli/globals.py:276-286`) | adds `remote` (lands with `RemoteSigner` in M10) | addition | `signer_remote_flag` | — |
| D-032 | btauth server adapter | the recipe applies policy after `verify`, so a replayed request from a denied hotkey gets 401 (`signed-requests.mdx:143-156`) | authenticate → policy → commit → limit: the same request gets 403; `Verifier::verify` keeps Python's order (§7.2) | addition | `policy_before_replay_check` | — |
| D-033 | `timelock.encrypt` | `reveal_in`, `reveal_at` or `reveal_round` | adds `Reveal::InBlocks(n)` (§9.8) | addition | `timelock_reveal_in_blocks` | — |
| D-034 | `multisig_approve` plan | sends a hash-only `approve_as_multi` at the threshold without comment | same call, plus the warning "this approval cannot execute the call; send multisig_execute" (§3.12) | addition | `approve_at_threshold_warns` | — |
| D-035 | `balance.Balance` local arithmetic | arbitrary signed integer rao | all balances use u64 rao with checked underflow/overflow; valid-chain formatting retained (Q25) | stricter | `native_balance_rejects_negative_and_oversized_input`, `native_balance_arithmetic_reports_overflow_and_underflow`, `native_balance_keeps_valid_chain_json` | user-directed API choice; no upstream defect |
| D-036 | secret constructor and keyfile diagnostics | invalid input can appear in dependency errors and their source chains | fixed categories/messages and owned error wiping; valid pinned derivation retained | stricter | `wipes_every_owned_core_error_message_before_discarding_it`, `redacted_errors_expose_only_the_selected_static_input_label`, `missing_keyfile_field_errors_redact_unknown_input_key_names` | [subtensor#3252](https://github.com/RaoFoundation/subtensor/issues/3252); dependency-owned allocation risks remain open |
| D-037 | password and keyfile reads | password files read to EOF without a size or file-type bound | regular opened descriptors and a 16 MiB content cap; regular symlinks and whole-file trimming preserved | stricter | `password_files_reject_oversize_nonregular_and_invalid_utf8`, `bounded_wallet_reads_reject_large_and_nonregular_files` | local input-budget policy, M2-S05; additional Unix FIFO/symlink regression recorded in wallet evidence |
| D-038 | btauth retention equality | accepts retention equal to max_age + allowed_skew | requires strict greater-than because freshness and expiry include equality | stricter | `test_window_wider_than_store_retention_refused` | source-backed proof and filing audit in progress |
| D-039 | `Client.multisig` construction | derives an account for singleton sets, duplicate members and invalid thresholds | refuses these inputs before deriving an account; the pallet rejects an empty threshold-one other-signatory list | stricter | `account_derivation_uses_core_sorted_set_and_refuses_invalid_members`, `composition_checks_stamps_and_refuses_batch_inside_origin_wrappers` | source-backed issue filing pending |
| D-040 | `subnet_start_schedule` | uses the initial delay constant and unbounded addition | reads mutable `StartCallDelay` and uses the runtime's saturating u64 addition | improved | `d040_mutable_start_delay_overrides_initial_default`, `d040_start_schedule_saturates_u64_eligibility_bound` | source-backed local draft; no new filing |
| D-041 | automatic multisig approval continuation | a removed pending operation can become a new opening | retains the original inner call and opening, refuses changed state, and excludes completed approvals from owner-rate retries | stricter | `unknown_disappeared_or_replaced_opening_refuses_before_the_next_source_or_signature`, `completed_multisig_owner_rate_failure_preserves_receipt_without_reapproval` | source and fixture coverage recorded in `parity/divergences.toml`; hosted validation pending |
| D-042 | CLI book and recursive input budgets | no matching 16 MiB and 128-level policy | refuses oversized books and excessive reference/JSON nesting without selecting another account | stricter | `oversized_books_refuse_instead_of_redirecting_names_to_wallet_fallback`, `programmatic_raw_call_json_depth_refuses_before_runtime_encoding` | local safety policy; hosted validation pending |
| D-043 | generated query plural account parameters | splits arrays into CSV entries but resolves names only for scalar `_ss58` parameters | resolves each explicit `_ss58s` account reference before dispatch, preserves order/duplicates and refuses unknown entries | addition | `account_list_queries_resolve_every_reference_and_refuse_an_unknown_entry`, `query_resolves_csv_contact_names_before_actual_batched_read_dispatch` | prepared source; hosted validation pending; CLI build unverified; additive convenience with no upstream filing |
| D-044 | CLI PoW preparation and stale dry runs | composes stale work for previews and refreshes only after confirmation/unlock | actual unshielded requests refresh publicly before strict planning and again after unlock; dry runs never mine and reject stale challenges | stricter | `initially_expired_future_and_difficulty_invalid_pow_prepare_before_signing`, `expired_pow_dry_run_never_enters_public_mining_or_unlocks`, `late_pow_refresh_preserves_proxy_owner_and_explicit_hotkey_after_unlock` | prepared source; hosted validation pending; local policy without an upstream defect claim |

## Appendix D. Upstream contribution plan

Each PR's staged module is deleted when the PR ships in `core_tag` (`staged --check`, §1.6). None of them blocks rtao.
A PR whose symbols land only behind `host` does not retire a staged module that the no-host profile uses, because
`staged --check` probes without `host` (§1.6); PR-U1 and PR-U2 therefore target an ungated `protocol` module. PR-U7 and
PR-U8 retire nothing in rtao: rtao never calls `submit_shielded`, and PR-U8's reuse is in `tests/policy_equivalence.rs`,
which already builds the core with `host` as a dev-dependency. PR-U17 retires no staged module either: it retires the
core patch that rtao's workspace carries until PR-U17 ships (G16, §1.11).

Each PR and issue is filed as soon as its evidence exists (§15.2 Q18): the last column names the PLAN.md §12 step whose
output completes the evidence, and PLAN.md §15 records each filed link.

| PR | Target | Scope | rtao-side effect when merged | Evidence at step |
|---|---|---|---|---|
| PR-U1 | `sdk/bittensor-core/src/protocol/{metadata,storage,runtime_api,extrinsic_lookup,dispatch_error,fee}.rs`, a new module declared in `lib.rs` without `#[cfg(feature = "host")]`; the blocking `client.rs` calls into it | move the private protocol functions of `client.rs` (`fetch_metadata` :1234, `decode_storage` :1352, runtime-API encoding :469-499/:1334, `find_extrinsic` :942, `decode_outcome` :993, `dispatch_error` :1365) into public sans-IO functions that the blocking client also calls; add the `TransactionPaymentApi_query_info` probe builder and `RuntimeDispatchInfo` decode | delete `rtao-core-ext::ops::{metadata, storage, runtime_api, extrinsic_lookup, dispatch_error, fee}` | M2-06 (with M1-02) |
| PR-U2 | `sdk/bittensor-core/src/protocol/receipt.rs` (ungated) | Python-parity outcome decoding: `Sudo.SudoAsDone`, `Utility.BatchInterrupted`/`ItemFailed`, MevShield failure events, `Treasury`/`Balances.Deposit` fee fallback, and `Sudo.Sudid`'s `sudo_result` field (today a failed `Sudid` reports success, §3.9.4); evidence: the `FakeChain` failed-`Sudid` test (M2) and scenario 18's Sudo-wrapped ops (M5) | delete `ops::receipt` | M2-06; scenario 18 (M5-06) adds live evidence |
| PR-U3 | `website/apps/bittensor-website/scripts/generate.py` → `errors.json` | export custom transaction codes, invalid-tx needles, dispatch errors, per-code explanations, help overrides | extraction from `error_map.py`/`result.py` disappears | M2-03 |
| PR-U4 | `sdk/bittensor-core/Cargo.toml` | `w3f-bls = { version = "=0.1.3", features = ["std"] }` instead of `{ workspace = true }`; the root patch keeps the monorepo on `vendor/w3f-bls`; **with** rtao's evidence from `ci/pr-u4-probe/` that crates.io-`w3f-bls` ciphertexts decrypt offline and in pallet-drand, and the four-file source diff (§1.3, §11.4 test 16) | downstream `[patch.crates-io]` line removed from the template; `rtao doctor` relaxes | M2-20 (scenario 16) |
| PR-U5 | `timelock/epoch_schedule.rs`, `timelock/mod.rs` | `should_run_epoch` uses `blocks_since_last_step > tempo`; the simulator caps the counter at `tempo + 1`; safety-net vectors; `generate_commit_v2_at(now_unix, …)` with the clock as a parameter | delete `epoch_schedule` and `timelock_commit`; Python gets the fix too | M2-11 |
| PR-U6 | `sdk/bittensor-core/Cargo.toml` features | split `host` into `keyfiles-json` (pure), `keyfiles-crypto` (sodium, OpenSSL, Ansible), `drand-http`, `rpc-client`, `parallel`, `pow` | delete `keyfile_json`; owner builds can drop pieces too | M2-02 |
| PR-U7 | `client.rs` `submit_shielded` | era ≤ 8 (`MAX_SHIELD_ERA_PERIOD`) for carrier and inner; inner-receipt scan | none (rtao never used it); fixes the core's own users | M4-15 (scenario 11) |
| PR-U8 | `transaction.rs` | public `PolicySubject`/`PolicyInput` so any intent layer can use `Policy::check` | the equivalence test becomes direct reuse | M2-08 |
| PR-U9 | `runtime/mod.rs` | `Runtime::metadata_ir()` (serde), shared by `bittensor-core-py` and rtao, extended with storage hashers and key idents, event field idents, runtime-API signatures and versions | delete `ir` | M1-02 |
| PR-U10 | `keyfiles/mod.rs`, `keys/mod.rs` | pure `encode_password_for_environment`; public `keys::verify_strict` (no `<Bytes>` fallback) | delete `keyfile_env`, `verify_strict` | M2-02 (`verify_strict` from M0-17) |
| PR-U11 | `generate.py` → `intents.json` | add `default` to `input_schema` properties; add `mev_shield_default`, `mev_shield_required`, `all_amount_fields` | extraction of defaults and class vars disappears | M2-03 |
| PR-U12 | `.github/workflows/watch-mainnet-release.yml` | publish `metadata-v15.scale` and `metadata-v16.scale` as release assets | the drift job skips the live fetch for finney | M1-21 |
| PR-U13 | `sdk/python/tests/unit/test_keyfiles_golden.py` | commit encrypted keyfile vectors (NaCl, Fernet, Ansible) with passwords | rtao's interop-recorded cross vectors become upstream fixtures | M3-08 |
| PR-U14 | issue (runtime) | expose `SelectiveMetagraphIndex` through metadata or a runtime API | the `syn` extraction from pallet source disappears | M1-03 |
| PR-U15 | `codec/value.rs` | move the `Value` navigation helpers out of host-gated `client.rs` | delete `value_ext` | M1-02 |
| PR-U16 | `website/apps/bittensor-website/scripts/generate.py` `read_targets` (`:405-480`) | scan each read's module-local helpers and module-level `Item(...)`/`Method(...)` descriptors, so the catalog `hits` are complete (12 reads are under-reported today, parity-inventory §5.15) | `parity/read_hits_extra.toml` shrinks to the out-of-module items | M2-09 |
| PR-U17 | `sdk/bittensor-core/src/signers/{mod.rs, ledger.rs, hid.rs}` | `pub trait ApduTransport: Send + Sync { fn exchange(&self, apdu: &[u8]) -> Result<Vec<u8>, CoreError>; }` in `signers/mod.rs`, outside the `ledger` gate, implemented by the HID `Transport` (its `exchange` moved unchanged); `LedgerDevice` holds a `Box<dyn ApduTransport>`; a new `LedgerDevice::with_transport(transport: impl ApduTransport + 'static) -> Self`; `open()` unchanged, so the bindings are untouched and `LedgerDevice` stays `Send + Sync`; three unit tests; optionally a std-only Speculos TCP transport and a binding constructor, so upstream's own tests can run on the emulator. Evidence: G16 (`CORE/signers/mod.rs:6-9`, `ledger.rs:51-65, 158-219`), the WebHID note in `UP/sdk/bittensor-core-wasm/src/errors.rs:15-18`, and the emulator runs of `RES/J-ledger-ci-shim.md` §1; the lane's transcripts (M7-09) strengthen the open PR. Prototyped while planning (`RES/J-ledger-ci-shim.md` §8): 108 added lines, upstream's checks passing, and the core's `ledger.rs` driving the app through `with_transport`; M0-01 starts from it. Filed from the user's fork (`github.com/sam0x17/subtensor`), whose `rtao-core-patch/<core_tag>` branches carry the same commits into rtao's workspace until a `core_tag` contains them (§1.11) | delete both core-patch tables and `[core.patch]`; adapt `rtao-testkit::ledger_emu` to the API as merged (the lane and `ledger_replay` already run the core's `ledger.rs` through `with_transport`) | M0 (the evidence predates the plan) |

Issues to file (Python package, docs, scripts and runtime), each when the PLAN.md step in parentheses has produced its
pinning test or evidence:

1. `hotkey_owner`/`miner_collateral` `None` checks never fire on zero-account defaults (D-001; M1-03).
2. Gather reads are unpinned (D-002; M4-07).
3. `Snapshot.constant` reads the head codec (D-007; M1-14).
4. `attach_signature` ignores `transaction_version` (D-008; M5-01).
5. A `ServeAxonTls` certificate over 65 bytes is silently dropped on chain (D-017; M3-03).
6. The docs claim a hotkey-shield refusal the code lacks (D-018; M4-03).
7. `Metagraph.total_stake` is declared `TaoBalance` while carrying stake weight (D-019, a question, §15.2 Q14; M1-16).
8. Limit-price call arguments are declared `TaoBalance` although they carry a price (D-022; M1-03).
9. `retracted` and `finalityTimeout` are treated as fatal although the extrinsic may still land (D-024; M2-07).
10. A shielded submission proceeds when the finality lag has already expired its era (D-025; M4-03).
11. After a shielded submission the nonce cache is left at n, so the next submission reuses n+1 (D-026; M4-03).
12. `SyncClient` lacks the `collateral` namespace (D-027; M6-03).
13. Nested module errors resolve through the generated table instead of the block's runtime (D-028; M2-08).
14. `scripts/record_golden.py` hard-codes `SubtensorModule.Alpha`, removed after spec 461; replace it with `AlphaV2` in
    `STORAGE_KEY_CASES` (§11.2; M2-22).
15. A question: should `get_next_epoch_start_block` and the reveal predictor account for `MaxEpochsPerBlock` deferral
    (§15.2 Q9; M2-17)?

Outside the monorepo, the Ledger app (`Zondax/ledger-polkadot`) formats an amount only for a fixed list of balance type
names (`DOTAPP/app/src/metadata_parser.c:74-75`), so stake amounts typed `TaoBalance` appear on the device as raw
integers (`RES/J-ledger-ci-shim.md` §4.6). At the user's request (§15.2 Q22) the fix was filed on 2026-10-13, before the
plan runs: issue [Zondax/ledger-polkadot#295](https://github.com/Zondax/ledger-polkadot/issues/295),
PR [#296](https://github.com/Zondax/ledger-polkadot/pull/296), which adds `TaoBalance` to that list; `AlphaBalance`, a
per-subnet token and never TAO, stays unformatted. The agent only follows it up (answering maintainer questions and
pushing the changes they ask for to the PR branch), and the Ledger guide's note stays until a released app version
formats these amounts.

## Appendix E. CI workflows

Q30 retires the earlier workflow specimen with many scheduled node/container lanes and per-milestone releases.
Actual workflow files must follow the bounded client scope of PLAN.md §9.7 before being enabled.

- Use only free `ubuntu-latest`. Pin actions by SHA, set timeouts, and cancel superseded development runs.
- Run applicable Rust tests, Clippy, formatting, doctests, source/generated/pin/dependency guards, supply audits
  and selected MSRV/features in hosted jobs. Preserve raw audit output and the exact Q26-Q28 guards.
- `cargo xtask gates` is hosted orchestration. The laptop does not run the full matrix before pushes. The cargo-hack
  no-dev snapshot is temporary CI data; validate retained lock identities and remove it after the job. Do not copy
  its targets/caches to the laptop or prepare local replicas.
- Oracle refresh, cross-language interop, emulator, localnet and targeted fuzz/performance checks are optional
  hosted jobs for concrete changes. Use timeouts and the smallest relevant selection; report absent evidence.
  Never fall back to local containers or nodes when free CI cannot fit a check.
- Remove mainnet-clone/full-node jobs. Keep release, automatic upkeep and hardware/platform rehearsals disabled or
  deferred. No paid runner, build farm, local Mac build or device run is a client-development prerequisite.
- Keep useful bounded logs and reports as downloadable artifacts. Do not deploy Pages or populate the laptop with
  CI build products. Never add or toggle the `CI` label to start or retry CI.

## Appendix F. Verification log (re-checked on disk by the synthesizer)

| # | Fact | Evidence |
|---|---|---|
| 1 | Python tags commit-reveal version 4: constant, `SetWeights` use, `CommitWeights` field default | `PY/intents/weights.py:33, 411, 472-477` |
| 2 | Python quantizes with `round((weight / top) * U16_MAX)` and clips with `epsilon = 1e-7`, sorted `estimation` and `accumulate` | `PY/intents/weights.py:54-114` |
| 3 | The chain requires equality with `CommitRevealWeightsVersion` "to gate runtime upgrades"; the arg is "the client (bittensor-drand) version"; the commit is `Pays::No` | `UP/pallets/subtensor/src/subnets/weights.rs:300-345`; `UP/pallets/subtensor/src/macros/dispatches.rs:1946-1948` |
| 4 | Core predicate `> MAX_TEMPO_U64`; pallet predicate `> tempo`; pallet counter cap `tempo + 1`; deferral via `PendingEpochAt = block + 1` + `EpochDeferred`; Null epochs cap the per-block count at 1; `get_next_epoch_start_block` ignores deferral and the safety net | `CORE/timelock/epoch_schedule.rs:45-58`; `UP/pallets/subtensor/src/coinbase/run_coinbase.rs:356-431, 1224-1238, 1240-1274` |
| 5 | V14 parse yields `apis = Vec::new()`; V16 is rejected | `CORE/runtime/mod.rs:163-199` |
| 6 | Python fetches V15 and falls back to `state_getMetadata` only on the missing-method needle; disk cache `md1-<chain16>-<spec>.bin`, `STMD` + `<IB`, prune glob `md*.bin`, 8 files | `PY/_transport/runtime.py:33-39, 55-122, 292-312` |
| 7 | Pre-V15 and `Vec<u8>` runtime APIs go through a frozen legacy table | `PY/_transport/runtime_api.py:1-13, 95-171, 167` |
| 8 | jsonrpsee rejects a handshake as `Rejected { status_code }`; tungstenite exposes `Error::Http(Box<Response<Option<Vec<u8>>>>)` and `CloseCode::Policy` = 1008 | `REG/jsonrpsee-client-transport-0.26.1/src/ws/mod.rs:207, 522-524`; `REG/tungstenite-0.28.0/src/error.rs:69-75`, `src/protocol/frame/coding.rs:154, 214, 245` |
| 9 | `RpcClientT` (`request_raw`, `subscribe_raw`); subxt-rpcs default features pull jsonrpsee; `Error::Client(Box<dyn Error>)`; `LegacyRpcMethods::new(RpcClient)`; `RpcConfig` | `REG/subxt-rpcs-0.51.1/src/client/rpc_client_t.rs:26-80`; `Cargo.toml [features]`; `src/lib.rs:45-52, 69-94`; `src/methods/legacy.rs:24-26` |
| 10 | Local registry has tokio-tungstenite 0.26.2/0.28.0, tungstenite 0.26.2/0.28.0, subxt-rpcs 0.51.1, scale-decode 0.16.2, scale-type-resolver 0.2.0, criterion 0.8.2, governor 0.6.3, arc-swap 1.9 | `ls REG` |
| 11 | Python policy constants, non-idempotent methods, handshake 403/429 headers incl. `X-RateLimit-Reset` fallback, JSON-RPC classification, close 1008, fate-unknown text, untransmitted submits still sent | `PY/_transport/rpc.py:48-65, 84-135, 190-241, 686-708` |
| 12 | `NonceCache` semantics (`account_nextIndex`, `use_cache`, `pin`, `clear`) and `_failed_submit` clearing on any failure | `PY/_transport/extrinsics.py:55-84`; `PY/_substrate.py:745-772` |
| 13 | Transient retry substrings; shield ciphertext bound; poll constants | `PY/executor.py:47-65` |
| 14 | Python receipts check 6 nested-result events; the core checks 3 | `PY/_transport/utils/receipt.py:1-52`; `CORE/client.rs:1018-1048` |
| 15 | Shield era must be ≤ 8 (`Stale` otherwise) | `UP/runtime/src/check_mortality.rs:15-22, 88-104` |
| 16 | `host` gates `client`, `keyfiles`, `pow`, `transaction`; keyfile JSON codec uses only non-host constructors; `keys::verify` and the wrapper fallback are non-host; batch decode is serial without `host` | `CORE/lib.rs:10-31`; `CORE/keyfiles/mod.rs:25-50, 175-370`; `CORE/keys/mod.rs:11-37, 79-100, 337-470`; `CORE/codec/batch.rs:14-45` |
| 17 | The core inherits `w3f-bls` from the workspace, with the comment about pallet-drand; root path and patch | `UP/sdk/bittensor-core/Cargo.toml:51-62, 95-127`; `UP/Cargo.toml:40, 299, 335` |
| 18 | No-patch lockfile has two `w3f-bls 0.1.3` sources; the no-patch build fingerprint contains E0277; `tag = "v477"` builds in 12.93 s and prints Alice | `SP/design-bench-nopatch/Cargo.lock:2377-2404`; `SP/design-bench/target-nopatch/release/.fingerprint/bittensor-core-*/output-lib-bittensor_core`; `SP/design-tagpin/tagpin.log` |
| 19 | Decode benchmark numbers and V15 parse time | `SP/design-bench/bench-run-3.txt` |
| 20 | btauth defaults, messages, order, `ValueError` → `MalformedAuth`, post-signature retention check | `PY/http_auth.py:68-102, 242-343` |
| 21 | `str(Balance)` formats a float `amount = rao / 10**9` with `,.9f`; `_jsonable` uses `str(Balance)`; btcli prints JSON via Rich `print_json(json.dumps(...))` | `PY/balance.py:60-149`; `PY/settings.py:15`; `PY/cli/query.py:104-116`; `PY/cli/output.py:562` |
| 22 | Password precedence and keychain semantics; the `security` CLI with service `bittensor.coldkey` | `PY/wallets.py:62-118`; `PY/macos_password.py:1-80` |
| 23 | Catalog counts: 83 intents (75 coldkey / 8 hotkey; 74 signed / 7 subnet_owner / 2 root; 116 unique wraps), 102 reads, 18 codes, 390 chain errors | `jq` over `CAT/{intents,reads,errors}.json` |
| 24 | e2e manifest: 112 = 73 `intent_*` + 39 `test_*` | `UP/sdk/bittensor-core/tests/e2e-manifest.json` |
| 25 | `limit_price: TaoBalance` in calls 88, 89, 90, 103 (`Option`), 144, 149; `limit: Option<TaoBalance>` in 132 (`add_stake_burn`) | `SP/research/_call_index_map.txt:34-36, 46, 80, 84`; finney metadata (spec 477) call 132 fields |
| 26 | Public endpoints about 1 r/s per IP | `UP/docs/guides/local-development.mdx:158-163` |
| 27 | Python CLI is 23,083 lines; `cli/commands/*.py` 7,872 | `find PY/cli -name '*.py' \| xargs cat \| wc -l`; `wc -l PY/cli/commands/*.py` |
