# Judge report: maintainability and upstream tracking (proposals A, B, C, D)

Date: 2026-10-13. Lens: how mechanical a runtime bump (`spec_version`) and a Python SDK change (new intent or read, or a
behavior-only change) are under each design; whether codegen and CI drift detection are concrete; whether the crate
layout is modular and documented; whether the parity matrix is enforceable. Hand-maintained duplication and vague CI are
penalized. Correctness, performance, completeness and effort are scored too, but weighted below maintainability.

Path shorthands: `SP/` = `SP/`;
`UP/` = `SP/upstream/subtensor/` (monorepo @ `850f2755`); `CORE/` = `UP/sdk/bittensor-core/src/`;
`PY/` = `UP/sdk/python/bittensor/`; `REG/` = `REG/`;
`A`/`B`/`C`/`D` = `SP/design/arch-{A-mirror-python,B-type-driven,C-operator-first,D-minimal-custom-surface}.md`.
Markers: **[V]** verified by me in this session (file read or command run); **[R]** from a research report; **[I]** my
inference.

---

## 0. Verdict

| Proposal | correctness / fidelity | maintainability / upstream tracking | performance / async | parity completeness | feasibility / effort | **Total (/50)** |
|---|---:|---:|---:|---:|---:|---:|
| **D, minimal custom surface** | 8 | **9** | 7 | 9 | 7 | **40** |
| C, operator-first | 8 | 7 | 8 | 8 | 7 | 38 |
| B, type-driven | 8 | 7 | 9 | 7.5 | 6 | 37.5 |
| A, mirror Python | 7 | 8 | 7 | 9 | 6 | 37 |

**Winner: D.** It converts the most upstream change into generated code or a failing build. One pin
(`upstream.toml`) drives the core dependency, its patch, the catalogs, the goldens, the extracted Python and runtime
tables, the Python oracle, the localnet image and the toolchain. Intent structs and the `AnyIntent` / read dispatch enums
are generated from the catalogs, so a new upstream intent or read is a compile error until it is ported or listed in a
monotonically shrinking `pending.toml` (D §4.2, §4.7). Every Python literal table is extracted, never hand-copied
(D §4.1). A per-pin Python oracle turns behavior-only Python changes into failing tests and a readable JSON diff
(D §11.5). Staged upstream code deletes itself once the pinned core exports the symbol (D §1.4, `staged --check`).
D's weak spots (Python in CI, line-range provenance, a jsonrpsee transport that loses handshake headers) are fixed by
grafts from A, B and C (§5).

A is the strongest runner-up on *detection* (blob-SHA file map plus AST symbol diff), but it hand-writes the most code:
a 1:1 port of 60,461 Python lines including the 23,083-line CLI, hand-written intent structs, and its own transport.
B is the strongest on runtime-drift safety (structural shape hashes) and performance, but its Python tracking is
catalog-level and it owns a custom type generator. C is the most operationally sound and has the best guard rails
(`disallowed-methods`, fail-closed surface verification, the V14 catch), but it tracks Python behavior changes only
through names and a short file-watch list.

---

## 1. What actually drives rtao's maintenance load (measured)

I diffed six consecutive finalized release pairs with `gh api repos/RaoFoundation/subtensor/compare/<a>...<b>` [V]:

| Release pair | files changed | `sdk/bittensor-core/**` | `sdk/python/**` | `intents/*.py` changed | catalogs changed at the tag | root `Cargo.toml`/`Cargo.lock` |
|---|---:|---:|---:|---|---|---:|
| v466→v467 | 32 | 0 | 7 | `_root_claim_fee.py` (142 lines) | `reads.json` only | 0 |
| v467→v468 | 146 | 0 | 9 | `_root_claim_fee.py`, `registration.py` | intents, reads, errors | 0 |
| v468→v469 | 97 | 0 | 6 | `staking.py` | intents | 0 |
| v469→v473 | 282 | 1 (tests only) | 11 | `__init__.py`, `staking.py` | intents, reads, errors | 2 |
| v473→v475 | 263 | 5 (`pow/*`, `lib.rs`, `Cargo.toml`) | 36 (incl. `cli/` ~716 lines, `pow_registration.py` 255) | 5 files | intents, reads, errors | 1 |
| v475→v477 | 93 | 0 | 3 (`hyperparams.py`) | none | intents, reads | 2 |

Also verified: `v477` resolves to `5c6e83e2` and `compare v477...850f2755` is 9 commits / 32 files, none under `sdk/`
(exactly A's claim) [V]. Cadence is 9 finalized runtimes in 20 days [R: H §0.3]. Spec 478 changes no client-visible
metadata, only the `System.Version` constant (CORRECTIONS #2).

Three consequences frame the scores:

1. **Python is the high-churn upstream.** `sdk/python` changed in 6 of 6 releases and `intents/*.py` in 5 of 6. At least
   one change was behavior-only from the intents catalog's point of view: v466→v467 rewrote 142 lines of
   `_root_claim_fee.py` while `intents.json` stayed unchanged. A design whose Python tracking is catalog-level misses
   these. B watches `intents/*.py` (issue only, B §12.4); C does not watch it at all (C §12.2 `watch.yml` list).
2. **bittensor-core rarely changes.** Its sources changed in 1 of 6 pairs (one more pair touched only tests). All four
   proposals still bump the core git pin on every finalized tag (A §1.7, B §12.5, C §1.8, D §12.2 `pin-bump`). Because the
   downstream `[patch.crates-io] w3f-bls` tag must equal rtao's core tag (§4.1), every such bump forces every subnet
   repository to edit its manifest. The core pin should move only when core-relevant paths change (graft G6).
3. **"Spec bumped, surface unchanged" is the common runtime case.** All four omit `spec_version` from generated headers,
   following `UP/sdk/python/codegen/emit_python.py:22-30` [V], so this case yields no code diff in any of them. B and D
   auto-merge it; A says it "can be auto-merged"; C budgets about an hour of review.

---

## 2. Scenario walkthroughs

### 2.1 Runtime bump (finney spec N → N+1)

| | A | B | C | D |
|---|---|---|---|---|
| Detection | 6-hourly drift on `(spec, :code hash)` for finney/testnet/devnet; full YAML (A App. A) | same; adds `localnet:main` (B §12.2) | same; YAML (C §12.2) | same; full YAML (D §12.2) |
| Diff | semantic diff incl. pallet/call/event/error **indices**, storage, constants, API versions, extensions, V16 deprecations; `subxt diff` attached | same, plus shape-hash changes per descriptor | same | same, plus `check-models` |
| Classification | breaking / behavioural / additive | adds **surface-unchanged** class that auto-merges | none / additive / behavioural / breaking | same as B; auto-merge |
| Generated layer | name-based descriptors from V15 via core `Runtime` + a port of the binding's `metadata_ir` (A §3.7) | full `parity-scale-codec` wire types for 13 pallets + all runtime APIs, from rtao's own generator (B §3.5) | descriptors + typed call-argument structs from V16; surface hash excluding `System.Version` in the header (C §3.6.1) | descriptors + typed call builders from V15 IR (staged `ir`, D §3.5-3.6) |
| Typed models | hand-written `DecodeAsType`, by-name (A §3.8) | generated; `From<wire>` domain types by hand | hand-written `DecodeAsType` (~30) registered in `models.toml` | hand-written `DecodeAsType`; `check-models` against pinned metadata |
| Runtime guard | descriptors validated at connect (name, arity, ident); warn reads, refuse signing | **structural shape hash per descriptor**; mismatch → dynamic fallback for reads, refusal for signing | `verify_surface` at connect and every upgrade; fail-closed per intent | descriptors validated at connect; degraded → dynamic |
| Hand work on a field rename | fix model (runtime `CannotFindField`; caught by nightly decode of recorded payloads) | regenerate; fix `From` impls the compiler flags | fix model (flagged at connect) | fix model (flagged by `check-models` in the drift PR) |

B makes a behavioural runtime change the most mechanical at the wire layer. The price is owning a scale-info → Rust type
generator (generics, recursion, compact fields, typenum fixed-point, substitutions, overrides) that B neither sizes nor
budgets. A, C and D keep hand models, but `DecodeAsType` decodes by field name and ignores extra fields
(`REG/scale-decode-derive-0.16.2/src/lib.rs:222-236` [V]), so additive runtime changes cost nothing.

### 2.2 Python SDK change

| | A | B | C | D |
|---|---|---|---|---|
| New intent or read detected by | catalog layer + **file layer** (every upstream file mapped; unmapped file fails) + coverage gate (A §4.6-4.7) | catalog-drift PR from **live** bittensor.com catalogs (B §12.3) | nightly catalog sync + regex scan of `@register`/`@read` on monorepo `testnet`/`main` (C §4.7) | regenerated `AnyIntent`/`ReadName` enums → **compile error** unless in `pending.toml` (D §4.2, §4.7); early-warning diff of catalogs on `main` |
| Behavior-only change detected by | **git blob SHA per mapped file** + `ruff_python_parser` symbol diff joined through `#[doc(alias = "bittensor.…")]`; PR blocked until `parity bless` (A §4.6) | `upstream-files` hash watch → **issue** (B §12.4) | `watch.yml` sha256 → issue; list omits `intents/*`, `reads/*`, `executor.py`, `result.py` | porting report over broad watched paths via `// py: file:Lx-Ly` provenance + **oracle JSON diff fails the specific op** (D §11.5, §12.1) |
| Interface generation | none: structs hand-written, docstrings copied (A rejects generation, §4.1) | `xtask parity scaffold <op>` emits a struct with `todo!()` | `xtask scaffold intent/read` | structs, defaults, `IntentSpec`, dispatch generated; 46/83 `build()` bodies generated |
| Python literal tables | `xtask transpile` (Rust parser) + `--check` | `hyperparams.toml` hand-transcribed with a hash tripwire; errors from vendored `errors.json` | vendored + regex equality check | `tools/extract_py.py` (Python stdlib `ast`) → generated `py::*` |
| Behavioral oracle | dockerized Python at the tag → `parity/vectors/` | interop-recorded fixtures (reads, `conform`) | interop CLI `--dry-run --json` | per-pin FakeSubstrate dumps for all 83 intents and 102 reads |
| Parity enforcement | `allow.toml` must be empty at M6 | catalog/coverage/MEV checks; `PARITY.md` is generated documentation for non-catalog surfaces | `manifest.toml` statuses incl. Python modules, CLI tree, settings | `matrix.json` incl. executor/transport checklist, Typer CLI dump, signers, `__all__`, hyperparameters, metagraph fields; strict after M7 |

D is the most mechanical, and A is close behind on detection. B and C would let a v466→v467-style behavior change through
without a failing gate.

### 2.3 bittensor-core change and dependency churn

- **Core API drift.** A routes every import through `rtao-core` and runs a `core-oracle` lane. B has a seam contract suite
  but no canary against `main`. C uses `upstream-diff` plus the weekly `core-next` lane and `disallowed-methods`. D has a
  `core_surface` compile probe, a nightly `core-canary`, and `staged --check`. D and C give the earliest signal.
- **Transport library churn.** A and B own a tokio-tungstenite session (about 1-1.2k lines). C and D implement
  `subxt_rpcs::client::RpcClientT` over jsonrpsee (`REG/subxt-rpcs-0.51.1/src/client/rpc_client_t.rs:26` [V]), confined
  to one crate. subxt-rpcs 0.51.1 bumped jsonrpsee 0.24 → 0.26 in a patch release [R: F §1.7], so that exposure is real
  but contained.

---

## 3. Per-proposal assessment and spot checks

### 3.1 A, mirror Python (37)

**Strengths.**
- **Best detection machinery of the four.** The five-layer `xtask parity` is: file map with blob SHAs, AST symbol diff,
  catalog equality including normalized `input_schema`, `syn` parsing of `transaction_error.rs`/`fee_filters.rs`/the
  `SelectiveMetagraphIndex` arms plus `freeze_struct` tripwires, and transpile `--check`. `parity bless` leaves an audit
  trail.
- One tag pins the core, the parity target and the fixtures, and `bump-upstream` checks that the release is final and
  that finney runs it.
- Tooling is Rust-native: the Python parser is `ruff_python_parser`, and Python appears only in the oracle and interop
  lanes.
- Full CI YAML, a compatibility matrix and a maintenance playbook.

**Weaknesses (maintainability).**
- The 1:1 mirror means hand-porting about 30.5k lines of logic plus the 23k-line CLI.
- Intent structs are hand-written without scaffolding.
- It owns its transport.
- `rtao` is one large crate holding intents, reads, executor, wallets, metagraph and `http_auth`, which is less modular.
- The bless workflow is honest but high-friction at 2-4 releases per week.

**Correctness notes.**
- It deliberately inherits the core's epoch-predicate divergence (U2) by calling `generate_commit_v2`, which also reads
  the wall clock.
- `CommitRevealWeightsVersion` stays at 4 for parity (with a guard).
- Its M0 experiment to drop the `w3f-bls` patch cannot work as phrased (§4.1).

| Claim spot-checked | Source | Result |
|---|---|---|
| Python package line counts (`cli/` 23,083 … total 60,461) | `wc -l` over `PY/` | [V] exact; 60,461 includes the 738-line `namespaces.pyi` |
| Website catalogs byte-identical to fetched catalogs | `cmp UP/website/apps/bittensor-website/public/catalog/*.json SP/docs/*.json` | [V] identical |
| jsonrpsee reports a rejected handshake only as `Rejected { status_code }`; tungstenite exposes the response and close code 1008 | `REG/jsonrpsee-client-transport-0.26.1/src/ws/mod.rs:205-210`; `REG/tungstenite-0.28.0/src/error.rs:71`, `protocol/frame/coding.rs:154,214` | [V] |
| `Value` helpers at `CORE/client.rs:1515-1570`; `metadata_ir`/`runtime_api_map` in the binding | `CORE/client.rs:1515,1525,1529,1543,1550,1558,1565`; `UP/sdk/bittensor-core-py/src/runtime.rs:554,579` | [V] |
| Custom codes are parseable from `impl From<CustomTransactionError> for u8` | `UP/common/src/transaction_error.rs:38` | [V] |
| Epoch predicate `> MAX_TEMPO_U64` vs pallet `> tempo` | `CORE/timelock/epoch_schedule.rs:54`; `UP/pallets/subtensor/src/coinbase/run_coinbase.rs:1232` | [V] |
| `v477` = `5c6e83e2`; `v477...850f275` touches nothing under `sdk/` | `gh api …/git/ref/tags/v477`, `…/compare/v477...850f2755…` | [V] 9 commits, 32 files, dirs chain-extensions, docs, pallets, precompiles, runtime, ts-tests, website |

### 3.2 B, type-driven (37.5)

**Strengths.**
- **Structural shape hashes** (B §3.5) are the best runtime-drift guard proposed. Typed positional decode runs only when
  the live registry shape matches; otherwise reads fall back to the dynamic `Value` and signing refuses. Typed-equals-
  dynamic property tests run per committed metadata.
- Drift classes include **surface-unchanged**, which auto-merges.
- `WRAPS` references generated call descriptors, so a renamed call breaks compilation.
- `RecordingView` makes read `hits` exact.
- The `w3f-bls` patch was **measured**: without it the build fails, with it the tag pin works.
- Best performance evidence: measured decode benchmarks and a columnar view.
- Typed safety: `KeySource<R>`, `NonceLease` with RAII clearing, `ShieldPeriod`, typed `PoolRejection`.

**Weaknesses (maintainability).**
- It owns a custom type generator, unbudgeted.
- Python tracking is catalog-level, fetched from the **live** website (which deploys after mainnet and need not match the
  pinned core tag), plus issue-only file hashes.
- `hyperparams.toml` is hand-transcribed.
- 23 crates add Cargo surface.
- No canary against the monorepo's `main` for core API changes.
- The 21-35 EW estimate is optimistic: M6 gives 3-5 weeks to the hand-written CLI groups, against 23,083 Python CLI
  lines.

| Claim spot-checked | Source | Result |
|---|---|---|
| Decode benchmark (core `Value` 330 µs, scale-value 911, codec 29.8, `DecodeAsType` 1,380, view 6.5 at 256 UIDs) | `SP/design-bench/bench-run-3.txt` | [V] numbers present and match |
| No-patch build fails with two `w3f-bls` sources (E0277) | `SP/design-bench-nopatch/Cargo.lock` (two `w3f-bls 0.1.3`: crates.io and git); `SP/design-bench/target-nopatch/…/output-lib-bittensor_core` contains E0277 | [V] |
| `tag = "v477"` resolves to `5c6e83e2`, builds in 12.9 s, prints Alice | `SP/design-tagpin/tagpin.log` | [V] |
| 75/8 signer, 74/7/2 origin, 116 distinct wrapped calls | `jq` over `SP/docs/intents.json` | [V] (139 total, 116 unique) |
| `SelectiveMetagraph` field order equals `SelectiveMetagraphIndex` 0..=76 | `UP/pallets/subtensor/src/rpc_info/metagraph.rs:119-245` vs `:488-570` | [V] 77/77 match after case normalization |
| `freeze_struct` hashes `93460b9b…`, `bb742022…` | `metagraph.rs:14,119` | [V] |
| `MAX_SHIELD_ERA_PERIOD = 8`, `Stale` for longer eras | `UP/runtime/src/check_mortality.rs:19,90-101` | [V] |
| subxt-rpcs reconnecting client drops subscriptions and pending calls | `REG/subxt-rpcs-0.51.1/src/client/reconnecting_rpc_client/mod.rs:5-12` | [V] |
| Epoch vectors are a private `#[cfg(test)]` module | `CORE/timelock/mod.rs:13-14` | [V] |
| Fee wrapper keeps the `ChargeTransactionPayment` metadata | `UP/runtime/src/transaction_payment_wrapper.rs:324-326` | [V] |

### 3.3 C, operator-first (38)

**Strengths.**
- `clippy.toml` `disallowed-methods` makes accidental use of the core's blocking or buggy paths a CI failure (C §1.7).
- `verify_surface` runs at connect and on every runtime upgrade, fail-closed per intent with a dynamic read twin
  (C §3.6.4).
- A correctness catch the others missed: `state_getMetadata` on finney returns **V14**, and the core's V14 `Runtime` has
  no runtime APIs (§4.4).
- `deny.toml` duplicate bans for the SCALE stack.
- The parity manifest covers Python modules, the CLI tree and settings.
- Concrete operator safety rails.

**Weaknesses (maintainability).**
- Behavior-only Python changes are mostly invisible: the watch list omits `intents/*`, `reads/*`, `executor.py` and
  `result.py`, and there is no per-intent oracle.
- Models and intents are hand-written from scaffolds.
- `DISPATCH_ERRORS` and `HYPERPARAMS` are vendored with regex checks rather than generated.

**Correctness notes.**
- It claims a terminal `PolicyRefused { retry_after }` for HTTP 403/429 at handshake while using jsonrpsee 0.26, which
  exposes only the status code. This gap is unacknowledged.
- `DecodeAsType` was the slowest decoder measured, yet C picks it for the metagraph hot path. That is fine for
  validators, but miners need an indexed model.

| Claim spot-checked | Source | Result |
|---|---|---|
| `state_getMetadata` returns V14 (357,499 B); the runtime API returns V15 (400,688 B) | `SP/meta_getMetadata.json` (magic `meta`, version byte 14, 357,499 B); `SP/meta_v15_call.json` (400,693 B with the `Option`/compact prefix) | [V] |
| A V14 `Runtime` has empty `apis` | `CORE/runtime/mod.rs:172-181` (`Vec::new()` in the V14 arm) | [V] |
| `RpcClientT` exists for a custom session | `REG/subxt-rpcs-0.51.1/src/client/rpc_client_t.rs:26` | [V] |
| scale-decode's named-composite decode is by name; `scale-type-resolver` is default-features-off in scale-decode | `REG/scale-decode-derive-0.16.2/src/lib.rs:222-236`; `REG/scale-decode-0.16.2/Cargo.toml:73-75`; `REG/scale-type-resolver-0.2.0/src/portable_registry.rs:59` | [V] |
| SN2 30 s submit / 180 s inclusion deadlines | `SP/upstream/inference-labs-inc_subnet-2/crates/sn2-chain/src/weights.rs:13-14` | [V] |
| Python password precedence | `PY/wallets.py:62-80` | [V] |
| Public endpoints limited to about 1 r/s | `UP/docs/guides/local-development.mdx:160` | [V] |
| btlightning pulls crates.io `sp-core 34` | `SP/upstream/inference-labs-inc_btlightning/crates/btlightning/Cargo.toml:30` | [V] |

### 3.4 D, minimal custom surface (40, winner)

**Strengths.**
- Everything derives from one pin.
- Ownership budget: about 10k generated, about 25k ported, about 9k I/O lines.
- Generated catalog crate and intent structs; compile-time enforcement plus a `pending.toml` that may only shrink.
- Extraction instead of hand-copied constants: Python tables via `ast`, runtime tables via `syn`.
- Per-pin Python oracle with JSON diffs.
- `rtao-core-ext` stages upstream PRs and auto-deletes them via `staged --check`.
- `core_surface` probe and `core-canary`.
- `lint-upstream` with the monorepo's own `subtensor-linting`, which keeps the eventual `sdk/bittensor-rs` move feasible.
- Full YAML for `pr-gates`, `drift` and `pin-bump`.
- The broadest enforceable parity matrix.
- It reuses upstream's own `record_golden.py`/`record_shape_corpus.py` instead of porting the recorders.

**Weaknesses.**
- Python is required in CI for extraction, the oracle (a maturin build of the core) and the recorders.
- `// py: file:Lx-Ly` provenance is brittle under upstream edits.
- The jsonrpsee transport loses handshake rate-limit headers (acknowledged).
- The "46 pass-through intents" overstates savings: several of them carry heavy hand-written preflights
  (`claim_root`'s 771-line fee model, `burned_register`, `register_subnet` completion).
- The Q1 "drop the patch" experiment fails as phrased (§4.1).
- It pins the core to every finalized tag, which drags downstream patch edits along (§1).

| Claim spot-checked | Source | Result |
|---|---|---|
| No `default` in any catalog `input_schema` property | `jq` over `SP/docs/intents.json` | [V] 0 |
| `reads.json` hits: 10 zero, 81 single, 11 multi | `jq` over `SP/docs/reads.json` | [V] |
| 46 of 83 intents map 1:1 to a single wrapped call | re-ran `SP/design-work/classify_intents.py` against `SP/docs/intents.json` and `SP/research/_call_index_map.txt` | [V] 46 simple / 37 complex (68 single-wrap, 15 multi-wrap) |
| `MockRpcClient` builder exists | `REG/subxt-rpcs-0.51.1/src/client/mock_rpc_client.rs:54,171,184` | [V] |
| subxt-rpcs pulls no codegen (jsonrpsee optional) | `REG/subxt-rpcs-0.51.1/Cargo.toml` `[dependencies.*]` | [V] |
| Monorepo lint crate exists and is used | `UP/support/linting/src/*` (`require_freeze_struct.rs`, …), `UP/support/linting/Cargo.toml` (`subtensor-linting`, `publish = false`), `UP/build.rs:10` | [V] |
| `_NESTED_RESULT_EVENTS` drives Python receipts | `PY/_transport/utils/receipt.py:8,42` | [V] |
| FakeSubstrate composes structural `(module, function, params)` | `UP/sdk/python/tests/harness/fake_substrate.py:314-318` | [V] |

---

## 4. Cross-cutting findings no proposal fully resolves

### 4.1 Downstream `w3f-bls` patch lockstep
Without the patch, Cargo locks two `w3f-bls 0.1.3` sources and bittensor-core fails with E0277 (measured by B; [V] via
`SP/design-bench-nopatch/Cargo.lock`). A's M0 experiment (A §2.6) and D's Q1 (D §15.2) propose dropping the patch, which
cannot compile while the core's manifest points `w3f-bls` at the vendored path. A patch at a different tag than rtao's
core pin also yields two sources [I, from the same mechanism]. Combined with §1, a design that bumps the core pin every
release forces every downstream workspace to edit its patch line every release.

Fixes: B's one-line upstream change (`w3f-bls = "=0.1.3"` in `UP/sdk/bittensor-core/Cargo.toml`), `rtao doctor`, and
moving the core pin only when core-relevant paths change.

### 4.2 Handshake policy metadata
jsonrpsee 0.26 reduces a rejected handshake to `Rejected { status_code }` (`REG/jsonrpsee-client-transport-0.26.1/src/ws/mod.rs:205-210` [V]).
Python reads `X-RateLimit-Policy`, `X-RateLimit-Reason` and `Retry-After` there (A1 §6.1). C and D lose this, and C does
not say so. A and B keep it by owning a tungstenite session.

### 4.3 Python in CI
D's regeneration path runs Python (`tools/extract_py.py`, the oracle, upstream's recorders). That is acceptable for a
test oracle, but it makes a second language part of the regeneration pipeline itself. A keeps transpilation in Rust.

### 4.4 V14 metadata fallback
On finney, `state_getMetadata` returns V14 [V], and the core's `Runtime::parse` gives V14 an empty `apis` list
(`CORE/runtime/mod.rs:172-181` [V]). The coexistence feasibility run parsed that V14 blob. A, B and D keep "fall back to
`state_getMetadata`" without restricting it. A silent fallback would break every runtime-API read.

### 4.5 Epoch predicate and reveal prediction
The core's `blocks_since_last_step > MAX_TEMPO_U64` (`CORE/timelock/epoch_schedule.rs:54`) diverges from the pallet's
`BlocksSinceLastStep > tempo` (`run_coinbase.rs:1232`) [V]. A keeps the core's behavior for Python parity. B, C and D port
the pallet predicate. None models `MaxEpochsPerBlock` deferral; all rely on the +3 block offset.

### 4.6 Hot-path decode
`DecodeAsType` measured 1.38 ms at 256 UIDs and 11 ms at 2,500, against 0.33 / 2.68 ms for the core `Value` and 0.03 / 0.26 ms
for codec derive (`SP/design-bench/bench-run-3.txt` [V]). A, C and D use it for the metagraph wire model. That is
acceptable next to a 0.7-0.9 s RPC, but miner request admission must not decode per request.

### 4.7 Catalog provenance
B diffs against the live website catalogs. The site deploys only after mainnet promotion (H §1.6), so the catalogs can
disagree with the pinned core tag. A and D vendor the catalogs from the monorepo at the pinned tag, which is correct.

---

## 5. Grafts the synthesis (built on D) must adopt

- **G1 [A]** Replace D's line-range provenance with A's `parity/map.toml`:
  - one row per watched upstream file, with its Rust path(s), kind and synced git blob SHA;
  - a `ruff_python_parser` symbol diff joined through `#[doc(alias = "bittensor.<path>")]`;
  - `xtask parity bless`;
  - an unmapped new upstream file fails the gate.
- **G2 [A]** Make D's extractor Rust-native (`xtask transpile --check` over the literal subset, with a Rust Python parser),
  so regeneration needs no Python. Keep Python only in the optional oracle, interop and recorder lanes.
- **G3 [A/B]** Own the WebSocket layer (tokio-tungstenite, or soketto under the `RpcClientT` interface) so handshake
  403/429 headers and close 1008 are surfaced. Use B's `ReadMethod` enum so `author_submit*` can never be resent after a
  reconnect.
- **G4 [B]** Compute a structural shape hash for every typed model and descriptor from the pinned metadata at regen, and
  check it lazily per live spec. A mismatch falls back to dynamic for reads and refuses signing. The drift job classifies
  hash changes as behavioural. Add typed-equals-dynamic property tests per committed metadata (a port of the
  `record_shape_corpus.py` byte generator).
- **G5 [B]** Replace the "drop the patch" experiment with the upstream one-line manifest change. Until it lands, ship
  `rtao doctor` and an in-repo test that `Cargo.lock` holds exactly one `w3f-bls` source at the core tag.
- **G6 [judge evidence]** Split `upstream.toml` into a `core_tag` and a `parity_tag`. Move `core_tag` (and the downstream
  patch line) only when `sdk/bittensor-core/**`, root `[workspace.dependencies]`/`[patch]` or `vendor/w3f-bls/**`
  changed between tags. Move `parity_tag` every finalized release.
- **G7 [B]** Make `wraps` typed references to generated call descriptors, so regeneration breaks the build when a
  wrapped call disappears. Add a `RecordingView` test that each read touches exactly its declared targets.
- **G8 [C]** `clippy.toml` `disallowed-methods` for the core's blocking and divergent entry points:
  - `Client::connect`;
  - `transaction::Executor::{new, with_policy}`;
  - `timelock::{generate_commit_v2, decrypt, get_round_info, get_reveal_round_signature}`;
  - `epoch_schedule::predict_first_reveal_block`;
  - `keyfiles::save_password_to_environment`.
- **G9 [C]** `verify_surface` at connect and on every `state_subscribeRuntimeVersion` upgrade, fail-closed per intent.
  Restrict the V14 `state_getMetadata` fallback to reads only, and fail runtime-API calls loudly under V14.
- **G10 [C]** `deny.toml` duplicate bans on the shared SCALE stack and jsonrpsee, so a core bump that splits the stack
  fails at PR time. Add a weekly `core-next` lane.
- **G11 [B]** Transaction-safety types: RAII `NonceLease`, `ShieldPeriod { P4, P8 }`, and a typed `PoolRejection` that
  never retries `Stale` for shield carriers.
- **G12 [B]** A shape-gated borrowed `MetagraphView` (or an indexed owned model built once per refresh) for miner
  admission. Benchmark before choosing `DecodeAsType` for the metagraph.

---

## 6. Score rationale by criterion

- **Correctness and fidelity.**
  - B, C and D score 8. Each corrects the epoch predicate and reads `CommitRevealWeightsVersion`. C has the V14 catch and
    `disallowed-methods`; B has typed safety and measured facts; D has Python-parity receipts and oracle-checked
    behavior.
  - Each also has gaps: C's unacknowledged header gap, D's header gap and failing Q1, B's custom generator risk.
  - A scores 7 for deliberately inheriting the epoch divergence and the wall-clock commit, and for its failing M0
    experiment, despite the highest Python fidelity.
- **Maintainability and upstream tracking.**
  - D: 9, the most generated, compile-time-enforced and oracle-checked.
  - A: 8, best detection but the most hand duplication.
  - B: 7, best runtime guard, but catalog-level Python tracking and an owned generator.
  - C: 7, good runtime guard, weak Python behavior tracking.
- **Performance and async.**
  - B: 9, measured, columnar view, return-position `impl Future` (RPITIT).
  - C: 8, head tracker, `ArcSwap`, selective presets.
  - A: 7, `Value` plus `DecodeAsType` and `async-trait` boxing.
  - D: 7, `DecodeAsType` with no hot-path view, though chunked batches help.
- **Parity completeness.**
  - A: 9, a 1:1 mirror with CLI-tree parity.
  - D: 9, the broadest enforceable matrix.
  - C: 8, manifest including CLI tree and settings.
  - B: 7.5, non-catalog parity is documentation, not gates.
- **Feasibility and effort.**
  - C: 7 (about 42 EW, early operator value).
  - D: 7 (about 33 pw, less owned code; heavier CI infrastructure).
  - A: 6 (35-49 EW, largest hand port).
  - B: 6 (21-35 EW is optimistic given an owned generator and transport).
