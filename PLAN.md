# rtao: plan to build Bittensor subnets in Rust

**Status.** Plan v2.2, 2026-10-14: the user's answers to the open questions are recorded as decisions
([§14.1](#141-decisions-answered-2026-10-13)), including three given after v2 (Q20, the PR-U17 core patch; Q21, one goal
loop without PRs; Q22, the Ledger app fix filed with Zondax), plus Q23's free Ubuntu CI and downloadable docs,
and the plan carries no time estimates. Pins: the plan
was written at `core_tag` = `parity_tag` = `v477` (monorepo tag commit `5c6e83e2`), and its evidence was checked against
RaoFoundation/subtensor at commit `850f2755` (`main`, runtime source at spec 478), whose `sdk/` tree is byte-identical
to `v477` (re-checked: `git diff v477 850f2755 -- sdk` is empty), while finney and testnet ran spec 477 and devnet 478.
Later on 2026-10-13 `850f2755` was released as `v478`, its `:v478` localnet image was published, and finney (from block
#9276925) and testnet moved to spec 478. M0-06 therefore pins the newest release that finney runs, `v478` today, and the
`v477` in manifest and `upstream.toml` examples stands for that tag. The rtao repository today is the `cargo new`
skeleton: package `rtao` 0.1.0, edition 2024, `authors = ["sam0x17"]`, `license = "MIT"`, a placeholder `src/lib.rs`,
two commits (the initial one and the one adding this plan), an untracked `Cargo.lock`, no `LICENSE` file; M0's first
steps relicense it (Q1).

rtao is a set of Rust crates with which Bittensor subnet owners build validators, miners and owner tooling without
Python, at full parity with the v11 Python package (`sdk/python/bittensor`). This file is the entry point. It states the
whole approach and the work plan, as steps an AI coding agent can execute in order. Depth lives in
[architecture.md](docs/plan/architecture.md), the authoritative design.

## 0. How to read this plan

| Document | Contents | Use it to |
|---|---|---|
| `PLAN.md` (this file) | approach, decisions, parity and test strategy, rules for executing the plan, milestones as numbered steps with exit checks, risks, the user's decisions, progress log | understand the plan and execute it |
| [architecture.md](docs/plan/architecture.md) | the design: §0-§17 and appendices A-F (judge concerns, grafts, divergence register, upstream PRs, CI YAML, verification log) | go deep on any mechanism; every section below links into it |
| [parity-inventory.md](docs/plan/parity-inventory.md) | all 83 intents, 102 reads, 18 error codes and 163 hand-written CLI commands, each with rtao home, body kind and milestone | plan and track parity work; review `parity/pending.toml` |
| [python-module-map.md](docs/plan/python-module-map.md) | all 220 files of the Python package plus the 137 other watched upstream files (357 rows), with rtao home, kind and milestone; it seeds `parity/map.toml`, every row `pending` in M0 | port a Python file; find where Python code lands |
| [research/](docs/plan/research/) | 11 research reports, [CORRECTIONS.md](docs/plan/research/CORRECTIONS.md) (overrides them), three measured feasibility checks, and [J](docs/plan/research/J-ledger-ci-shim.md), the Ledger-in-CI research done after the user's decisions | check the evidence behind a claim |
| [panel/](docs/plan/panel/) | the four competing proposals (A-D) and the three judge reports | see why an alternative was rejected |
| [bench/](docs/plan/bench/) | decode benchmark, tag-pinned core build log, no-patch reproduction project | check performance and build measurements |

Reading order: this file, then the architecture sections it links, then the two appendices for per-item detail, then
research and panel documents for evidence. An agent that executes the plan reads
[Executing this plan](#executing-this-plan) first. By role:

- **Deciding** (the user): [§1](#1-summary), [§3](#3-fixed-decisions-and-how-the-design-reads-them),
  [§14.1](#141-decisions-answered-2026-10-13), and the stop-and-ask points of
  [Executing this plan](#executing-this-plan).
- **Implementing** (an AI coding agent or a person): [Executing this plan](#executing-this-plan) first, then
  [§4](#4-architecture-overview), [§5](#5-bittensor-core-taken-wrapped-staged-avoided), [§12](#12-milestones) (M0's
  steps are the [first steps](#1214-first-steps)), then architecture
  [§1](docs/plan/architecture.md#1-boundary-with-bittensor-core)-[§3](docs/plan/architecture.md#3-chain-client-rtao-rpc-rtao-client-rtao-runtime).
- **Reviewing parity**: [§8](#8-python-parity), then [parity-inventory.md](docs/plan/parity-inventory.md).

Conventions:

- "architecture §n" always links to section n of architecture.md.
- Citations use the architecture's prefixes ([Evidence conventions](docs/plan/architecture.md#evidence-conventions)):
  `UP/` is the monorepo at `850f2755`; `CORE/` = `UP/sdk/bittensor-core/src/`; `PY/` = `UP/sdk/python/bittensor/`;
  `PYT/` = `UP/sdk/python/tests/`; `CAT/` = `UP/website/apps/bittensor-website/public/catalog/` (byte-identical to
  <https://www.bittensor.com/catalog/>); `REG/` = the crates.io source of a named crate version; `DOTAPP/` = the
  Polkadot generic Ledger app (`Zondax/ledger-polkadot` at `644ec638`); `RES/`, `P-*`, `J-*` = the research and panel
  documents.
- **[I]** marks an inference or a size estimate (lines, crate counts) that was not executed. The plan carries no time
  estimates: no durations, effort figures, milestone dates or staffing calendars
  ([§14.1](#141-decisions-answered-2026-10-13)).
- Step IDs (`M2-07`) name the steps of [§12](#12-milestones); each milestone numbers its exit checks, and C1-C7 are the
  common exit checks of [§12.1](#121-overview).
- "(re-checked)" marks a claim verified against the upstream sources while writing this plan;
  [§18.2](#182-facts-re-checked-for-this-plan) lists them.

## Executing this plan

The plan is written to be executed step by step by an AI coding agent (the user will run it with Codex, as one goal
loop that commits to `main`, Q21). It carries no time estimates: order, dependencies and objective checks decide what
comes next.

1. **Order.** M0 to M9 run as one continuous goal loop (Q21, [§14.1](#141-decisions-answered-2026-10-13)): work one
   milestone at a time, in numeric order (M0, M1, …, M9), without pausing except at the points of rule 6, and even then
   continue with the work that does not depend on the answer. Numeric order respects every dependency of
   [§12.1](#121-overview). M10 is optional and starts only if the user asks for it. Within a milestone, do the steps
   in ID order (`M2-07` before `M2-08`): each step depends only on earlier steps and earlier milestones. Steps run side
   by side only where the milestone's "Parallel work" note allows it, and work moves ahead of the current step or
   milestone only while a check or an answer is pending (rules 3 and 6).
2. **Done, commits and pushes.** There are no PRs in the rtao repository: the agent commits directly to `main` and
   pushes, without asking, until M9 exits. A step is done when the output its text names exists, its tests are
   committed, and the checks it touches pass (`pr-gates`, plus any lane the step names); its box (`- [x]`) in this file
   is checked in the commit that completes it. Commit messages start with the step ID (`M2-07: …`; work outside a step
   names what it does, such as `pin: v477 → v478`) and carry no AI attribution trailers (the user's standing
   preference). Before every push, run `cargo xtask gates`, the local equivalent of the `pr-gates` job (every `pr-gates`
   step that runs on the agent's OS, as M0-27 defines it; before M0-27 creates it, the step's own checks), and push only
   when it passes. Before starting each step, check `main`'s latest CI runs (`gh run list --branch main`): a red
   blocking lane ([§9.7](#97-ci-lanes-and-what-each-catches): `pr-gates`, `e2e-required`, `core-unpatched`, and
   `interop` on a pin bump) is fixed first (stop the line). A milestone is done when its numbered exit checks and the
   common exit checks C1-C7 of [§12.1](#121-overview) pass. Never skip, weaken or reinterpret an exit check; if one
   cannot pass as written, record why in the [progress log](#19-progress-log) and ask the user.
3. **Waiting.** A check that needs wall-clock time or an outside event (the M2 testnet soak, the M7 device run, the
   upstream release M9-05 needs) runs or waits in the background. Meanwhile, continue with the steps that do not depend
   on it, including the next milestone's. A milestone's tag (C7) is pushed only after all its other exit checks pass,
   including the soak or the bump being waited on. Only two things follow it: from M7 on, the device run, which runs on
   that tag and whose recording commit completes the milestone (C3); and at M9, the settings that M9-07 makes after the
   agent's last push.
4. **Progress log.** Append a line to [§19](#19-progress-log) for each milestone exit, each deviation from a step's
   text (what and why), each decision taken under rule 5, each upstream filing, each pin bump held back and what blocks
   it (C5), each `core-patch-conflict` resolved, and each wait on a prerequisite or on the user.
5. **Surprises.**
   - *Upstream changed* (a new release, metadata drift, a core API change): follow the pin-bump playbook
     ([§10.3](#103-a-pin-bump-step-by-step), [§10.6](#106-maintenance-playbook)); never hand-edit pins, vendored files
     or generated code.
   - *A Python behavior looks wrong*: keep parity unless the behavior is unsafe or provably wrong; in that case register
     a divergence in `parity/divergences.toml` with a pinning test ([§8.4](#84-registered-divergences)) and file the
     upstream issue (rule 7).
   - *A design gap* (the plan and architecture.md are silent or disagree): decide by architecture.md's principles
     (chain bytes from the core, behavior from Python, I/O owned by rtao, fail closed, parity enforced mechanically,
     Python only in CI), record the decision in the progress log, and update the section that owns the topic. A gap
     whose answer would change a user decision falls under rule 6.
   - *A step needs a later step*: do the minimum of the later step that unblocks it, and log the reordering.
6. **Stop and ask the user before** spending money (paid runners, hosts, services, hardware); anything that would hand
   secrets or keys to a third party (creating tokens or repository secrets included); changing any decision of
   [§3](#3-fixed-decisions-and-how-the-design-reads-them) or [§14.1](#141-decisions-answered-2026-10-13); filing an
   issue or PR outside the RaoFoundation monorepo; force-pushing, rewriting pushed history, or deleting a pushed branch
   or tag, in the rtao repository or in the fork (PR-U17's review branch and the `rtao-core-patch/*` branches); and
   anything that needs a prerequisite only the user can provide (rule 8), the Ledger device runs included. Upstream
   declining PR-U17 is also a stop-and-ask point: keep carrying the core patch, fall back to a test-only transcription,
   or build a uhid bridge ([§5.10](#510-the-pr-u17-core-patch)). While waiting for an answer, continue with the steps
   that do not need it.
7. **Pre-authorized** (no need to ask): committing and pushing to `main` once `cargo xtask gates` passes (rule 2);
   tagging each milestone exit (`rtao-v0.<k>.0`; `rtao-v1.0.0` for M9) or a fix within it (a patch tag,
   [§10.5](#105-compatibility-matrix-and-run-time-guard)) and pushing the tag, which runs `release.yml` (binaries, the
   GitHub release, rustdoc and the book as downloadable release assets); dispatching CI lanes (`gh workflow run`); creating the fork of the
   monorepo and filing each upstream PR and issue of [§15](#15-upstream-contributions) in the RaoFoundation monorepo as
   soon as its evidence exists, with the link recorded in §15's Filed column; maintaining PR-U17's review branch and the
   `rtao-core-patch/*` branches in the fork, never force-pushed ([§5.10](#510-the-pr-u17-core-patch)); following up the
   Ledger app issue and PR filed with `Zondax/ledger-polkadot` on 2026-10-13 (Q22): answering maintainer questions and
   pushing the changes they ask for to the PR's branch (`fix/tao-balance` in the user's fork
   `sam0x17/ledger-polkadot`), never force-pushed; running the local cold-reader security passes
   ([§12.1](#121-overview)).
8. **Prerequisites the user provides** (testnet funds and a soak host, GitHub/GHCR access, a Ledger device) and Q23's
   answers are listed in [§12.13](#1213-dependency-order-and-external-prerequisites); M0-01 asks for
   them, and each step that needs one names it. Q23 authorizes `sam0x17` and only free `ubuntu-latest` hosted jobs;
   docs ship as release assets, with no Pages environment. The goal loop needs no branch protection, merge queue or bot App: the
   rtao-bot App and, for PR-based upkeep, branch protection with the merge queue belong to the handover (M9-07).

## 1. Summary

### 1.1 What rtao is

rtao is a Rust SDK, operator framework and CLI for Bittensor subnets. It reproduces the behavior of the v11 Python
package (59,723 lines of Python; re-checked) and takes every chain-defined byte from the official Rust core,
`sdk/bittensor-core`, consumed as a git dependency pinned by tag. Subnet code depends on one facade crate, `rtao`, and
picks a role profile: `validator`, `miner`, `owner`, or `full` for the CLI.

### 1.2 The approach

- **A veneer over bittensor-core, with behavior from Python.** Keys, SCALE, metadata, extrinsic assembly, era math,
  the MEV-shield envelope, drand timelock and PoW come from the core through one seam crate, `rtao-core`. Product
  behavior (intents, executor, reads, receipts, retries, CLI) is ported from Python and checked against a CI-only
  Python oracle. The core's blocking `Client` and `Executor` serve only as test oracles
  ([architecture §0.2](docs/plan/architecture.md#02-the-architecture-in-one-page),
  [§1.1](docs/plan/architecture.md#11-principle-who-defines-the-bytes)).
- **rtao owns the async I/O.** A tokio-tungstenite JSON-RPC session reproduces `rpc.py`'s reconnect and refusal
  contract. On top sit a V15 runtime cache, pinned snapshots, coalesced reads, a submission pipeline with RAII nonce
  leases, the MEV-shield path and gap-free head streams
  ([architecture §3](docs/plan/architecture.md#3-chain-client-rtao-rpc-rtao-client-rtao-runtime)).
- **Generated where upstream publishes data.** Intent and read interfaces, dispatch enums, error tables and literal
  tables come from the vendored catalogs and from Python and Rust sources through Rust-native extraction. Chain
  descriptors come from pinned V15 metadata and are verified against each live runtime by shape hashes
  ([architecture §3.7](docs/plan/architecture.md#37-generated-chain-descriptors-shape-hashes-surface-verification-rtao-runtime),
  [§4.1](docs/plan/architecture.md#41-where-each-piece-comes-from)).
- **An operator framework that v11 lacks.** `rtao-neuron` provides validator and miner runtimes, weight scheduling with
  reveal watching, idempotent axon publishing, metagraph sync, btauth/1, metrics, health and safety rails
  ([architecture §9](docs/plan/architecture.md#9-framework-for-owners-validators-and-miners-rtao-neuron)).
- **Parity is a CI gate, not a promise.** A blob-SHA file map, a Python symbol diff, catalog equality, an extraction
  check, a coverage gate, oracle vectors, a shrink-only `pending.toml` and a divergence register run in `pr-gates` on
  every push, offline ([architecture §4.9](docs/plan/architecture.md#49-parity-enforcement-the-machinery)).

### 1.3 Crates at a glance

19 crates plus `xtask` ([architecture §2.2](docs/plan/architecture.md#22-crates); the full table is in
[§4.3](#43-crates)):

| Layer | Crates |
|---|---|
| seam to the core | `rtao-core-ext` (code staged for upstream), `rtao-core` (the only importer of `bittensor_core`) |
| vocabulary and generated data | `rtao-primitives`, `rtao-catalog`, `rtao-runtime` |
| I/O and keys | `rtao-rpc`, `rtao-signer`, `rtao-wallet`, `rtao-client` |
| product layer | `rtao-tx`, `rtao-intents`, `rtao-reads`, `rtao-evm` |
| metagraph, networking, framework | `rtao-metagraph`, `rtao-btauth`, `rtao-neuron` |
| entry points and tooling | `rtao` (facade), `rtao-cli` (binary `rtao`), `rtao-testkit`, `xtask` |

### 1.4 Delivery order

Delivery is operator-first, and every milestone ends with a release ([§12](#12-milestones)). The plan states order,
dependencies and objective exit checks, not durations ([§14.1](#141-decisions-answered-2026-10-13)):

| Capability | Milestone | Depends on |
|---|---|---|
| weight-setting validator, no Perl/OpenSSL/libsodium | M2 | M0, M1 |
| btauth/1 miner, subnet template | M3 | M2 |
| owner tooling, MEV shield, CLI v0 | M4 | M2 |
| full v11 parity (M8), `rtao-v1.0.0` (M9) | M8, M9 | M5, M6, M7 |

Upstream tracking (pin bumps, re-ports of changed files, new ops and reads) runs from M2 to the end of the project:
during the goal loop the agent performs each pin bump itself, and the `pin-bump` bot takes over at the handover, M9-07
([§12.1](#121-overview)).

### 1.5 The five things that matter most

1. **Bytes from the core, behavior from Python.** rtao re-implements no byte format. Upstream golden vectors are replayed
   through rtao's own wrappers, so the seam is tested too. The pinned Python SDK is the executable specification of
   behavior, run as an oracle in CI ([§4.1](#41-who-defines-the-bytes)).
2. **Two pins and one mandatory patch line.** `parity_tag` follows every finalized release; `core_tag` moves only when
   the core's inputs change. Every subnet workspace must repeat `[patch.crates-io] w3f-bls` at `core_tag` until
   upstream accepts PR-U4; `rtao doctor` prints the exact line ([§5.5](#55-two-pins-and-upstreamtoml),
   [§5.6](#56-the-w3f-bls-patch-line)).
3. **Parity is enforced mechanically, starting in M0**, and Python stays in CI. A new or changed upstream file, symbol, catalog
   entry, chain call or behavior fails `pr-gates` until it is ported, blessed or registered as a divergence
   ([§8.3](#83-how-parity-is-enforced)).
4. **The weight path cannot lose commits silently.** A pallet-faithful epoch predicate over a schedule read at the best
   head (as Python reads it), `commit_reveal_version = 4` with an early refusal, `round_ties_even` quantization pinned
   by oracle vectors, Python's 128-block era anchored at the finalized head, RAII nonce leases with reconciliation, a
   clock-skew guard, and a reveal watcher that follows the commit's actual reveal epoch and attributes misses
   ([architecture §9.3](docs/plan/architecture.md#93-weightscheduler-the-validators-critical-path)).
5. **Operator-first delivery with libsodium- and OpenSSL-free neuron builds.** Validator, then miner, then owner.
   Neuron binaries build without `bittensor-core/host`, so their images need no libsodium, OpenSSL, `secp256k1-sys`,
   Perl or CMake. The only native code they compile is `ring` (TLS) and `blake3`'s SIMD kernels, built by the C
   compiler that rustc already uses as its linker, so they build on the stock `rust:1.89-slim` image
   ([§5.7](#57-the-no-host-neuron-profile)).

### 1.6 Decided by the user (2026-10-13)

The user's answers to the plan's open questions are final; [§14.1](#141-decisions-answered-2026-10-13) quotes each one
with its consequences. rtao is dual-licensed `MIT OR Apache-2.0`; Python runs only in CI lanes; the binary is `rtao`,
with no `btcli` shim; btauth/1 verification accepts plain and `<Bytes>`-wrapped signatures by default; the repository is
`github.com/sam0x17/rtao`; a manual Ledger device run per release stays the final release check, and an automated Ledger
emulator lane (Speculos) covers the rest in CI; until PR-U17 ships, rtao's own workspace builds `bittensor-core` through
a Cargo patch that adds it ([§5.10](#510-the-pr-u17-core-patch)); each upstream PR and issue is filed as soon as its
evidence exists, and the Ledger app fix was filed with Zondax on 2026-10-13, before the plan runs; the per-milestone
security passes are done by local cold readers; the agent develops M0 to M9 as one goal loop, committing to `main`
without PRs ([Executing this plan](#executing-this-plan)); and the plan carries no time estimates, because an AI coding
agent executes it step by step.

## 2. Goals and how the design delivers them

The user's design goals, mapped to the mechanisms that deliver them. "Fast" is read as latency on hot paths and "highly
performant" as throughput, resource use and build cost at subnet scale [I: the plan's reading of two overlapping goals].

| Goal | Mechanisms | Depth |
|---|---|---|
| **100% Rust, no Python** | every crate, binary and build step is Rust; chain bytes come from the Rust core; product behavior is ported to Rust; extraction and regeneration use `ruff_python_parser` and `syn` inside `xtask`; the `rtao` CLI replaces `btcli` and shares its files. Python runs only in CI lanes (oracle, interop, upstream recorders, and the Ledger emulator, whose Speculos runs Python inside its image), never in a build, in regeneration or in `pr-gates`; of these lanes only interop is blocking, and only on pin bumps ([§9.7](#97-ci-lanes-and-what-each-catches)); the user accepted this reading (Q2, [§14.1](#141-decisions-answered-2026-10-13)) | [architecture §0.3 #23](docs/plan/architecture.md#03-twenty-five-decisions-that-define-the-design), [§11.5](docs/plan/architecture.md#115-the-python-oracle-ci-only-outputs-committed), [§17 #13](docs/plan/architecture.md#17-what-we-deliberately-do-not-do-yet) |
| **Fast** (hot-path latency) | metagraph decoded by a borrowed, shape-gated `MetagraphView`: 6.5 µs at 256 UIDs against 330 µs for the core `Value` decoder and 1,380 µs for `DecodeAsType`, and 57 µs at 2,500 UIDs; signing inline (about 0.03 ms); `Runtime::parse` about 1 ms once per spec, off the reactor; a weights preflight is one `state_queryStorageAt` instead of about 18 RPCs; O(1) `by_hotkey` | [architecture §8.2](docs/plan/architecture.md#82-the-metagraph-three-decode-paths-one-projection), [§8.6](docs/plan/architecture.md#86-performance-claims-corrected), [§3.6](docs/plan/architecture.md#36-reading-state), [bench-run-3.txt](docs/plan/bench/decode-bench/bench-run-3.txt) |
| **tokio / async** | own tokio-tungstenite session behind `subxt_rpcs::client::RpcClientT`, with TLS from one explicitly built `ring` rustls config (never the process default); return-position `impl Future + Send` on generic paths and boxed futures only on `dyn` seams; Argon2i unlock, Ledger HID, PoW, metadata parse, any decode above 64 KiB on every path, timelock encryption, the RFC-0078 digest (once per spec) and fsync'd file writes go through `spawn_blocking`; dropping the last `Client` clone cancels its tasks and closes the socket; async `NonceStore`; axum, tower and reqwest adapters; `CancellationToken` supervision; the core's blocking reqwest client is banned from runtime paths by `clippy` | [architecture §3.1](docs/plan/architecture.md#31-async-model), [§3.2](docs/plan/architecture.md#32-transport-rtao-rpcsession), [§1.8](docs/plan/architecture.md#18-deliberately-not-used-on-runtime-paths) |
| **Highly performant** (scale, resources, build) | coalesced reads and a client token bucket for public endpoints that allow about 1 request per second; an `ArcSwap` metagraph handle with no per-request decode; sparse weight and bond rows (a dense 2,500-UID matrix would be 6.25M cells); selective metagraph presets; no-host neuron builds (the core without `host` locks 241 packages and builds in 12.9 s; with `host` it pulls 330 crates, and its vendored OpenSSL needs Perl and make); a cold build first fetches about 293 MB (subtensor) and 480 MB (polkadot-sdk) of git history, which the template's BuildKit cache mounts pay once; criterion gates fail on regressions above 30% | [architecture §3.2.5](docs/plan/architecture.md#325-rate-limits-rtao-addition-and-request-volume), [§8.3](docs/plan/architecture.md#83-sync-service-c), [§8.4](docs/plan/architecture.md#84-miner-admission), [§2.8](docs/plan/architecture.md#28-build-weight), [§11.7](docs/plan/architecture.md#117-property-tests-fuzzing-benchmarks) |
| **Correct + tested** | upstream goldens replayed through rtao's wrappers; Python oracle vectors for composed calls, plans, reads and pure functions (u16 weight rows bit for bit, 10,000 `str(Balance)` vectors); a socket-level `FakeNode` that ports `test_rpc.py`; 19 rtao localnet scenarios, with the 112 upstream e2e manifest names and the 3 `PYT/e2e` tests mapped (115 keys); Python interop lanes; proptest and fuzzing; type-level safety (`KeySource<Role>`, `Balance<Tao>` vs `Balance<Alpha>`, `NonceLease`, `ShieldPeriod`, spec-stamped `CallBytes`, `ReadMethod`); fail-closed per-call surface verification | [§9](#9-testing-strategy), [architecture §11](docs/plan/architecture.md#11-testing) |
| **Well-documented** | `deny(missing_docs)`; every example is a compiled `docify` item; an mdBook that mirrors bittensor.com's taxonomy; generated reference pages per op, read, error and hyperparameter; `#[doc(alias = "bittensor.…")]` on every mirrored item; a v11 → rtao migration table generated from `parity/map.toml`; `llms.txt` | [§11](#11-documentation-plan), [architecture §13](docs/plan/architecture.md#13-documentation) |
| **Easy to maintain** | rtao owns only I/O, generators, ported behavior and staged upstream code (about 14k generated, 13k I/O, 27k ported, 7k framework, 8k tooling and 2k staged lines [I]); one seam crate absorbs core API changes; ported modules mirror Python file for file; provenance by git blob SHA in `parity/map.toml`; layering enforced by `xtask deps-check`; `clippy` `disallowed-methods` | [architecture §0.4](docs/plan/architecture.md#04-what-rtao-owns-budget), [§2.3](docs/plan/architecture.md#23-dependency-layers-and-enforced-rules) |
| **Modular** | 19 crates in nine layers with enforced dependency rules; facade features and role profiles; one feature per Python intent group; the btauth/1 core has no I/O; signers, EVM, PoW GPU and the subxt escape hatch sit behind features | [§4.2](#42-layers), [§4.4](#44-facade-features-and-role-profiles) |
| **Easy to keep up to date** with Python and subtensor | two pins in `upstream.toml`; a pin-bump playbook, run by the agent during the goal loop and by a bot every 30 minutes after it; a metadata-drift diff with four classes (`surface-unchanged` regenerates zero lines; after the loop a 6-hourly job auto-merges it); auto-bless for cosmetic Python edits; a nightly `upstream-next` diff against monorepo `main` and `testnet`; a nightly `core-canary`; a compile probe naming every core symbol rtao uses; forced deletion of upstreamed staged code; a compatibility matrix and run-time guard; a maintenance playbook | [§10](#10-keeping-up-with-upstream), [architecture §12](docs/plan/architecture.md#12-upstream-tracking) |

## 3. Fixed decisions and how the design reads them

The user's decisions are final. The design interprets them as follows.

| User decision (quoted) | How the design reads it | Consequences | Depth |
|---|---|---|---|
| "don't worry about crates.io, we just need to build something that works" | Publishability is not a goal. Releases are git tags `rtao-vX.Y.Z` with a generated `compat.json`: during the goal loop the agent tags each milestone exit (C7), and after it release-plz cuts them, its `cargo-semver-checks` run picking the bump (the check is informational in `pr-gates` before 1.0, blocking from 1.0) | git and `[patch]` dependencies are acceptable; `release.yml` attaches rustdoc and the book as downloadable docs assets (Q23) | [architecture §12.8](docs/plan/architecture.md#128-release-process), [§17 #12](docs/plan/architecture.md#17-what-we-deliberately-do-not-do-yet) |
| "git dependencies are fine" | `bittensor-core` is pinned by git tag; every final workspace repeats `[patch.crates-io] w3f-bls` at the same tag; `xtask`-only git dependencies (`ruff_python_parser`, `subtensor-linting`) are allowed | subnet repositories carry one patch line until PR-U4 lands; the template and `rtao doctor` keep it right | [architecture §1.2](docs/plan/architecture.md#12-pinning-core_tag-and-parity_tag), [§1.3](docs/plan/architecture.md#13-the-w3f-bls-patch-end-to-end) |
| "probably we can build what we need as a veneer over bittensor-core if that now exists" | It exists: `sdk/bittensor-core`, version 0.1.0, `publish = false`, edition 2021, Apache-2.0 (`UP/sdk/bittensor-core/Cargo.toml:1-7`, re-checked). "Veneer" means every chain-defined and compatibility byte comes from the core, while product behavior comes from Python, because the core's product layer (blocking `Client`, `Executor`, `Policy`) diverges from Python on era, anchor, shield era, fee RPC, receipts, error codes and intent coverage | one seam crate; a staging crate for upstream-bound code; the core client used as a test oracle only | [architecture §1](docs/plan/architecture.md#1-boundary-with-bittensor-core), [§5](#5-bittensor-core-taken-wrapped-staged-avoided) |
| scope = "matching everything that is today in python" | Full parity with the v11 package at `parity_tag`: 83 intents, 102 reads, 18 error codes and 390 classified chain errors, transport semantics, wallets, keyfiles and passwords, signers (Ledger, Vault, browser extension), EVM helpers, timelock, btauth/1, the metagraph, and the CLI (163 hand-written commands plus generated `tx`/`query`). JSON output is value-equal to Python's. Phased but complete; the parity gate turns strict at M8 | parity machinery starts in M0 and is complete in M2; full parity at M8, `rtao-v1.0.0` at M9 ([§12](#12-milestones)) | [§8](#8-python-parity), [architecture §4](docs/plan/architecture.md#4-intents-and-reads-parity) |

**The reading of "100% Rust".** What subnet owners build and run is Rust only: rtao's crates, every subnet binary, the
`rtao` CLI, extraction and regeneration (`xtask`), and the offline PR gates. Python appears in four CI-only lanes: the
oracle that runs the pinned Python SDK to produce committed vectors, the interop lane that runs `btcli` and Python
neurons against rtao, upstream's own fixture recorders, and the Ledger emulator lane, whose Speculos (Ledger's device
emulator) is a Python program inside its own image ([§9.5](#95-python-oracle-and-interop)). A failing Python toolchain
can delay a pin bump (`oracle-stale`, `ledger-stale`) but cannot block a build. The user accepted this reading
on 2026-10-13 ("python ok for CI", Q2).

The user's answers of 2026-10-13 to the plan's open questions (license, Python in CI, binary name, btauth/1 default,
repository, Ledger in CI, upstream filing, security passes, time estimates, the PR-U17 core patch, one goal loop
without PRs, the Ledger app filing) are equally final;
[§14.1](#141-decisions-answered-2026-10-13) records each answer and its consequences.

## 4. Architecture overview

### 4.1 Who defines the bytes

`bittensor-core` sorts its own modules into CHAIN (bytes the chain defines), COMPAT (bytes an external ecosystem
defines) and PRODUCT (`client.rs`, `transaction.rs`). rtao's boundary follows that split
([architecture §1.1](docs/plan/architecture.md#11-principle-who-defines-the-bytes)):

| Kind | Examples | Defined by | rtao takes it from | Checked by |
|---|---|---|---|---|
| CHAIN bytes | keys and ss58, SCALE codec, metadata model, signature payload in metadata extension order, v4 extrinsic assembly, era math, multisig account ids, RFC-0078 digest and Ledger proof, ML-KEM shield envelope, drand tlock, PoW seal | the runtime | `bittensor-core`, through `rtao-core` | upstream golden vectors, replayed through rtao's wrappers |
| COMPAT bytes | btwallet keyfiles ($NACL, Ansible Vault, legacy Fernet), polkadot-js JSON, Ledger APDUs | btwallet, polkadot-js, Ledger | `bittensor-core` (the plaintext JSON reader is staged so neurons avoid `host`) | keyfile vectors, a staged-vs-core differential test, the interop wallets lane |
| PRODUCT behavior | intent `build`/`preflight`/effects, executor, plan and policy, nonce semantics, era defaults, fee probe, receipts, retries, reads, results and error classification, CLI | Python v11 at `parity_tag` | ported to Rust, file for file | Python oracle vectors and interop lanes |
| behavior nobody upstream provides | async transport, rate limiting, metagraph view and sync, operator framework, persistence | rtao | rtao | `FakeNode`, `FakeChain`, localnet e2e, benchmarks |
| chain-adjacent logic missing from the core | sans-IO protocol functions, Python receipt semantics, metadata IR, pallet-faithful epoch schedule, plaintext keyfile reader | should live in the core | `rtao-core-ext`, one module per upstream PR, deleted when the core exports it | `xtask staged --check`, upstream vectors |

### 4.2 Layers

Dependency layers, enforced by `cargo xtask deps-check` in `pr-gates`
([architecture §2.3](docs/plan/architecture.md#23-dependency-layers-and-enforced-rules)):

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

Rules checked by `deps-check`:

1. Only `rtao-core`, `rtao-core-ext`, `rtao-testkit` and `xtask` depend on `bittensor-core`.
2. Only `rtao-rpc`, `rtao-testkit` and `rtao-signer` under feature `extension` or `vault` (the extension bridge's
   client, and the WebSocket upgrades of the bridge server and of the Vault page server) depend on `tokio-tungstenite`,
   all at the single workspace version. axum's own `ws` feature is not used: axum 0.8.9 requires
   `tokio-tungstenite ^0.29`, a second `tungstenite` that `deny.toml` bans. Only `rtao-rpc`, `rtao-client` and
   `rtao-testkit` (`FakeChain`) depend on `subxt-rpcs`. `jsonrpsee` and `subxt` are absent from the default graph
   (facade feature `subxt` only), and `jsonrpsee`, `subxt`, `rtao-signer/extension` and `rtao-signer/vault` are absent
   from the no-host profiles.
3. `rtao-catalog` and every `generated/` module are 100% generated: `xtask regen --check` must be a no-op.
4. `rtao-btauth` without features has no tokio, axum, reqwest or hyper. Every intra-workspace dependency is declared
   `default-features = false` and enables exactly what it uses, so a feature turned off in the facade stays off.
5. The no-host profiles (`validator,prometheus` and `miner,redis`, each with `--no-default-features`) contain, in
   `cargo tree -e normal,build`, none of the crates listed in [§5.7](#57-the-no-host-neuron-profile), no blocking
   `reqwest`, and `sp-core` without `std`.
6. `Cargo.lock` holds exactly one `w3f-bls`, at `core_tag`.
7. Exactly one rustls crypto provider, `ring`: `aws-lc-rs`, `aws-lc-sys` and `native-tls` are absent from every
   profile, and `deny.toml` bans them.
8. Test seams stay out of shipped graphs: `rtao-signer/test-util` and `rtao-client/test-util` are enabled only by
   `rtao-testkit` and dev-dependencies, never by a facade feature or by `rtao-cli`.

PR-U17's items, which only the core patch provides, stay out of shipped crates through `clippy.toml` and the
`core-unpatched` job rather than `deps-check` ([§5.10](#510-the-pr-u17-core-patch)).

### 4.3 Crates

"First milestone" is where the crate first ships code, from
[architecture §14](docs/plan/architecture.md#14-milestones) and the
[parity inventory](docs/plan/parity-inventory.md#1-summary-by-milestone).

| Crate | Purpose | Key public types | Features | First milestone |
|---|---|---|---|---|
| `rtao-core-ext` | code written as if already upstream; no tokio | `ops::{metadata, storage, runtime_api, extrinsic_lookup, receipt, dispatch_error, fee}`, `ir::MetadataIr`, `epoch_schedule`, `timelock_commit`, `keyfile_json`, `keyfile_env`, `verify_strict`, `value_ext::ValueExt` | none | M0 (skeleton) |
| `rtao-core` | the seam: curated re-exports, `CoreError` mapping, `offload`, compile probe, lock test | `DynValue`, `offload`, `BITTENSOR_CORE_TAG`, `BITTENSOR_CORE_COMMIT`, re-exported `keys`, `codec`, `runtime`, `extrinsic`, `digest`, `mlkem`, `timelock`, `epoch`, `keyfile_json` | `host`, `ledger`, `offload` (default) | M0 |
| `rtao-primitives` | vocabulary: ids, money, fixed point, Python formatting | `AccountId`, `DefaultAccount`, `NetUid`, `MechId`, `Uid`, `NetUidStorageIndex`, `TaoBalance`, `AlphaBalance`, `Balance<Tao>`, `Balance<Alpha>`, `DynBalance`, `Money`, `Amount`, `ResolvedAmount`, `Spend`, `PriceRao`, `StakeWeight`, `PerU16`, `U64F64`, `I96F32`, `Ratio`, `IntoCoreValue`, `pyfmt`, `ratio::ratio_fraction`, `hyperparams::{Hyperparam, HyperparamTable, proportion_to_raw}`, `drand::{current_round, round_at, reveal_time, …}`, `axon::{AxonTarget, axon_base_url}`, `json::{from_slice, from_str}` (Value-first parsing, [§5.8](#58-known-core-defects-and-how-rtao-handles-them) #10) | `serde` (default), `schemars` | M0 (later modules land with their first users: `ratio`, `drand` and `proportion_to_raw` in M1, `axon` in M2, `HyperparamTable` in M4) |
| `rtao-catalog` | generated data only | `INTENTS: [IntentSpec; 83]`, `READS: [ReadSpec; 102]`, `ErrorCode`, `CHAIN_ERRORS`, `py::*`, `rt::*` (incl. `MAX_SHIELD_ERA_PERIOD`, `rate_limits`, chain rules), `hyperparams::TABLE` | none | M1 (settings tables), M2 (catalogs) |
| `rtao-runtime` | generated chain surface plus hand models | `storage::*`, `calls::*` (typed builders, `CallDescriptor`), `runtime_apis::*`, `constants::*`, `events::*`, `errors::MODULE_ERRORS`, `extensions::EXPECTED`, `models::*`, `ShapeHash`, `verify_surface`, `SurfaceReport`, `RuntimeHandle`, `RuntimeEntry`, `MetadataLevel`, `CallBytes`, `ComposeError` | none | M1 |
| `rtao-rpc` | tokio-tungstenite JSON-RPC session | `Session`, `SessionConfig`, `EndpointPool`, `ReadMethod`, `RpcError`, `PolicyRefusal`, `SubmitError`, `SessionState`, `RateLimit`; `impl RpcClientT for Session` | `rate-limit` (default), `metrics` | M1 |
| `rtao-signer` | signing seam and roles | `Signer`, `SignerCaps`, `SigningRequest`, `MultiSignature`, `Role`, `Coldkey`, `Hotkey`, `KeySource<R>`, `DynKeySource`, `KeypairSigner`, `LedgerSigner`, `VaultSigner`, `ExtensionSigner`, `extension::{BridgeServer, BridgeClient, connect_extension_signer, ensure_bridge, run_bridge, stop_bridge_daemon}`, `vault::{VaultSessionServer, VaultError, VaultPageError, scan_vault_address}`, `RemoteSigner` (M10) | `ledger`, `vault` (UOS framing, the one-shot page server on axum with a hyper WebSocket upgrade, QR rendering), `extension` (bridge client on tokio-tungstenite, bridge server on axum with a hyper upgrade), `remote` (M10), `test-util` (the `LedgerBackend` seam for `rtao-testkit`) | M0 (`Signer` and `KeypairSigner`, needed by btauth/1 vectors); M2 roles; M7 Ledger, Vault and the extension bridge; M10 `RemoteSigner` |
| `rtao-wallet` | btcli-compatible wallets and passwords | `Wallet`, `HotkeyFile`, `ColdkeyFile`, `PasswordSources`, `PromptPolicy`, `resolve_wallet_password` | `encrypted` (enables core `host`), `keychain` (macOS, M4), `keyring`, `prompt` | M2 |
| `rtao-client` | chain access | `Client` (`connect`, `from_rpc`, `decode_scale`, `estimate_weight`, `estimate_shielded_carrier_fee`), `ClientConfig`, `ClientError`, `HeadPolicy`, `Snapshot`, `View`, `DynView`, `RuntimeCache` (and re-exports of `rtao_runtime::{RuntimeHandle, MetadataLevel, CallBytes}`), `ReadPlan`, `HeadTracker`, `BlockHeader`, `BlockInfo`, `EpochEvent`, `SubmitOptions`, `SubmitFailure`, `NonceManager`, `NonceLease`, `ShieldPeriod`, `ShieldCiphertext`, `PoolRejection`, `UnsignedExtrinsic`, `compose::*`, `config` (btcli files and caches), `EpochClock`, `blocking::BlockingRuntime` | `blocking` (the runtime thread only; the facades are in `rtao::blocking`), `subxt`, `metrics`, `test-util` | M1 |
| `rtao-tx` | product layer for writes | `Intent`, `IntentInterface`, `DynIntent`, `Executor`, `Plan`, `Policy`, `ExecuteOptions`, `ExtrinsicResult<O>`, `ChainError`, `PolicyError`, `TxError`, `classify`, `Spend` (re-export), `multisig::{Multisig, ApproveOptions}`, `timelock::{seal, open, Timelocked, Reveal, OpenOptions, TimelockError, DrandClient}` | none | M2 (multisig and the raw path: M4; timelock UX: M7) |
| `rtao-intents` | the 83 intents | one struct per op (`AddStake`, `SetWeights`, …), `AnyIntent`, `weights::{conform, TimelockCommitter}`, `pow::PowSolver`, `hyperparameters::{Hyperparameter, to_raw}` (`to_raw` delegates to `rtao_catalog::hyperparams::TABLE`), `MultisigAdapter`, `MultisigThreshold1Adapter` | one per Python group (20), `all`, `pow`, `pow-gpu` | M2 |
| `rtao-reads` | the 102 reads | `ReadName`, `ReadSpec`, `ParamSpec`, `dispatch`, `ReadError`, 14 namespace types, read output models, `commitments::{NeuronCommitment, CommitmentMap, fetch_commitments, fetch_commitment_map}` | none | M1 (`metagraph`, `token_symbols`, the `commitments` module, the `Subnets` accessor) |
| `rtao-metagraph` | metagraph model, view and sync | `Metagraph`, `MetagraphNeuron`, `MetagraphView`, `NeuronRef`, `SubnetsMetagraphExt` (`metagraph`, `metagraph_with`), `MetagraphOptions`, `MetagraphSync`, `MetagraphHandle`, `MetagraphDiff`, `SyncPolicy`, `persist` | `persist` | M1 (model, view, `SubnetsMetagraphExt`); M2 (`MetagraphSync` validator preset); M3 (miner preset, event-triggered refresh, `persist`) |
| `rtao-btauth` | btauth/1 signed HTTP | `build_payload`, `sign`, `Verifier` (`authenticate`, `Authenticated::commit`, `verify`), `VerifyConfig`, `AuthError`, `NonceStore`, bounded `InMemoryNonceStore`; `server::{BtAuthLayer, VerifiedCaller, MetagraphPolicy, PerHotkeyLimiter, Json}`; `client::{SignedClient, FanOut}` (`fan_out`, `fan_out_stream`); `RedisNonceStore`; `legacy::verify_v10` | `server`, `client`, `redis`, `metagraph-policy`, `legacy-v10` | M0 (core), M2 (client adapter), M3 (server adapter) |
| `rtao-neuron` | operator framework | `NeuronConfig`, `NeuronRuntime`, `NeuronCx`, `StateCell`, `Validator`, `Miner`, `StepPolicy`, `WeightsProposal`, `WeightScheduler`, `AxonPublisher`, `OwnerOps`, `HyperparamPlanner`, `OwnerMonitor`, `StateStore`, `EmaScores`, `anyhow` (re-export) | `validator`, `miner`, `owner`, `prometheus` (default), `auto-update` (M9) | M2 (validator), M3 (miner), M4 (owner) |
| `rtao-evm` | `PY/evm/*` parity | `EvmKeystore`, `Precompiles`, `EvmRpc`, `association_proof` | none | M7 (the `h160_to_ss58` helper lives in `rtao-primitives` for M5 intents) |
| `rtao-cli` | the `rtao` binary, btcli-equivalent, with no `btcli` shim (Q3); its `[[bin]]` sets `doc = false` ([§11](#11-documentation-plan)) | — | `ledger`, `vault`, `extension`, `evm` | M0 (`doctor` lock check only); M4 CLI v0 |
| `rtao-testkit` | test infrastructure | `FakeNode`, `FakeChain`, `FakeState`, `Fault`, `RecordingView`, `Localnet`, `Scenario`, `PinProxy`, `fixtures`, `oracle`, `ledger_emu` (the Speculos harness, with the Speculos TCP and replay transports that `LedgerDevice::with_transport` takes from the core patch, [§5.10](#510-the-pr-u17-core-patch)) | `docker`, `interop`, `core-oracle`, `ledger-emu` (each gates its own `[[test]]` target) | M0 (`fixtures`), M1 (fakes), M2 (`Localnet`, `Scenario`), M4 (`PinProxy`), M7 (`ledger_emu`) |
| `rtao` | the facade subnet code depends on | `prelude` (contents per feature, [§4.4](#44-facade-features-and-role-profiles)), re-exports per feature, `blocking::{SyncClient, SyncSnapshot, Subtensor, set_weights, close_shared_clients}` (`SyncClient` has all 14 namespaces, D-027) | [§4.4](#44-facade-features-and-role-profiles) | M0 (`blocking`: M6) |
| `xtask` | pin, import-upstream, extract, regen, parity, staged, core-surface, deps-check, drift, oracle, record, docs, compat, localnet (bootstrap, run, smoke, up/down), localnet-image, probe, release-notes, pr-u4-proof, fuzz, bench, bench-live, mainnet-clone, template (check), upstream (parity-tag), ci (issue), lint-upstream, ledger-emu (build-app, up, record, speculos-image), gates (the local `pr-gates`, M0-27), core-patch (rebase, strip; M7-09) (the subcommands Appendix E and the executing rules call) | — | — | M0 |

Two splits are deliberate: `rtao-runtime` is separate from `rtao-catalog` (a runtime bump and a Python bump recompile
only their own dependents), and `rtao-rpc` is separate from `rtao-client` (the transport has no chain knowledge and is
tested alone on `FakeNode`).

### 4.4 Facade features and role profiles

The facade `rtao` maps roles to features ([architecture §2.4](docs/plan/architecture.md#24-facade-features-and-role-profiles)):

| Profile | Facade features | Builds `bittensor-core/host`? | Typical binary |
|---|---|---|---|
| validator | `validator` (+ `prometheus`) | no | weight-setting validator, hotkey only |
| miner | `miner` (+ `redis`) | no | axum miner behind btauth/1 |
| owner | `owner` (+ `ledger`) | yes (encrypted coldkeys) | owner tooling, coldkey operations |
| CLI and agents | `full` | yes | the `rtao` binary, agents using `execute_tool` |

Other features: `client`, `reads`, `tx` (the library default), `intents`, `metagraph`, `wallet`, `wallet-encrypted`,
`btauth`, `btauth-server`, `btauth-client`, `redis`, `neuron`, `intents-all`, `pow`, `pow-gpu`, `ledger`, `vault`,
`extension`, `remote-signer` (M10; outside `full` until then), `keychain`, `evm`, `blocking`, `prometheus`, `subxt`.
`AnyIntent` variants are gated per Python intent group, so neuron builds compile only the intents they use. Three
definitions matter for the profiles: `intents = ["tx", "reads", "dep:rtao-intents"]`, `neuron = ["intents",
"metagraph", "wallet", "btauth-client", "dep:rtao-neuron"]`, and `blocking = ["intents-all", "metagraph", "wallet",
"rtao-client/blocking"]`.

With `dep:` syntax there is no implicit feature to `cfg` on, so the facade gates its modules and prelude items on its
own features:

| Feature | Module | Prelude additions |
|---|---|---|
| always | `rtao::{core, primitives}` | `NetUid`, `MechId`, `Uid`, `AccountId`, `Balance`, `Tao`, `Alpha` |
| `client` | `rtao::client` | `Client`, `ClientConfig`, `View`, `ViewExt`, `Snapshot`, `BlockRef` |
| `reads` | `rtao::reads` | — |
| `tx` | `rtao::{tx, signer}` | `Executor`, `ExecuteOptions`, `PlanOptions`, `Policy`, `ExtrinsicResult`, `TxError` |
| `intents` | `rtao::intents` | `AnyIntent` |
| `metagraph` | `rtao::metagraph` | `SubnetsMetagraphExt` (so `client.subnets().metagraph(netuid)` resolves) |
| `wallet` | `rtao::wallet` | `Wallet`, `HotkeyFile` |
| `btauth` | `rtao::btauth` | — |
| `neuron` | `rtao::neuron` | `NeuronConfig`, `NeuronRuntime`, `NeuronCx`, `Validator`, `Miner`, `StateCell`, `WeightsProposal` |
| `evm` | `rtao::evm` | — |
| `blocking` | `rtao::blocking` | — |

`pr-gates` compiles the three examples of [§7](#7-what-a-subnet-builder-writes) as
`crates/rtao/examples/{validator,miner,owner}.rs`, each with `required-features` set to its profile, so a profile that
stops exposing what its example uses fails the build. Each example, and its line in the `features` job, lands with its
profile: `validator` in M2, `miner` in M3, `owner` in M4.

Feature unification caveat: Cargo unifies features across the packages selected for one build, so a subnet workspace
that also contains an owner tool must build neuron binaries with `cargo build -p <binary>` to keep `host` out. The
template's Dockerfile does exactly that [I] ([architecture §1.7](docs/plan/architecture.md#17-host-and-the-no-host-neuron-profile)).

### 4.5 Generated, ported, owned

Scale estimates from [architecture §0.4](docs/plan/architecture.md#04-what-rtao-owns-budget) [I]:

| Kind | Where | Rust lines [I] | Kept correct by |
|---|---|---:|---|
| reused unchanged | `bittensor-core` (about 13.6k lines with tests) | 0 | upstream |
| staged for upstream | `rtao-core-ext` | about 2k | upstream style and lints; deleted as PRs merge |
| generated | `rtao-catalog`, `rtao-runtime/generated`, `rtao-intents/generated`, `rtao-reads/generated`, docs | about 14k | diff review only (`regen --check`) |
| I/O and runtime | `rtao-rpc`, `rtao-client`, `rtao-signer`, `rtao-wallet`, `rtao-btauth` | about 13k | rtao, on `FakeNode` and localnet |
| ported behavior | intent bodies, executor, read bodies, CLI groups | about 27k | the Python oracle at `parity_tag` |
| framework | `rtao-neuron`, `rtao-metagraph` | about 7k | rtao (v11 ships no framework) |
| tooling | `xtask`, `rtao-testkit` | about 8k | rtao |

Across the Python package, the [module map](docs/plan/python-module-map.md#scope-and-totals) classifies the 220 files
as 143 `ported`, 6 `extracted`, 24 `vendored`, 29 `generated` and 18 `n/a`; with the 137 other watched upstream files it
seeds 357 `parity/map.toml` rows.

## 5. bittensor-core: taken, wrapped, staged, avoided

### 5.1 Used as-is

Definition sites are in `CORE/` at `850f2755` ([architecture §1.4](docs/plan/architecture.md#14-used-as-is)):

| Area | Items | Behind `host`? | rtao consumer |
|---|---|---|---|
| metadata model | `Runtime::parse` (V14/V15; V16 rejected), `pallet`, `storage_entry`, `constant`, `type_id_of`, `resolve`, `module_error`; `types: PortableRegistry` | no | `rtao_runtime::RuntimeHandle` (cached by `rtao_client::RuntimeCache`), surface verification, codegen |
| encode | `Runtime::compose_call` (the single encoder), `encode_id`, `encode_era_value`, `coerce_account_id` | no | every call, storage key argument and runtime-API argument |
| decode | `decode_id`, `decode_call_value`, `batch::{decode_batch, decode_map_page, decode_map_changes}`, `Value`, `to_corpus_json` | no (rayon only with `host`) | dynamic reads, receipts, CLI and agent JSON |
| storage keys | `Runtime::storage_key`, `decode_storage_key_params`, hashers | no | every storage key, always from the live entry |
| extrinsics | `TxParams`, `signature_payload_parts`, `signature_payload`, `encode_signed_extrinsic` (v4 only), `era_birth`, `multisig_account_id` | no | the submission pipeline, offline prepare and attach |
| keys | `Keypair` constructors, `sign`, `verify`, ss58 helpers | no (encrypted JSON import is `host`) | `AccountId`, signers, btauth/1 |
| keyfiles | format detectors, `encrypt_keyfile_data`, `decrypt_keyfile_data`, JSON codec | yes, whole module | `rtao-wallet` feature `encrypted` |
| timelock | `WeightsTlockPayload`, `encrypt_and_compress`, `encrypt_at_round`, `decrypt_with_signature`, constants | crypto no; drand fetches yes | weights commits, timelock UX, timelocked commitments, the round math of `rtao_primitives::drand` |
| MEV shield | `mlkem::seal(pk, plaintext, include_key_hash)` | no | `rtao-client::shield` |
| RFC-0078 | `metadata_digest`, `generate_extrinsic_proof` | no | `CheckMetadataHash`, Ledger |
| PoW | `pow::{seal, mine_cpu}`, `pow::opencl::GpuMiner` | yes | `PowSolver` (owner and CLI builds) |
| Ledger | `LedgerDevice::{open, app_version, address, sign}` | feature `ledger` | `LedgerSigner`, behind the `LedgerBackend` seam; the upstream HID transport cannot be swapped (G16), so rtao's test lane takes PR-U17 from the core patch ([§5.10](#510-the-pr-u17-core-patch)) |

### 5.2 Wrapped

Thin adapters whose bytes stay the core's ([architecture §1.5](docs/plan/architecture.md#15-wrapped-thin-adapters-the-bytes-stay-the-cores)):
`Runtime` → `rtao_runtime::RuntimeHandle(Arc<RuntimeEntry>)`, re-exported by `rtao-client` and cached by
`rtao_client::RuntimeCache` (V15 enforcement, caching per `(genesis, spec)`, surface report, shape cache, the RFC-0078
digest computed once per spec; V16 is decoded only by `xtask`);
`Keypair::sign` → `KeypairSigner` (inline, about 0.03 ms); `LedgerDevice::sign` → `LedgerSigner` (one device mutex
across whole operations, plus `spawn_blocking`, behind the `LedgerBackend` seam); `decrypt_keyfile_data` →
`Wallet::unlock_coldkey` (once per process, Argon2i with 512 MiB, zeroized); `mine_cpu`/`GpuMiner` → `PowSolver`
(dedicated threads, cancellation, work-block refresh); timelock crypto → `TimelockCommitter` and `rtao_tx::timelock`
with an async drand client, and the pure round math in `rtao_primitives::drand`; `Value` → `DynValue`.

### 5.3 Staged in `rtao-core-ext`

Each module starts with three machine-read lines: `//! UPSTREAM-TARGET: <path> (PR-Ux)`,
`//! UPSTREAM-PROBE: symbol <path>` or `vectors <suite>`, and `//! UPSTREAM-FEATURES: <core features>` (default none).
`cargo xtask staged --check` generates one probe crate per module: its own workspace, the pinned core with
`default-features = false` plus exactly the listed features, rtao's `w3f-bls` patch and a copy of rtao's `Cargo.lock`.
Only an unresolved-name error (E0405, E0412, E0425, E0432, E0433 or E0599), a missing core feature or failing vectors
count as "not upstream yet"; any other failure fails the gate with the compiler output, so it cannot go silently inert.
A passing probe fails the gate, which forces the deletion of the staged copy; a probe that passes only with `host`, for
a module the no-host profile uses, is reported as "upstream but host-gated" and deletes nothing. G4's probe is
`vectors`, because the core already exports the predicate's names with the buggy predicate. When upstream absorbs a
module without `host`, one `pub use` line in `rtao-core` flips and no other crate changes
([architecture §1.6](docs/plan/architecture.md#16-gaps-and-divergences-in-the-core-and-the-staged-code-that-closes-each)).

| Gap | Staged module | PR |
|---|---|---|
| G1 protocol logic private inside the blocking client | `ops::{metadata, storage, runtime_api, extrinsic_lookup, dispatch_error}` (sans-IO) | PR-U1 |
| G2 fee estimation uses deprecated `payment_queryInfo` | `ops::fee` (immortal zero-signature probe, `RuntimeDispatchInfo` decode) | PR-U1 |
| G3 receipts check 3 nested wrappers, Python 6 | `ops::receipt` (Python semantics) | PR-U2 |
| G4 epoch predicate differs from the pallet | `epoch_schedule` (pallet-faithful) | PR-U5 |
| G5 `generate_commit_v2` reads the wall clock | `timelock_commit` (`now` as a parameter) | PR-U5 |
| G6 `submit_shielded` signs with a 64-block era | none: rtao never calls `submit_shielded`; the runtime's era bound is extracted into `rtao_catalog::rt::MAX_SHIELD_ERA_PERIOD` and `ShieldPeriod` is tested against it | PR-U7 (fixes the core's users; retires nothing in rtao) |
| G7 `Policy::check` needs private `IntentCall` constructors | none: rtao ports `PY/intents/plan.py` and differential-tests it on 21 of the core's 22 constructors (`set_root_claim_type` is retired) | PR-U8 (retires nothing in rtao) |
| G8 metadata IR exists only in the PyO3 binding | `ir::MetadataIr` | PR-U9 |
| G9 plaintext keyfile JSON codec gated behind `host` | `keyfile_json` (line-for-line copy) | PR-U6 |
| G10 `save_password_to_environment` calls `std::env::set_var` | `keyfile_env::encode_password_for_environment` (pure) | PR-U10 |
| G11 strict signature verification is private | `verify_strict` | PR-U10 |
| G12 `Value` helpers live in host-gated `client.rs` | `value_ext::ValueExt` | PR-U15 |
| G13 V16 rejected, V14 has no runtime APIs | none: V15 required at run time; `frame-metadata` 23 for V16 in tooling | none |
| G14 extension guard fails only at signing time | none: rtao probes `signature_payload_parts` at connect | none |
| G15 22 constructors and about 9 read helpers vs 83 intents and 102 reads | none: `rtao-tx`, `rtao-intents`, `rtao-reads` port Python | none (out of the core's scope) |
| G16 the core's Ledger APDU transport is a private concrete HID type, so nothing can point `LedgerDevice` at an emulator | none: until PR-U17 ships in `core_tag`, rtao's own workspace builds the core through a Cargo patch that adds it, and only `rtao-testkit::ledger_emu` names its items ([§5.10](#510-the-pr-u17-core-patch)) | PR-U17 |

G16 is the one gap closed by a patch rather than a staged module. Staged code adds functions that rtao can host in
`rtao-core-ext`; PR-U17 changes a private type of the core, which only a patched core can expose, and because Cargo
ignores `[patch]` in dependencies, a patch suits only rtao's own test lane, never code that subnets run.

### 5.4 Not used on runtime paths

`client::Client` (blocking reqwest, polling receipts, no subscriptions or nonce cache), `Client::submit_shielded`,
`transaction::{Executor, IntentCall, Plan, Policy}`, `timelock::generate_commit_v2`, the blocking drand fetches,
`epoch_schedule::{predict_first_reveal_block, should_run_epoch}`, `keyfiles::save_password_to_environment`, and a V14
`Runtime` for signing. A workspace `clippy.toml` `disallowed-methods` list turns any use into a CI failure; the
`core-oracle` test lane opts out per item ([architecture §1.8](docs/plan/architecture.md#18-deliberately-not-used-on-runtime-paths)).

All ten listed paths exist in the core (re-checked). Seven are host-gated and resolve only when the core is built with
`host`: `client::Client::connect`, `transaction::Executor::{new, with_policy}`,
`timelock::{decrypt, get_round_info, get_reveal_round_signature}` and `keyfiles::save_password_to_environment`
(`CORE/lib.rs:10-29`; `CORE/timelock/mod.rs:241, 275, 297`). Measured on clippy 1.89 and 1.99: a configured path that
does not resolve produces a non-lint warning ("does not refer to a reachable function") that `-D warnings` does not
turn into an error, so an upstream rename would silently disable its ban. Decided:

- the seven host-gated entries carry `allow-invalid = true` (accepted by clippy 1.89 and 1.99), which silences the
  warning in no-host runs and still enforces the entry wherever its path exists;
- the enforcing run stays `cargo clippy --workspace --all-targets --all-features -- -D warnings`, where all ten paths
  resolve; the `no-host` job adds a no-host clippy run, and clippy runs only on the development toolchain (the MSRV job
  runs `cargo check`);
- `xtask core-surface` emits, for each of the ten paths, `#[allow(clippy::disallowed_methods)] const _: <signature> =
  <path>;` into `crates/rtao-core/tests/core_surface.rs` (the host-gated ones under `#[cfg(feature = "host")]`), so a
  renamed or removed path fails the build instead of disabling its ban.

### 5.5 Two pins and `upstream.toml`

`upstream.toml` is the single machine-readable pin file, written only by `cargo xtask pin` (and, for `[core.patch]`,
by `cargo xtask core-patch`, [§5.10](#510-the-pr-u17-core-patch))
([architecture §1.2](docs/plan/architecture.md#12-pinning-core_tag-and-parity_tag)):

```toml
[core]                      # what Cargo builds; downstream [patch] lines must match this tag
tag = "v477"
commit = "5c6e83e2f4936f257234f246957f75684c35227c"
toolchain = "1.89"
fingerprint = "b3:…"        # core-input fingerprint at core.tag

[parity]                    # what rtao mirrors; moves on every finalized mainnet release
tag = "v477"
commit = "5c6e83e2f4936f257234f246957f75684c35227c"
spec_version = 477
python_package_version = "11.3.1.dev0"   # the in-tree source version at parity.tag, not a PyPI release
# plus [monorepo]; [localnet] image digests for pinned, mainnet, testnet, devnet and main (the pinned digest is written
# by `xtask pin --check`); [networks.*] genesis and metadata pins; [artifacts] sha256 table; and, while rtao carries
# PR-U17 (from M7-09), [core.patch] (§5.10)
```

- **`parity_tag`** moves on every finalized mainnet release, once its `:vN` localnet image is published: re-vendor
  catalogs and fixtures, re-extract, regenerate, refresh oracle vectors.
- **`core_tag`** moves only when the **core-input fingerprint** changes: a BLAKE3 hash over, for each package in the
  core's path-dependency closure (today `sdk/bittensor-core`, `support/macros` and `vendor/w3f-bls`), the git tree SHA
  of `src/` and the blob SHAs of `Cargo.toml` (and `build.rs`); the inherited `[workspace.dependencies]` entries and
  `[workspace.package]` keys (`edition`); and the root `[patch.crates-io]` entries for crates in the core's graph
  (today only `w3f-bls`). Pallets, runtime, Python and docs cannot change what Cargo builds for rtao and do not move
  it.
- Over the last six finalized release pairs, the core's sources changed in one pair (`v473→v475`) and its tests alone in
  one more (`v469→v473`), while `sdk/python` changed in all six (re-checked with `git diff` between tags).
- When the fingerprints match, the oracle built from `parity_tag` uses a core byte-identical to `core_tag`'s, so the
  oracle stays a valid specification for rtao's build.
- Third-party versions are a separate matter: the oracle builds with `UP/Cargo.lock`, rtao and every subnet with their
  own locks, and at v477 88 of the 241 packages in the core's no-host graph resolve to versions absent from
  `UP/Cargo.lock`. `xtask pin` records `UP/Cargo.lock`'s versions of the byte-defining crates in
  `vendor/oracle/lock.json`, and `deps-check` warns on rtao's differences, so an oracle mismatch is checked against
  that file before anything is ported.

### 5.6 The `w3f-bls` patch line

The core declares `w3f-bls = { workspace = true, features = ["std"] }` (`UP/sdk/bittensor-core/Cargo.toml:62`,
re-checked), which resolves to the monorepo's vendored copy, with the comment that the vendored fork keeps "ciphertexts
never drift from what pallet-drand can decrypt" (`:51-53`). `tle` asks crates.io for `=0.1.3`. Cargo ignores `[patch]`
tables in dependencies, so without a patch in the final workspace the lockfile holds two `w3f-bls 0.1.3` sources and
the core fails with E0277 (verified in the design session, [architecture §1.3](docs/plan/architecture.md#13-the-w3f-bls-patch-end-to-end);
the reproduction project is [bench/nopatch](docs/plan/bench/nopatch/)). Every final workspace therefore
carries:

```toml
[patch.crates-io]
w3f-bls = { git = "https://github.com/RaoFoundation/subtensor.git", tag = "v477" }   # MUST equal rtao's core_tag
```

Enforcement:

1. `crates/rtao-core/tests/lock.rs` asserts that rtao's own `Cargo.lock` has exactly one `w3f-bls`, from the same git
   source and tag as `bittensor-core`'s declaration, equal to `rtao_core::BITTENSOR_CORE_TAG` (while the core patch
   exists, the lock takes `bittensor-core` itself from `[core.patch]`, which `xtask pin --check` verifies, and the
   patch's second table keeps `w3f-bls` at that tag, [§5.10](#510-the-pr-u17-core-patch)).
2. `rtao doctor` runs the same check on a downstream workspace, taking the expected reference from that workspace's
   own `bittensor-core` entry (the reference kind must match too: a `rev` of the same commit is a different Cargo
   source and locks a second copy). It prints the exact line to add or fix, so the E0277 symptom becomes a one-line
   diagnosis, and only warns when that tag differs from its own `BITTENSOR_CORE_TAG`.
3. The `cargo generate` template ships the line once, in its workspace root (Cargo ignores `[patch]` in member
   manifests), and never the core patch of [§5.10](#510-the-pr-u17-core-patch).
4. Upstream PR-U4 changes the core's manifest to `w3f-bls = { version = "=0.1.3", features = ["std"] }`; the monorepo
   keeps its root patch to `vendor/w3f-bls` (`UP/Cargo.toml:335`, re-checked). The PR ships with evidence from
   `ci/pr-u4-probe/`, a separate Cargo workspace (no `[patch]`, its own lock) that builds the core with PR-U4's
   manifest line: its ciphertexts decrypt offline in rtao's patched build and on localnet in pallet-drand (scenario
   16). crates.io `w3f-bls` 0.1.3 and `vendor/w3f-bls` differ in four files, none on the timelock path, which predicts
   a yes. Until that proof exists and the PR merges, the patch stays.

Because `core_tag` moves rarely, the downstream line changes only on core bumps; those bumps are marked
`downstream-patch-change` and the release notes print the new line. A core bump rewrites every reference generated
from `[core]` (the root manifest, the template, the README and book snippets), and a repository test asserts that
every monorepo git dependency (all declared in the root `[workspace.dependencies]`) and every `w3f-bls` git line carry
`[core].tag`.

### 5.7 The no-host neuron profile

A validator or miner needs a plaintext hotkey plus signing, codec, runtime, extrinsic assembly and timelock
encryption. None of these is host-gated; only the plaintext keyfile codec is (`CORE/lib.rs:10-31`, re-checked). So
([architecture §1.7](docs/plan/architecture.md#17-host-and-the-no-host-neuron-profile)):

- `rtao-core` exposes `host` (off by default, forwards to `bittensor-core/host`) and `ledger`.
- `rtao-wallet` reads plaintext hotkeys through the staged `keyfile_json`, without `host`. Encrypted coldkeys need
  feature `encrypted`.
- Facade features `validator` and `miner` do not enable `host`; `owner`, `wallet-encrypted`, `pow` and `full` do.
- A differential test (`crates/rtao-core/tests/keyfile_json_diff.rs`) runs the staged reader and the core's
  `deserialize_keypair_from_keyfile_data` over every keyfile vector (upstream goldens, btcli-created files, legacy
  `" ss58Address"` files, numeric-string `cryptoType`, bare mnemonics and seeds, polkadot-js exports) and asserts
  identical keypairs or identical error classes.
- The CI `no-host` job builds `-p rtao --no-default-features --features validator,prometheus` and, separately,
  `--features miner,redis`. It asserts with `cargo tree -e normal,build` that none of `sodiumoxide`, `libsodium-sys`,
  `openssl`, `openssl-sys`, `openssl-src`, `native-tls`, `fernet`, `ansible-vault`, `secp256k1-sys`, `aws-lc-rs`,
  `aws-lc-sys`, `cmake`, `hidapi`, `libloading` or `rayon` is present. Build edges are included so that C builders
  such as `cmake` and `openssl-src` are visible. It also asserts that `reqwest` lacks the `blocking` feature (the
  core's blocking copy) and that `sp-core` is built without `std` (`cargo tree -e features -i sp-core`), and it runs
  a no-host clippy pass.
- Native code that remains: `ring` (TLS) and `blake3`'s SIMD kernels (through `merkleized-metadata`), both built by
  the C compiler rustc already uses as its linker. Neuron images therefore need no Perl, OpenSSL, libsodium or CMake.
- TLS uses one explicitly built `ring` rustls config: `rtao-rpc` passes it to tokio-tungstenite as a
  `Connector::Rustls`, every rtao `reqwest` 0.13 client (`SignedClient`, `DrandClient`, `EvmRpc`, and `rtao-cli`'s
  `upgrade` release fetcher and `wallet history` indexer client) is built with `rustls-no-provider` and
  `tls_backend_preconfigured`, and library crates never consult or install the process-default provider. Applications
  (`rtao-cli`'s `main`, the template binaries) install ring as the default as a backstop for third-party code. Loopback
  `wss://` and `https://` `FakeNode` tests run in the no-host profile, in an `--all-features` build and in a build that
  also enables `rustls/aws-lc-rs` ([architecture §2.7](docs/plan/architecture.md#27-third-party-stack)).
- PR-U6 proposes splitting `host` upstream (`keyfiles-json`, `keyfiles-crypto`, `drand-http`, `rpc-client`, `parallel`,
  `pow`); the staged reader is deleted when it lands.

`hidapi` is gated by the core's `ledger` feature, not `host`, and on Linux it uses the pure-Rust hidraw backend
(`UP/sdk/bittensor-core/Cargo.toml:82-94, 126`, re-checked).

### 5.8 Known core defects and how rtao handles them

| # | Defect | Evidence | rtao handling | Upstream |
|---|---|---|---|---|
| 1 | **MEV-shield era.** `Client::submit_shielded` signs both the inner extrinsic and the `MevShield.submit_encrypted` carrier with a 64-block era. The runtime rejects a `submit_encrypted` extrinsic whose era is immortal or longer than 8 blocks with `InvalidTransaction::Stale`, so the core's shielded submission is refused at the pool | `CORE/client.rs:32` (`DEFAULT_ERA_PERIOD: u64 = 64`), `:622-654` (inner signed at `:633`, carrier at `:640-646`); `UP/runtime/src/check_mortality.rs:19` (`MAX_SHIELD_ERA_PERIOD: u64 = 8`), `:91-100`; identical at `v477` (re-checked) | rtao never calls it. `ShieldPeriod { P4, P8 }` is the only era type the shield path accepts; inner and carrier both use it, as Python does (`PY/settings.py:24-29`, `PY/executor.py:1039, 1049`, re-checked). Carrier and inner are applied together in block B + 2, so the shield path also reads the finalized head F and refuses with `TxError::ShieldEraExpired` when B + 2 ≥ F + p (`P8` tolerates a finality lag of at most 5, `P4` at most 1; D-025); `lease_pair` leaves the next nonce at n + 2 (D-026); shielded submissions are never retried, as in Python | PR-U7; **confirmed bug** |
| 2 | **Epoch predicate.** The core's `should_run_epoch` fires the safety net at `blocks_since_last_step > MAX_TEMPO_U64` (50,400); the pallet fires at `BlocksSinceLastStep > tempo` and caps the counter at `tempo + 1`. After an `EpochSkipped` slot (`LastEpochBlock` advances but `BlocksSinceLastStep` is not reset), or after `sudo_set_tempo` lowers the tempo below the counter (only `LastEpochBlock` is reset), the pallet fires the safety net as soon as the counter exceeds `tempo`: the very next block after a skipped scheduled slot or a lowered tempo, and every block while the input state stays inconsistent. The core waits for `LastEpochBlock + tempo`, so it predicts the first reveal at least one epoch late, the ciphertext targets a round after the reveal epoch, and the commit is lost silently. Deferral sets `PendingEpochAt`, which both predicates honor, and does not diverge. Python inherits the defect through the binding | `CORE/timelock/epoch_schedule.rs:45-59` (predicate at `:54`), `CORE/timelock/constants.rs:6-9`; `UP/pallets/subtensor/src/coinbase/run_coinbase.rs:1224-1238` (predicate at `:1232`), cap at `:412-416`, deferral `:422-431`, counter reset only on a consistent run `:434-438`, skip `:472-483`; `coinbase/tempo_control.rs:38` → `utils/misc.rs:119-125`; Python path `PY/intents/weights.py:308` → `UP/sdk/bittensor-core-py/src/timelock.rs:61` → `generate_commit_v2`; identical at `v477` (re-checked) | staged pallet-faithful `epoch_schedule`, tested to agree with every upstream vector and to differ only on new safety-net vectors (skip, owner-triggered skip, lowered tempo; [§12.4](#124-m2-validator)); divergence D-006; `clippy` bans the core functions | PR-U5; **confirmed bug** |
| 3 | `generate_commit_v2` reads the wall clock internally | `CORE/timelock/mod.rs:160-195` (`now_unix()` at `:177`, re-checked) | staged `timelock_commit` with an injectable clock; `ClockGuard` refuses commits when skew exceeds 12 s | PR-U5 |
| 4 | Receipts check 3 nested-result events (`Proxy.ProxyExecuted`, `Sudo.Sudid`, `Multisig.MultisigExecuted`); Python checks 6 plus MevShield failure events and a `Treasury`/`Balances.Deposit` fee fallback. The core also misreads `Sudo.Sudid` (verified by code reading, not executed): it looks up field `result` and otherwise uses the whole attribute map; named event fields decode to a map keyed by field name, so `Sudid`'s map is `{sudo_result: …}`, and `variant` is a top-level field lookup, so an `Err` under `sudo_result` is never found. A failed `Sudo.sudo` therefore reports success through the core client | `CORE/client.rs:1024-1044` (lookup at `:1025-1037`), `:1515-1527` (`field`, `variant`); `CORE/codec/decode.rs:428-435`; finney metadata (spec 477): `Sudo` event 0 is `Sudid { sudo_result: DispatchResult }`; `PY/_transport/utils/receipt.py:8-16, 75-96` (re-checked) | staged `ops::receipt` with Python's semantics, which reads `sudo_result`; a `FakeChain` receipt test with a failed `Sudid` (M2) and scenario 18's two Sudo-wrapped ops (M5) are PR-U2's evidence | PR-U2 |
| 5 | Fee estimation calls the deprecated `payment_queryInfo` on a 64-block mortal extrinsic; Python probes `TransactionPaymentApi_query_info` through `state_call` with an immortal, zero-signature extrinsic | `CORE/client.rs:551-576`; `PY/_transport/interface.py:485-491, 714-731` (re-checked) | staged `ops::fee` | PR-U1 |
| 6 | The core client anchors mortal eras at the best head; Python anchors at the finalized head | `CORE/client.rs:326-328, 514-518`; `PY/_transport/interface.py:485-499` (re-checked) | rtao anchors at the finalized head and warns when finality lags more than half the period | none (the core client is not used) |
| 7 | `Runtime::parse` rejects V16 and builds no runtime APIs for V14; finney's `state_getMetadata` returns V14 | `CORE/runtime/mod.rs:171-198` (re-checked) | V15 always; V14 only on Python's missing-method needle, read-only (D-010) | none |
| 8 | Extrinsic assembly is v4 only, while finney advertises `[4, 5]` | `CORE/codec/extrinsic.rs:268-273` (re-checked) | sign v4; the drift job raises a `breaking` alert on any move to v5 | none yet |
| 9 | 4 of the core's 22 `IntentCall` constructors (`transfer_allow_death`, `move_stake_limit`, `transfer_stake_and_hotkey`, `set_root_claim_type`) have no catalog op of the same name | `CORE/transaction.rs:251-826`; `CAT/intents.json` (re-checked) | the `Policy` equivalence test pairs 21 of the 22: 18 by op name, `transfer_allow_death` ↔ `Transfer { keep_alive: false }`, `move_stake_limit` ↔ a cross-subnet `MoveStake` with slippage protection, `transfer_stake_and_hotkey` ↔ `TransferStake` with a different destination hotkey. `set_root_claim_type` is a retired call, absent from spec-477 metadata, so it is excluded and its e2e name is mapped n/a. The pairs grow with the intents: 3 in M2, 8 in M4, 21 at the M5 exit | PR-U8 |
| 10 | (interaction) The core depends on `serde_json` with `arbitrary_precision`, non-optionally, so Cargo enables it for every crate in an rtao or subnet build. Measured on serde_json 1.0.154: non-integer numbers parsed from text fail inside `#[serde(untagged)]` enums and `#[serde(flatten)]` structs, and `serde_json::Value` equality compares number spelling (`1e-05 != 1e-5`) | `UP/sdk/bittensor-core/Cargo.toml:40-42`; `REG/serde_json-1.0.154/src/de.rs:118-129, 937-953`, `src/number.rs:20-24, 72-73` (found in this plan). The core needs the feature: `CORE/codec/value.rs:58-77` renders u128 and i128 values beyond 64 bits and U256 through it and substitutes 0 without it, and every U64F64 value ≥ 1.0 has bits ≥ 2^64 | decided (M0): (1) `[workspace.dependencies] serde_json = { version = "1.0.132", features = ["arbitrary_precision"] }`, used by `rtao-core`, so rtao's JSON stays digit-exact whatever upstream does; (2) rtao's number-or-string and untagged input types (`WeightsArg`, `Amount`, `Ratio`) deserialize through `serde_json::Value` and read numbers from `Number::as_str()`, the exact decimal text, so they parse from any entry point; (3) every rtao text-JSON entry point parses `Value`-first through `rtao_primitives::json::{from_slice, from_str}` (CLI `--args` and `call`, `execute_tool` from text, `UnsignedExtrinsic` import, `btcli.json` and the books, oracle vectors); (4) subnet authors use `rtao::btauth::server::Json<T>`, an axum extractor with the same `Value`-first parse and axum's rejection shape; (5) the oracle comparator walks both trees and compares integer tokens (no `.`, `e` or `E`) exactly as i128/u128 (canonical decimal strings beyond), non-integers as f64 (shortest round trip, so `1e-05` equals `1e-5`), and an integer against a non-integer as a mismatch, never with `serde_json::Value` equality; (6) the M0 test pins direct-vs-`Value`-first parsing of untagged and flatten floats, a u128/U256 `to_corpus_json` round trip, and the comparator on 11,614,906,923,326,217 vs 11,614,906,923,326,216 (finney's total issuance), 2^53 + 1, `u64::MAX / 4` and its neighbor, and `1e-05` vs `1e-5`; (7) the template documents the extractor | none needed; optionally ask that the feature stay a documented contract of `to_corpus_json` |

### 5.9 Keeping up with an unversioned core

`bittensor-core` is `publish = false` at version 0.1.0 and never stamped. Three mechanisms catch API changes
([architecture §1.9](docs/plan/architecture.md#19-keeping-up-with-an-unversioned-core)): the generated `core_surface`
compile probe names every core item `rtao-core` and `rtao-core-ext` use, with its exact signature; the nightly
`core-canary` builds against the monorepo's `main` with the `:main` localnet image; and the golden vectors are
replayed through rtao's wrappers in `pr-gates`.

### 5.10 The PR-U17 core patch

The user decided (Q20, [§14.1](#141-decisions-answered-2026-10-13)) that until a `core_tag` contains PR-U17 (the
`ApduTransport` trait and `LedgerDevice::with_transport`, [§15](#15-upstream-contributions)), the rtao workspace builds
`bittensor-core` through a Cargo `[patch]` that adds exactly PR-U17 on top of `core_tag`
([architecture §1.11](docs/plan/architecture.md#111-the-pr-u17-core-patch)). M7-09 adds it, unless `core_tag` already
contains PR-U17. The mechanics below were prototyped in Docker while planning, with a local stand-in for the fork:
[research J §8](docs/plan/research/J-ledger-ci-shim.md#8-core-patch-prototype) has the PR-U17 diff, the checks and an
emulator run of the core's own `ledger.rs` through `with_transport`.

- **Branches.** PR-U17 is written and filed in M0-01 from the user's fork of the monorepo (default
  `github.com/sam0x17/subtensor`, created when filing); its review branch targets monorepo `main`. The patch branch
  `rtao-core-patch/<core_tag>` in the same fork is `core_tag` plus the PR-U17 commits, cherry-picked, and new commits
  refresh it when the PR changes in review. Patch branches are never force-pushed or deleted while an rtao commit pins
  them (once a deleted branch's commits are pruned, a fresh resolve of its `rev` fails); each new `core_tag` gets a
  new branch.
- **Root `Cargo.toml` only**, in two tables:

  ```toml
  # PR-U17 until core_tag contains it ([core.patch] in upstream.toml; checked by `cargo xtask pin --check`)
  [patch."https://github.com/RaoFoundation/subtensor.git"]
  bittensor-core = { git = "https://github.com/sam0x17/subtensor.git", rev = "<full commit>" }

  # The patched core's other path dependencies, kept at core_tag (one w3f-bls, one subtensor-macros)
  [patch."https://github.com/sam0x17/subtensor.git"]
  w3f-bls = { git = "https://github.com/RaoFoundation/subtensor.git", tag = "v477" }
  subtensor-macros = { git = "https://github.com/RaoFoundation/subtensor.git", tag = "v477" }
  ```

  The `[patch.crates-io] w3f-bls` line ([§5.6](#56-the-w3f-bls-patch-line)) stays as it is, but the first table alone
  does not build: the patched core's in-repo path dependencies take the fork's source, so the lock holds a second
  `w3f-bls` next to `tle`'s and the core fails with E0277, and `rtao-core-ext`'s `subtensor-macros` gets a second copy
  too. The second table keeps one of each at `core_tag`, so `bittensor-core` is the only package from the fork, locked
  as `git+https://github.com/sam0x17/subtensor.git?rev=<full commit>#<full commit>`. Cargo prints no warning for
  either table; features stay on the `[workspace.dependencies]` entry, because Cargo ignores them on a `[patch]` entry.
- **`upstream.toml`** gains `[core.patch]`: `repo`, `branch`, `rev`, `base` (equal to `core.commit`), `upstream_pr`
  (the PR's link), `paths` (the files PR-U17 may touch: `mod.rs`, `ledger.rs` and `hid.rs` in
  `sdk/bittensor-core/src/signers/`) and `probe`, the two items whose presence in the unpatched core proves that PR-U17
  shipped (`bittensor_core::signers::ApduTransport` and
  `bittensor_core::signers::ledger::LedgerDevice::with_transport`).
- **`cargo xtask pin --check`** (in `pr-gates`) verifies, while `[core.patch]` exists, that `base` equals `core.commit`
  and `rev` descends from `base`; that `git diff base rev` touches only `paths`; that the lockfile's `bittensor-core`
  source is `repo` at `rev` and no other package comes from `repo`; that the two `clippy.toml` entries below name the
  `probe` items; and that the patch is still needed: a generated probe crate outside the rtao workspace (its own
  `[workspace]` table, `w3f-bls` line and target directory) names both `probe` items, compiles against `repo` at `rev`
  (carrying the second table above, without which `w3f-bls` splits as with the first table alone), and fails against the
  unpatched `core_tag` with an unresolved-name error in its own source; any other failure is a check error, as for the
  staged probes of [§5.3](#53-staged-in-rtao-core-ext). Once the probe compiles against `core_tag`, or the PR's merge
  commit is an ancestor of `core.commit`, the check fails with "PR-U17 shipped: drop the core patch".
- **`cargo xtask core-patch rebase <tag>`** cherry-picks the PR-U17 commits (`base..rev`; on its first run, in M7-09,
  the commits of PR-U17's review branch) onto the tag, pushes `rtao-core-patch/<tag>` to the fork, and rewrites
  `[core.patch]` and both tables. A conflict stops it with `core-patch-conflict`, which the agent resolves like any
  upstream change and logs. A pin bump that moves `core_tag` runs it while `[core.patch]` exists and the new tag lacks
  PR-U17; when the new tag contains PR-U17, the bump deletes both tables and `[core.patch]`, adapts `rtao-testkit` to
  the API as merged, and logs it ([§10.3](#103-a-pin-bump-step-by-step)). `cargo xtask core-patch strip` deletes both
  tables in a disposable CI checkout.
- **Subnets never see it.** Cargo applies `[patch]` only from the root manifest of the workspace being built and
  ignores a dependency's tables without a warning, so subnet projects keep building the upstream tag with the `w3f-bls`
  line as their only patch, and the `cargo generate` template must not copy the core patch. Shipped crates must
  therefore compile against the unpatched core: only `rtao-testkit` (feature `ledger-emu`) may name PR-U17's items.
  `clippy.toml` bans `LedgerDevice::with_transport` (`disallowed-methods`) and `ApduTransport` (`disallowed-types`),
  both with `allow-invalid = true`, everywhere but `rtao-testkit::ledger_emu`; the bans flag imports, type positions,
  bounds and calls, but not a trait `impl` header written by full path. The `core-unpatched` job (nightly, and on
  pushes that change `upstream.toml` or the root `Cargo.toml`) catches that case too: it deletes both tables in its
  disposable checkout and runs `cargo check --workspace --exclude rtao-testkit --exclude xtask --all-features` without
  `--locked` (Cargo then re-locks only `bittensor-core`), which proves what subnets build; because `--all-features`
  still builds an excluded crate that a feature reaches, it also fails if a shipped feature pulls in `rtao-testkit`
  ([§4.2](#42-layers) rule 8). The `template` job (M3-09) stays the end-to-end downstream proof.
- **Coverage.** From its first run the emulator lane drives the core's own `ledger.rs` (APDU building, 250-byte
  chunking, status-word mapping) against the real app, through `LedgerDevice::with_transport` over `rtao-testkit`'s
  Speculos TCP transport, and `ledger_replay` in `pr-gates` uses a replay transport the same way
  ([§9.5](#95-python-oracle-and-interop)); the prototype did both. The device run covers only what the emulator
  cannot: the HID transport and OS device access (`hid.rs`, `LedgerDevice::open`), the app binary users install, real
  firmware and the review on a real screen.
- **Fallbacks.** If the PR changes in review, the patch branch follows it. If upstream declines PR-U17, the agent stops
  and asks the user (rule 6 of [Executing this plan](#executing-this-plan)): keep carrying the patch, fall back to a
  test-only transcription of `CORE/signers/ledger.rs` in `rtao-testkit`, or build a uhid bridge, which otherwise stays
  a non-goal ([§16](#16-non-goals)).

## 6. Key technical decisions

### 6.1 The 25 decisions that define the design

From [architecture §0.3](docs/plan/architecture.md#03-twenty-five-decisions-that-define-the-design); the reasons are
condensed from the decision log ([§16](docs/plan/architecture.md#16-decision-log)).

| # | Decision | Reason | Depth |
|---|---|---|---|
| 1 | Base on proposal D; graft C's operator runtime and seams, A's transport and fidelity rules, B's types and measurements | D had the best total (119) and mean rank (1.83) across the three judges; its anti-drift machinery is the hardest part to retrofit | [§0.1](docs/plan/architecture.md#01-which-proposal-is-the-base-and-why) |
| 2 | Seam = `rtao-core` (only importer of `bittensor_core`) plus `rtao-core-ext` (staged upstream code with a deletion gate) | a core API change is absorbed in one crate; an upstream merge is a one-line `pub use` flip | [§1.1](docs/plan/architecture.md#11-principle-who-defines-the-bytes) |
| 3 | Two tags: `core_tag` (moves on a core-input fingerprint change) and `parity_tag` (every finalized release) | the downstream patch line must match `core_tag`, and the core rarely changes while Python always does | [§1.2](docs/plan/architecture.md#12-pinning-core_tag-and-parity_tag) |
| 4 | The downstream `[patch.crates-io] w3f-bls` line is mandatory and checked by `rtao doctor`; PR-U4 removes it only after a localnet decrypt proof | without it Cargo locks two `w3f-bls` sources and the core fails with E0277 | [§1.3](docs/plan/architecture.md#13-the-w3f-bls-patch-end-to-end) |
| 5 | Neuron binaries build without `bittensor-core/host`, through a staged plaintext keyfile reader tested against the core | neurons need only a plaintext hotkey; neuron images need no libsodium, OpenSSL, `secp256k1-sys`, Perl or CMake (their only native code is `ring` and `blake3` SIMD, built by the compiler rustc already uses as its linker) | [§1.7](docs/plan/architecture.md#17-host-and-the-no-host-neuron-profile) |
| 6 | Transport: rtao's own tokio-tungstenite session behind `RpcClientT`, with TLS from one explicitly built `ring` rustls config, never the process-default provider; no jsonrpsee in the default graph | Python's refusal contract needs the refused handshake's headers and close code 1008, which jsonrpsee drops; with zero or two rustls providers in the graph, `ClientConfig::builder()` panics | [§3.2](docs/plan/architecture.md#32-transport-rtao-rpcsession) |
| 7 | Reconnect re-sends only methods in a `ReadMethod` allowlist; `author_submit*` is structurally absent; a queued frame whose caller is gone is never transmitted | a re-sent submit can clear the nonce cache under a live transaction; a cancelled submission's dropped `NonceLease` has already cleared it | [§3.2.1](docs/plan/architecture.md#321-the-resend-allowlist-readmethod) |
| 8 | Policy refusals are classified exactly as `rpc.py` does, from extracted constants | parity; an ordinary `-32005` must not become a terminal refusal | [§3.2.3](docs/plan/architecture.md#323-policy-refusals-exactly-as-python-classifies-them) |
| 9 | V15 metadata always; V14 only on Python's missing-method needle, and then read-only | finney's `state_getMetadata` is V14, which has no runtime APIs | [§3.4](docs/plan/architecture.md#34-runtime-metadata-v15-required-v14-only-where-python-uses-it) |
| 10 | Generated descriptors are name-based; storage keys always come from the live entry; shape hashes only verify | archive reads stay correct across hasher changes; no second encoding engine | [§3.7](docs/plan/architecture.md#37-generated-chain-descriptors-shape-hashes-surface-verification-rtao-runtime) |
| 11 | Exact shape hashes for calls and the positional metagraph view; projection hashes for by-name models; the outer `RuntimeCall`/`RuntimeEvent`/`RuntimeError` enums are opaque; fail closed per call | appended runtime fields stay harmless for by-name models; signing and positional parsing stay safe; a call added in any pallet does not make older builds refuse the intents that wrap `batch_all`, `as_multi`, `proxy_announced` or `sudo` | [§3.7](docs/plan/architecture.md#37-generated-chain-descriptors-shape-hashes-surface-verification-rtao-runtime) |
| 12 | One encoder: `Runtime::compose_call` | the golden vectors pin a single encoder | [§3.8](docs/plan/architecture.md#38-typed-call-builders-and-callbytes) |
| 13 | Coalesced reads: heterogeneous keys in one `state_queryStorageAt` at one pinned hash, chunked at 256 keys | public endpoints allow about 1 r/s and refuse storage work terminally (`-32004`) | [§3.6](docs/plan/architecture.md#36-reading-state) |
| 14 | Nonces follow Python (clear on any failure, re-ask the pool-aware `account_nextIndex`) through an RAII `NonceLease`; one `NonceManager` per `Client`, shared by every clone and executor; reconciliation sits on top, never a reservation | reserving a nonce after `FateUnknown` strands later pipelined nonces when the transaction was dropped; two managers would hand out the same nonce | [§3.9.1](docs/plan/architecture.md#391-nonces-python-semantics-enforced-by-raii) |
| 15 | Python's era defaults everywhere (128 blocks, anchored at the finalized head), weight commits included; `ShieldPeriod { P4, P8 }` is the only shield era type, and a shielded submission whose era the finality lag would expire before inclusion is refused (D-025) | parity; 128 blocks already bounds reconciliation well inside a tempo; the chain caps shield eras at 8, and carrier and inner land together in B + 2 | [§3.9](docs/plan/architecture.md#39-submission-pipeline), [§3.11](docs/plan/architecture.md#311-mev-shielded-submission) |
| 16 | Receipts use Python's semantics (6 nested wrappers, MevShield failure events, fee fallback), staged as `ops::receipt`; the `retracted` and `finalityTimeout` watch statuses are resolved, not reported as failures (D-024) | the core misses failures inside `SudoAsDone`, `Sudid` and `Utility` batches; a retracted extrinsic may still be included, and a finality timeout means it is in a block | [§3.9.4](docs/plan/architecture.md#394-outcome-resolution-pythons-receipt-semantics-staged-opsreceipt-pr-u2) |
| 17 | Intents: generated interfaces and dispatch, a generated pass-through `build()` for 46 of 83 (the generator lands in M2), hand-ported bodies; `AnyIntent` plus a shrink-only `pending.toml` | a new upstream op is a compile error; a removed wrapped call breaks the build | [§4.2](docs/plan/architecture.md#42-intent-representation), [§4.3](docs/plan/architecture.md#43-generated-pass-through-bodies-and-their-limits) |
| 18 | Compile-time signer roles (`KeySource<Role>`), with a `dyn` path that keeps Python's runtime check | "neuron runtimes never load a coldkey" becomes a type error | [§4.2](docs/plan/architecture.md#42-intent-representation), [§6.4](docs/plan/architecture.md#64-roles-key-sources-and-key-views) |
| 19 | `commit_reveal_version` stays 4; the framework refuses early when storage differs; intents unchanged from Python | the chain uses the value to gate client upgrades; echoing storage could turn a loud error into a silent reveal loss | [§9.3](docs/plan/architecture.md#93-weightscheduler-the-validators-critical-path) |
| 20 | Reveal rounds read the schedule at the best head, as Python does, and use a pallet-faithful epoch predicate with an injectable clock and a clock-skew guard (registered divergence, upstream fix) | the core's predicate loses commits after an `EpochSkipped` slot or a tempo lowered below the step counter; the core's predictor assumes inclusion at the queried head + 1 | [§1.6](docs/plan/architecture.md#16-gaps-and-divergences-in-the-core-and-the-staged-code-that-closes-each), [§9.3](docs/plan/architecture.md#93-weightscheduler-the-validators-critical-path) |
| 21 | Weight quantization uses `f64::round_ties_even` and Python's summation order, pinned by oracle vectors with `.5` ties | Python's `round()` is half-to-even; Rust's `f64::round` is not | [§9.3](docs/plan/architecture.md#93-weightscheduler-the-validators-critical-path) |
| 22 | `Balance` text emulates Python's float formatting exactly | read JSON embeds `str(Balance)`, which is float-formatted above 2^53 rao | [§5.3](docs/plan/architecture.md#53-pythons-strbalance-byte-for-byte) |
| 23 | Python only in CI (oracle, interop, recorders, and the Speculos of the Ledger emulator lane); extraction and regeneration are Rust-only; PR gates are offline | 100%-Rust builds; a Python failure cannot block a build | [§11.5](docs/plan/architecture.md#115-the-python-oracle-ci-only-outputs-committed), [§11.9](docs/plan/architecture.md#119-ledger-emulator-lane-speculos) |
| 24 | Parity enforced mechanically: blob-SHA file map, symbol diff, catalog equality, extraction check, environment-variable map, coverage gate, oracle, shrink-only `pending.toml`, divergence register | catches behavior-only Python changes that leave the catalogs unchanged | [§4.9](docs/plan/architecture.md#49-parity-enforcement-the-machinery) |
| 25 | Operator-first milestones: validator, miner, owner, then intent, read, signer and CLI parity; no time estimates | operators get a working validator at M2, before most intents, reads and the CLI exist, instead of waiting for all reads | [§14](docs/plan/architecture.md#14-milestones) |

### 6.2 Consequential alternatives that were rejected

From the decision log ([architecture §16](docs/plan/architecture.md#16-decision-log)) and the non-goals
([§17](docs/plan/architecture.md#17-what-we-deliberately-do-not-do-yet)):

| Rejected | Proposed by | Why rejected |
|---|---|---|
| Bump the core pin on every release (one tag) | A, B, C, D | every subnet would edit its patch line 2-4 times a week although the core's sources changed in 1 of 6 release pairs |
| Build without the `w3f-bls` patch | A (M0), D (Q1) | two `w3f-bls 0.1.3` sources in the lockfile, E0277 |
| jsonrpsee `WsClient` or subxt `ReconnectingRpcClient` | C, D | loses `X-RateLimit-*`/`Retry-After` and close 1008; does not retain subscriptions or pending calls |
| Positional codec types and baked storage hashers for everything | B | appended runtime fields become errors; baked hashers break archive reads; a second encoder |
| A 32-block era with `InBlock` waits for weight commits; nonce reservation on `FateUnknown` | C | strands pipelined nonces; 128 blocks already resolve within about 26 minutes, inside a 72-minute tempo |
| Echo `CommitRevealWeightsVersion` from storage | B, C, D | after a root bump it tags a v4 payload with a new version and risks a silent reveal loss |
| Inherit the core's epoch predicate for Python parity | A | parity with a bug that silently loses validator commits |
| Python in the regeneration path | D | breaks 100%-Rust builds; Python failures would block builds |
| "Typed decoding is faster" (`DecodeAsType` for the metagraph) | A, D (claim); C (design) | measured 4x slower than the core `Value` decoder; the borrowed view is 50x faster than `Value` |
| `jsonschema` validation of intent inputs | D | Python validates through dataclasses; serde plus a ported `validate()` matches it with less compile weight |
| Re-export the core's blocking `Client` as `rtao::blocking` | D | its product behavior diverges from Python; `rtao::blocking` mirrors Python's `SyncClient`/`Subtensor` instead |
| subxt codegen or `OnlineClient` on runtime paths; `chainHead_v1` backends | — | a second metadata engine, encoder and signer; `CombinedBackend` fails on finney's WS endpoint |
| Model `MaxEpochsPerBlock` deferral | J-CF concern | deferral moves reveals later, which per-block retries absorb; its inputs are unpredictable; events are recorded for attribution instead |
| Strict-by-default rejection of the btauth `<Bytes>` wrapper | report D | parity: an rtao miner accepts every request a Python miner accepts (configurable; the user chose this default, Q4) |

## 7. What a subnet builder writes

### 7.1 Manifests for the three profiles

From [architecture §2.6](docs/plan/architecture.md#26-what-a-subnet-owners-manifests-look-like); the repository URL
is the decided one (Q5), and the rtao tag is illustrative.

```toml
# validator/Cargo.toml: hotkey only; no Perl, OpenSSL or libsodium
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
# miner/Cargo.toml: btauth/1 server; no Perl, OpenSSL or libsodium
[dependencies]
rtao  = { git = "https://github.com/sam0x17/rtao", tag = "rtao-v1.0.0", default-features = false, features = ["miner", "redis"] }
axum  = "0.8"
tokio = { version = "1.53", features = ["rt-multi-thread", "macros", "signal"] }

[patch.crates-io]
w3f-bls = { git = "https://github.com/RaoFoundation/subtensor.git", tag = "v477" }
```

```toml
# owner-tool/Cargo.toml: coldkey operations; builds the core with `host` (needs Perl and make)
[dependencies]
rtao = { git = "https://github.com/sam0x17/rtao", tag = "rtao-v1.0.0", default-features = false, features = ["owner", "ledger"] }

[patch.crates-io]
w3f-bls = { git = "https://github.com/RaoFoundation/subtensor.git", tag = "v477" }   # = rtao::core::BITTENSOR_CORE_TAG
```

In a workspace that holds all three, the patch line appears once, in the workspace root, because Cargo ignores
`[patch]` in member manifests. Neuron images are built with `cargo build --release -p sn42-validator` so the owner
tool's `host` feature does not unify into them.

### 7.2 A validator

The user writes scoring and a weights proposal; the runtime does metagraph sync, scheduling, commit-reveal, reveal
watching, persistence, metrics and shutdown
([architecture §9.2](docs/plan/architecture.md#92-what-a-validator-author-writes)):

```rust
#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cfg = rtao::neuron::NeuronConfig::load()?;                          // flag > env > ~/.bittensor/btcli.json > default
    rtao::neuron::NeuronRuntime::validator(cfg, MyValidator::default()).run().await
}

impl rtao::neuron::Validator for MyValidator {
    type State = rtao::neuron::EmaScores;                                   // keyed by hotkey, never by uid alone
    async fn step(&self, cx: &NeuronCx, s: &StateCell<Self::State>) -> anyhow::Result<()> {
        let mg = cx.metagraph.load();                                       // Arc<Metagraph>; O(1) by_hotkey
        let replies = cx.http.fan_out(mg.axon_targets(), make_request, FanOut::default()).await;  // btauth/1-signed
        s.write().update(&mg, &replies);                                    // a !Send guard cannot cross the next .await
        Ok(())
    }
    fn weights(&self, _cx: &NeuronCx, s: &Self::State, _mech: MechId) -> WeightsProposal { s.to_proposal() }
}
// Typed one-offs: cx.executor.execute(&ServeAxon { .. }, &cx.hotkey, &Default::default()).await?
// `cx.executor.execute(&AddStake { .. }, &cx.hotkey, …)` does not compile: AddStake::Role = Coldkey.
```

`StateCell` wraps a `std::sync::RwLock`, whose guards are `!Send`: holding one across an `.await` inside `step` does not
compile. `step` runs in its own task, paced by `step_policy()`; an error or a panic is counted, backed off and flips
`/readyz` after 5 in a row, but never ends `run()`. Behind `weights()`, the `WeightScheduler` reads 17 storage items in
one RPC at the best head's hash, as Python does, refuses only on certain chain rejections (including a missing
validator permit on a Null-consensus subnet), conforms the row exactly as Python does, encrypts to the drand round of
the predicted reveal block, submits with era 128 and finalization, reconciles a lost watch, follows the commit's
actual reveal epoch, and attributes any missed reveal to its cause.

### 7.3 A miner

The user writes an axum router; the runtime wraps it in btauth/1 verification, the metagraph caller policy (registered,
validator permit, stake floor) and a per-hotkey rate limiter, and publishes the axon idempotently
([architecture §9.4](docs/plan/architecture.md#94-miner), [§7.2](docs/plan/architecture.md#72-server-adapter-feature-server-axum-08-tower-05-http-1)):

```rust
use axum::{routing::post, Json, Router};
use rtao::btauth::server::VerifiedCaller;
use rtao::neuron::{Miner, NeuronConfig, NeuronCx, NeuronRuntime, StateCell};

#[derive(Default)]
struct MyMiner;

impl Miner for MyMiner {
    type State = ();
    fn router(&self, _cx: &NeuronCx, _state: StateCell<Self::State>) -> Router {   // called after on_start
        Router::new().route("/generate", post(generate))   // wrapped in BtAuthLayer + MetagraphPolicy + PerHotkeyLimiter
    }
    // caller_policy() defaults to MetagraphPolicy::validators(cx.metagraph.clone())
}

async fn generate(VerifiedCaller(caller): VerifiedCaller, Json(req): Json<Prompt>) -> Json<Reply> {
    tracing::info!(hotkey = %caller.hotkey, "btauth/1 request verified");   // 401/403/413/429 never reach this handler
    Json(answer(req))
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    NeuronRuntime::miner(NeuronConfig::load()?, MyMiner).run().await       // [I] named by analogy with ::validator
}
```

Request bodies are buffered (4 MiB default [I]) because the signature covers the body hash; responses may stream. The
runtime wraps the router as `router.layer(DefaultBodyLimit::max(cfg.body_limit)).layer(BtAuthLayer { .. })`, because
axum's `Json`, `Bytes` and `String` extractors otherwise cap bodies at 2 MiB, and the layer runs authenticate → policy
→ commit → limit, so unregistered keys cannot fill the replay store and a replayed request cannot spend a validator's
rate-limit tokens (D-032). Handlers receive the miner's state through the `StateCell` passed to `router`, which the
runtime calls after `on_start`. Because `bittensor-core` and rtao turn on `serde_json/arbitrary_precision` for the
whole build, `#[serde(untagged)]` and `#[serde(flatten)]` request types that carry non-integer numbers fail to parse
directly from text (defect #10 in [§5.8](#58-known-core-defects-and-how-rtao-handles-them)). For such types, use
`rtao::btauth::server::Json<T>`, which parses through `serde_json::Value` first, instead of `axum::Json<T>`; the
template and the miner guide do.

### 7.4 An owner operation

Owner operations are intents signed by the coldkey, with a dry-run plan before submission
([architecture §4.6](docs/plan/architecture.md#46-executor-plan-policy-rtao-tx-port-of-pyexecutorpy-1319-lines),
[§9.7](docs/plan/architecture.md#97-owner-operations)):

```rust
use rtao::intents::{AnyIntent, SetMechanismCount};
use rtao::prelude::*;                                                       // contents per feature: §4.4

async fn configure(executor: &Executor, owner: &Wallet) -> anyhow::Result<()> {
    // Typed path: Wallet is KeySource<Coldkey>; SetMechanismCount's catalog signer is coldkey, origin subnet_owner.
    let op = SetMechanismCount { netuid: NetUid(42), mechanism_count: MechId(2) };
    let plan = executor.plan(&op, owner, &PlanOptions::default()).await?;  // fee, effects, warnings, violations; nothing signed
    anyhow::ensure!(plan.ok(), "policy violations: {:?}", plan.violations);
    executor.execute(&op, owner, &ExecuteOptions::default()).await?        // era 128, waits for finalization
        .raise_for_failure()?;

    // JSON path (CLI, agents, execute_tool): Python's validation, Python's result JSON.
    let hp = AnyIntent::from_tool("set_hyperparameter",
        serde_json::json!({ "netuid": 42, "name": "immunity_period", "value": 5000 }))?;
    let result = executor.execute_dyn(hp.as_dyn(), owner, &ExecuteOptions::default()).await?;
    println!("{}", result.to_json());                                       // == Python's ExtrinsicResult.to_dict()
    Ok(())
}
```

The framework adds `OwnerOps` (create subnet with the queued-registration follower, start, hyperparameters through a
planner that waits for the admin freeze window and the per-setter owner rate limits, identity, symbol, mechanisms,
trims) and `OwnerMonitor`.

### 7.5 The `cargo generate` template

`templates/subnet/` in the rtao repository ([architecture §2.1](docs/plan/architecture.md#21-repository-tree),
[§9.11](docs/plan/architecture.md#911-persistence-shutdown-upgrades-deployment)) ships:

- a workspace with a validator, a miner and an owner tool, the `w3f-bls` patch line once in the workspace root (never
  the core patch of [§5.10](#510-the-pr-u17-core-patch)), and
  `rtao doctor` in CI; each binary installs ring as the process-default rustls provider first thing in `main`, as a
  backstop for third-party TLS code, and a test asserts it;
- a multi-stage Dockerfile: neuron images are built with `-p <binary>` and BuildKit cache mounts for the cargo git and
  registry directories, in `rust:1.89-slim` as shipped (its C compiler links the binary and compiles `ring` and
  `blake3`); only the owner image adds `perl` and `make`;
- a non-root runtime image with a read-only hotkey mount and a state volume; a systemd unit (`Restart=on-failure`,
  `KillSignal=SIGTERM`, `TimeoutStopSec=90`);
- a docker compose file for the localnet loop and a CI workflow.

It lands in M3 with the validator and miner members, their no-host Dockerfile stages, compose and CI; a generated
project must pass its own CI ([§12.5](#125-m3-miner)). The owner-tool member (features `["owner"]`, plus the owner
image stage) joins in M4, and `ledger` in M7. Invocation [I]:
`cargo generate --git https://github.com/sam0x17/rtao templates/subnet`.

## 8. Python parity

### 8.1 Definition of done

Parity is reached at M8 when all of the following hold ([architecture §4.9](docs/plan/architecture.md#49-parity-enforcement-the-machinery),
[§14](docs/plan/architecture.md#14-milestones)):

- `parity/pending.toml` is empty: all 83 intents and 102 reads are implemented; `AnyIntent` and `ReadName` have no
  `NotYetPorted` stubs.
- The oracle matches 83/83 intents (composed call trees, effects, warnings, blocks, violations, spend, fee; including
  the root-subnet samples of `vendor/oracle/samples_extra.json`) and 102/102 reads (JSON values with integers compared
  exactly and non-integers as f64, at head and pinned), plus the pure-function vectors.
- Interop read JSON is value-equal to Python's at a pinned block for every sample; `rtao tx <op> --dry-run --json` is
  value-equal to `btcli`'s for every state-independent sample.
- The `cli_tree` parity row set has zero missing commands and flags, and every hand-written command's `--json` output
  (`--dry-run --json` for mutating ones) is value-equal to btcli's at a pinned block (interop lane 9), or the
  difference is a registered divergence.
- `xtask parity check` runs in strict mode: zero unexplained rows in every layer.
- Every entry in `parity/divergences.toml` has a pinning test and, where upstream is wrong, an upstream link.
- Every upstream e2e manifest name and every `PYT/e2e` test (key `pyt:e2e/<file>::<function>`; 115 keys at v477) is
  mapped in `parity/e2e_map.toml` to a green rtao test or `na`, with no `pending` key left; the retired
  `intent_set_root_claim_type` is mapped as n/a
  ([inventory §8](docs/plan/parity-inventory.md#8-inconsistencies-found-and-how-this-inventory-resolves-them) #8,
  #10).

Not claimed: byte-identical human tables (Rich in Python) or byte-identical JSON text; JSON is value-equal, and
`Balance` strings are byte-equal ([architecture §10.3](docs/plan/architecture.md#103-output-contract)).

### 8.2 Scope and counts per milestone

Upstream surface at `parity_tag`, re-checked against `CAT/`: 83 intents (75 coldkey / 8 hotkey signers; 74 signed /
7 subnet_owner / 2 root origins; 139 wrap entries over 116 distinct calls), 102 reads in 14 categories (81 with one
chain hit, 11 with several, 10 with none), 18 semantic error codes and 390 classified chain error names, an e2e
manifest of 112 names (73 `intent_*`, 39 `test_*`), and 3 Python e2e tests in `PYT/e2e/`. The CLI has 163 hand-written
leaf commands (160 in 19 groups, plus the root commands `tools`, `explain` and `call`) besides the generated `tx` and
`query` trees.

New items per milestone, cumulative in parentheses, from
[parity-inventory.md §1](docs/plan/parity-inventory.md#1-summary-by-milestone):

| Milestone | Intents | Reads | Error codes | CLI commands (of 163) | CLI groups complete (of 19) |
|---|---:|---:|---:|---:|---:|
| M0 | 0 (0) | 0 (0) | 0 (0) | 0 (0) | 0 (0) |
| M1 | 0 (0) | 2 (2) | 0 (0) | 0 (0) | 0 (0) |
| M2 | 9 (9) | 13 (15) | 18 (18) | 0 (0) | 0 (0) |
| M3 | 0 (9) | 1 (16) | 0 (18) | 0 (0) | 0 (0) |
| M4 | 11 (20) | 22 (38) | 0 (18) | 40 (40) | 3 (3) |
| M5 | 63 (83) | 2 (40) | 0 (18) | 20 (60) | 2 (5) |
| M6 | 0 (83) | 62 (102) | 0 (18) | 24 (84) | 4 (9) |
| M7 | 0 (83) | 0 (102) | 0 (18) | 34 (118) | 3 (12) |
| M8 | 0 (83) | 0 (102) | 0 (18) | 45 (163) | 7 (19) |
| M9 | 0 (83) | 0 (102) | 0 (18) | 0 (163) | 0 (19) |

Of the 46 generated pass-through `build()` bodies, 1 is an M2 intent (`transfer_all`), 9 are M4 intents and 36 are M5
intents. The generator (`generated/passthrough.rs`) lands in M2, so each body is generated when its intent lands and
none is hand-written first. The inventory re-derived the set from the `build()` bodies: the count stays 46, with
`transfer_all` in and `multisig_threshold_1` out compared to the heuristic behind the architecture's figure
([inventory §4](docs/plan/parity-inventory.md#4-generated-pass-through-bodies-which-46)). Only 19 of the 46 have no
hand-written hook besides `summary()`.

### 8.3 How parity is enforced

`cargo xtask parity check` runs offline in `pr-gates` and fails on any unexplained difference
([architecture §4.9](docs/plan/architecture.md#49-parity-enforcement-the-machinery)):

| Mechanism | What it is | What it catches |
|---|---|---|
| `parity/map.toml` (file layer) | one row per watched upstream file: monorepo path, Rust paths, kind (`generated`, `extracted`, `vendored`, `ported`, `oracle-only`, `n/a`), git blob SHA when last reconciled, tag. Every row is seeded `pending` in M0 (357 at v477); presence is checked for every row and blob equality only for blessed rows. Watched roots go beyond the package: the Python tests, scripts, completions and `pyproject.toml`, `sdk/bittensor-core-py`, and the runtime and pallet sources whose rules the framework mirrors; reads come from the vendored `vendor/upstream/tree.txt`, so ordinary pushes need no network | a new upstream file without a row; a vanished file; any blob change of a blessed row, shown as a diff. Catches behavior-only Python changes that leave the catalogs alone (v466→v467 rewrote 142 lines of `_root_claim_fee.py` with `intents.json` unchanged) |
| symbol layer | `ruff_python_parser` diff of old and new blobs (names, classes, dataclass fields and defaults, `ClassVar`s, functions and parameters), joined to Rust items through the `#[doc(alias = "bittensor.…")]` index | "Python `X` changed → Rust `crates/…:line`", or "no Rust counterpart" |
| catalog layer | generated registries vs vendored `CAT/*.json`: names, classes, signer, origin, verify, wraps, normalized schemas; read params (with the extracted `read_params.json` defaults), docs, categories, hits (the catalog hits must be a subset of each read's hit set, [§9.3](#93-fakes-at-the-right-layers)); error codes and names; extracted defaults vs generated `defaults.rs` | catalog drift, schema mismatch |
| extraction layer | `xtask extract --check` re-extracts every literal table from the vendored sources (settings, error maps, retry substrings, shield bounds, RPC policy constants, the hyperparameter table, `PROXY_TYPES`, samples). Beyond literals it accepts seven resolved forms: constant folding, module-level names, environment defaults, descriptor chains (`st.<Pallet>.<Item>`), `default_factory` lambdas, builtin type tags and `Path.home()` paths; an M1 test checks that every named table extracts | a hand-copied constant drifting; a table restructured into non-literal code (fails with file and line) |
| chain-source layer | `syn` over runtime sources: custom transaction-error codes, `call_filter_group!` fee filters (Python mirror must equal it), `SelectiveMetagraphIndex` arms, `freeze_struct` hashes, `ProxyType` vs `PROXY_TYPES`, `MAX_SHIELD_ERA_PERIOD` (into `rtao_catalog::rt`), the `prod_or_fast!` rate limits absent from metadata (into `rt::rate_limits`), and the chain rules the framework mirrors (tempo bounds, freeze window, rate-limit arms) into `chain_rules.json` | runtime changes that Python or the framework mirrors by hand, including code-only changes that produce no metadata drift |
| coverage gate | every call of Python's 13 covered pallets is wrapped by some intent, listed in Python's `RAW_ONLY`, or in `raw_only_extra.toml` | a new chain call nobody decided about |
| oracle layer | committed `vendor/oracle/*.json` compared in ordinary `cargo test` | any behavioral difference in plans, composed calls, reads, pure functions |
| completeness | `pending.toml` may only shrink (except when a pin bump, the change that moves `[parity]`, adds new upstream ops); every divergence needs a test and an upstream link; every `__all__` name of `PY/__init__.py` has a module-map row or a `#[doc(alias)]`, and every name of the 14 other public `__all__` lists (`intents`, `reads`, `evm`, `extension`, `vault`, `http_auth.py`, `keyfiles.py`, `ledger.py`, `timelock.py`, `wallet.py`, …) has a `#[doc(alias)]` or a reasoned row in `parity/exports_extra.toml`; strict from M8 | silent scope loss, including library API that the oracle never calls |
| environment layer | `parity/env.toml` maps every environment variable the Python package reads (28 names plus `BT_PW_*` at v477, extracted with file and line) to its rtao consumer or a divergence | a new, renamed or vanished environment variable |
| bless and auto-bless | `xtask parity bless <py-path>` records the new blob SHA after a human port; a pin bump auto-blesses only when the symbol diff, the extracted tables and the mapped oracle vectors are all unchanged; catalog blobs are compared normalized (without `line`, `end_line` and `url`), so a source-line shift auto-blesses; a `vendored` blob is auto-blessed only when its consumers' tests pass | keeps 2-4 releases a week manageable |
| early warning | nightly diff of monorepo `main` and `testnet` against `parity_tag` (catalogs, watched files, symbols) | upstream changes days before a tag ships |

### 8.4 Registered divergences

The oracle accepts only listed divergences; each has a pinning test
([architecture Appendix C](docs/plan/architecture.md#appendix-c-divergence-register-initial-paritydivergencestoml)).
Classes: `stricter` (rtao refuses something Python allows), `improved`, `kept` (a Python quirk reproduced on purpose),
`addition` (no Python counterpart), `typed-only` (the Rust type differs, the JSON does not).

| ID | Upstream symbol | rtao behavior | Class |
|---|---|---|---|
| D-001 | zero-account defaults in `hotkey_owner`, `miner_collateral` (+ `Keys`, `SubnetOwner`, `SubnetOwnerHotkey`) | `DefaultAccount::get()` is `None` for the zero account; JSON unchanged; applied by rule over the pinned metadata (offline test; scenario 15 is a live sanity check) | typed-only |
| D-002 | four unpinned gather reads | pinned to one block | improved |
| D-003 | library password prompting | the library never prompts; the CLI prompts on a TTY | improved |
| D-004 | connect | signing refused on a genesis mismatch for named networks | stricter |
| D-005 | `NonceStore` | async (`BoxFuture`) | addition |
| D-006 | epoch predicate (core and Python) | pallet predicate `> tempo` with the counter cap | improved |
| D-007 | `Snapshot.constant` | read from the snapshot's runtime, not the head's | improved |
| D-008 | `attach_signature` | also refuses on a `transaction_version` change | stricter |
| D-009 | btauth retention check | `ConfigError` at `Verifier::new` instead of after a verified signature | stricter |
| D-010 | V14 runtimes | read-only; runtime APIs only through the legacy table | stricter |
| D-011 | *withdrawn* (number kept) | Python never retries shielded submissions either, so there is no divergence | — |
| D-012 | head streams | resubscribe and backfill by number (`blocks_strict` keeps Python's) | improved |
| D-013 | `execute` across an upgrade | refuses spec-mismatched `CallBytes` and re-plans once | stricter |
| D-014 | `btcli query` | `rtao query <read> --block <n>` | addition |
| D-015 | RPC session | client-side token bucket on public presets | addition |
| D-016 | framework weight path | refuses when `CommitRevealWeightsVersion ≠ 4`; intents unchanged | addition |
| D-017 | `ServeAxonTls` | certificates over 65 bytes rejected client-side | stricter |
| D-018 | docs claim a hotkey-shield refusal | follows the code (no refusal) | kept |
| D-019 | `Metagraph.total_stake` | `StakeWeight` type; JSON unchanged | typed-only |
| D-020 | `execute_proxy_announced` | builds the inner intent against the signing wallet, as Python does | kept |
| D-021 | `Policy.allowed_netuids` | intents without a `netuid` field pass any allowlist, as in Python | kept |
| D-022 | `limit_price` call arguments and `add_stake_burn.limit` (call 132) | `PriceRao` instead of `TaoBalance` | typed-only |
| D-023 | `ExtrinsicResult` | adds `inclusion` and `fate_unknown`; `to_json()` unchanged | addition |
| D-024 | watch statuses | `retracted` keeps watching, then reconciles; `finalityTimeout(h)` resolves from block h; `dropped` reconciles when `reconcile` is on (Python treats all three as fatal) | improved |
| D-025 | shielded submission era | refused with `TxError::ShieldEraExpired` when B + 2 ≥ F + p (Python submits and fails) | stricter |
| D-026 | shielded submission nonces | `lease_pair` leaves the next nonce at n + 2 (Python leaves its cache at n, so the next submission reuses n + 1) | improved |
| D-027 | `SyncClient` namespaces | all 14 namespaces (Python's has 13, without `collateral`) | improved |
| D-028 | nested module errors | resolved through the block's runtime, as top-level errors are (Python uses its generated table) | improved |
| D-029 | `btcli tools` | adds a `rust` object per tool; the 9 upstream keys unchanged | addition |
| D-030 | `btcli tx` | `--period <n>` | addition |
| D-031 | `--signer` | adds `remote` (lands with `RemoteSigner` in M10) | addition |
| D-032 | btauth server adapter | authenticate → policy → commit → limit: a replayed request from a denied hotkey gets 403 where the recipe gives 401; `Verifier::verify` keeps Python's order | addition |
| D-033 | `timelock.encrypt` | adds `Reveal::InBlocks(n)` | addition |
| D-034 | `multisig_approve` plan | adds a warning when the approval is at the threshold and cannot execute the call | addition |

## 9. Testing strategy

### 9.1 Layers

([architecture §11.1](docs/plan/architecture.md#111-layers))

| Layer | What it proves | Runs | Needs |
|---|---|---|---|
| unit and property | units, `conform`, btauth canonicalization, eras, nonce leases, shape hashes, typed == dynamic decoding | every push | nothing |
| upstream goldens | chain-defined bytes reproduced through rtao's wrappers, not only the core | every push | nothing (metadata embedded in the fixtures) |
| oracle vectors | Python behavior: composed calls, plans, reads, pure functions | every push (committed JSON) | nothing; Python only to regenerate |
| `FakeNode` | the session supervisor: reconnect, resend, policy refusals, `FateUnknown` | every push | a loopback socket |
| `FakeChain` | client, executor, reads, scheduler under a paused clock | every push | nothing |
| localnet e2e | real chain behavior | every push except docs-only, nightly, weekly | docker |
| `core-oracle` | rtao and `bittensor_core::Client` agree where they should | nightly; pushes that change `upstream.toml` (pin bumps) | docker |
| Python interop | cross-language wire bytes, wallets, btauth/1, JSON | nightly; pushes that change `upstream.toml` (pin bumps) | docker (Python only inside containers) |
| Ledger emulator | the Ledger path against the real Polkadot generic app under Speculos: address, clear signing with the core's proof, rejection and tampering, Ledger-signed extrinsics accepted by a localnet | nightly; pushes that change the Ledger paths or `upstream.toml`; re-recordings | docker (Speculos runs Python inside its image) |
| mainnet clone | the read-only suite against a clone of mainnet state | weekly | free `ubuntu-latest` with Docker; resource feasibility must be measured (Q23) |
| fuzz, bench | robustness; performance regressions | nightly | nothing |
| live probes | public endpoints and finney's shape | every 6 hours | network |

Localnet, interop, core-oracle and Ledger emulator tests live in the `e2e`, `interop`, `core_oracle` and `ledger_emu`
`[[test]]` targets of `rtao-testkit`, each gated by `required-features`; the nextest `ci` profile filters them out, so
the `--all-features` `pr-gates` run and the separate local macOS verification never start them.

### 9.2 Upstream fixtures reused

Vendored at `parity_tag` with sha256 pins and a NOTICE ([architecture §11.2](docs/plan/architecture.md#112-upstream-fixtures-reused-vendored-at-parity_tag-sha256-pinned-apache-20-with-notice)):

- `PYT/fixtures/golden.json` (spec 461, embeds V15 and V14 metadata): 10 `calls` re-encoded through the typed builders,
  10 `storage_keys`, signature payloads (immortal and mortal 64), 2 extrinsics, 3 multisig and 4 ss58 vectors, and 3
  `constants` decoded through `ViewExt::constant`; storage values, maps, runtime calls, events, block, payment info and
  receipt served by `FakeChain` and reproduced by rtao's dynamic reads, typed models, receipt resolution and fee
  decode.
- `PYT/fixtures/shape_corpus/corpus.json` (797/802 types, 2,899 samples): the core replay re-run at `core_tag`, rtao's
  visitors, and every typed model projected to `Value`.
- `ledger_proof_vector.json` (Ledger request construction), the core's private epoch-schedule vectors (transcribed to
  JSON by `xtask import-upstream`), the RFC-0078 digest, PoW seals, the shield key hash, btauth/1 vectors
  (`PYT/unit/test_http_auth.py`, `signed-requests.mdx`), keyfile goldens plus encrypted cross vectors recorded once in
  the interop lane: `$NACL` from btcli, legacy Fernet from `cryptography.fernet`, Ansible from `ansible-vault`.
- Table-shaped Python unit tests (`test_null_weights`, `test_slippage_protection`, `test_root_claim_fee`,
  `test_lock_ownership_gate`, `test_hyperparams_units`, `test_fee_filters`, `test_multisig_safety`, `test_prepare_attach`,
  `test_wallet_compat`, `test_balance_properties`, `test_rpc`, `test_http_auth`) and the behavior tests the oracle does
  not reach (`test_v446_accounting_and_commitments` in M1 and M6; `test_pow_registration`, `test_cli_null_pruning` and
  `test_natural_name_key` in M4; `test_facades` in M6; `test_ledger_signer`, `test_extension_bridge`,
  `test_extension_signer` and `test_evm` in M7) are ported test by test with `// pyt:` provenance, 21 of the 34 files;
  their inputs are extracted, not copied.
- `PY/_generated/*.py` as a codegen cross-check: rtao's descriptors must agree with Python's generated ones.
- Spec-current fixtures come from upstream's own recorders (`record_golden.py`, `record_shape_corpus.py`), run by
  `xtask record` through rtao's `ci/oracle/record.py` wrapper in the oracle image against the `:v<spec>` localnet, into
  `vendor/recorded/<spec>/`. The wrapper drops recorder cases absent from the live metadata and lists them in
  `dropped.json` (today `SubtensorModule.Alpha`, which `record_golden.py` hard-codes; an upstream fix replaces it with
  `AlphaV2`).

### 9.3 Fakes at the right layers

([architecture §11.3](docs/plan/architecture.md#113-fakes-at-the-right-layers-j-pd-concern-5))

- **`FakeState`**: metadata, runtime versions, scripted heads and forks, storage, runtime-API responses, submit scripts
  (including `Retracted` and `FinalityTimeout`), event scripts and finality lag. Seeds are typed descriptors,
  Python-shaped JSON through the core's lenient encoder, the extracted `DEFAULT_STORAGE`/`DEFAULT_CONSTANTS`/
  `DEFAULT_RUNTIME` of `PYT/harness/fake_substrate.py`, or the per-op and per-read `FakeSubstrate` state the oracle
  dumps into `vendor/oracle/seeds/` (loaded by `FakeState::from_python_seed`, dumped fee included), so oracle inputs and
  rtao inputs are identical.
- **`FakeNode`**: a real JSON-RPC WebSocket server on `127.0.0.1:0` with fault injection (handshake 403/429 with
  `X-RateLimit-*` headers, drops after N frames, close 1008, `-32004`/`-32005` errors, silence, state discarded, slow
  responses), over `ws://` and, with a self-signed certificate added through `SessionConfig::extra_roots`, `wss://`.
  It runs the port of `PYT/unit/test_rpc.py` and the 19-row Python transport checklist
  ([architecture §3.15](docs/plan/architecture.md#315-python-transport-parity-checklist)).
- **`FakeChain`**: implements `RpcClientT` directly over `FakeState`, and a `Client` runs over it through
  `Client::from_rpc`; rtao's runtime cache, key building, decoding and submission state machine run for real;
  `tokio::time::pause` drives schedulers.
- **`RecordingView`**: wraps any view and records every descriptor and block hash used. Two tests use it:
  - *exact hits*: each read has a hit set H, the helper-aware hits that `xtask extract` scans from the read and the
    module-local helpers it calls, plus the reviewed `parity/read_hits_extra.toml` for items reached outside its module
    (view methods, `fetch_commitments`), each row with a Python citation. Every recorded run of the read touches a
    subset of H, and the union over its samples, including per-branch samples, equals H. The catalog hits must be a
    subset of H; `ReadSpec.hits` and `verify_surface` use H. As an independent cross-check, the oracle records the
    items `FakeSubstrate` served per sample (`touched`), and `xtask oracle` fails if one falls outside H. This lands in
    M2, before the first read enters the test, and PR-U16 proposes the helper-aware scan upstream;
  - *single block*: every multi-RPC read uses one block hash (D-002).

### 9.4 Localnet e2e

([architecture §11.4](docs/plan/architecture.md#114-localnet-e2e-docker))

- Harness (`rtao-testkit::Localnet`, M2): the plain `docker` CLI, as upstream does; readiness by healthcheck or
  `Imported #1`, then `Drand.LastStoredRound > 0` within 30 s (commit-reveal and timelock depend on pallet-drand
  fetching quicknet pulses over public HTTPS; on timeout `LocalnetError::DrandUnreachable`, and the job retries once
  with a fresh container); the e2e job first checks egress to the drand endpoint; upstream's env contract
  (`E2E_ENDPOINT`, `LOCALNET_IMAGE`, `SKIP_PULL`); images pinned by digest from `upstream.toml`.
- Lanes: `pinned` (`:v<parity spec>`, fast and slow, on every push except docs-only, behind `e2e-required`, the e2e
  stop-the-line gate), `mainnet` (pushes, blocking; nightly), `testnet` and `devnet` (nightly, allowed failure; a
  testnet failure opens or updates a "pre-break" issue), `main` (weekly, allowed failure). `wasm-override` is a manual
  fallback, not a lane, because upstream publishes `:v<spec>` on every mainnet mirror move. Fast blocks (250 ms,
  tempo 10) by default, with commit-reveal scenarios at tempo 48 so that one late drand pulse cannot expire a commit;
  a 12 s lane for timing-sensitive tests.
- Isolation: `e2e.yml` starts the containers itself, one per nextest partition plus one `global` container. Per
  container, `cargo xtask localnet bootstrap` sets the global knobs once as Alice (admin freeze window, owner, network
  and transaction rate limits, start-call delay, lock cost and reduction interval, subnet limit) and funds 64 slot
  accounts. Each test signs only with the accounts of its `NEXTEST_TEST_GLOBAL_SLOT`, gets a fresh subnet from the
  `Scenario` builder, and sends every Alice (sudo) transaction through `Localnet::sudo()`, which holds a file lock from
  the nonce fetch until inclusion. Tests that change global state (scenarios 6, 8, 10's queued path and planner, and
  16) run in test group `global` against the `global` container.
- Scenarios (rtao-specific, numbered as in the architecture): (1) validator with commit-reveal for 20 fast and 3 slow
  epochs, every reveal observed; (2) commit-reveal off; (3) two mechanisms; (4) Null consensus and trim-null batches;
  (5) owner bumps `WeightsVersionKey` mid-run; (6) the commit-reveal version gate; (7) a lost watch reconciled with
  correct later nonces; (8) a runtime upgrade mid-session (M2): sudo `System.set_code` with the fast-runtime wasm
  copied out of the `[localnet].main` image by digest while a validator runs, asserting the codec switch, surface
  re-verification, the scheduler's pause and resume, and one re-plan (D-013); `frame_system` refuses a spec that does
  not increase, so the test is skipped with its printed reason when `:main` is not ahead of the pinned spec, and the
  offline-payload refusal joins in M5; (9) a miner (idempotent `serve_axon`, 401/403/413/429);
  (10) owner flows: immediate subnet creation, and the queued path, forced in the `global` container by root
  `sudo_set_network_immunity_period(0)` then `sudo_set_subnet_limit(<current non-root subnet count>)`, so the next
  `register_network` prunes the lowest-priced subnet and emits `NetworkRegistrationQueued`, and the follower waits for
  that registration's `NetworkAdded`; plus the planner waiting for the freeze window and per-setter rate limits;
  (11) shielded `burned_register` (M4) and `add_collateral` (from M5); (12) multisig, pure and announced proxies,
  `batch_all` revert, raw-call refusal; (13) offline prepare, sign and submit, refused across an upgrade; (14) PoW
  registration on the 12 s lane; (15) the D-001 sanity check (the override itself is applied by rule and tested
  offline); (16) PR-U4 evidence from `ci/pr-u4-probe/`, a separate workspace: crates.io-`w3f-bls` ciphertexts
  decrypted offline by rtao and on localnet by pallet-drand; (17) 50 pipelined transfers from one account through two
  `Executor`s built from one `Client`, with no nonce gap or collision; (18) residual ops (M5): `move_swap_stake`,
  `stake_into_basket`, `swap_basket`, `swap_basket_many`, `set_reject_locked_alpha`, `set_min_collateral` and, signed
  by Alice as sudo, `set_root_claim_threshold` and `set_subnet_emission_enabled`, each with its chain effect asserted;
  (19) a timelocked commitment (M7), a port of `PYT/e2e/test_timelock_commitment.py`.
- All 112 manifest names and the 3 `PYT/e2e` tests are mapped in `parity/e2e_map.toml` (115 keys); the Python tests map
  to scenarios 4, 14 and 19. Each key holds `rtao = "<target>::<test fn>"` (which `parity check` resolves against the
  test functions of `rtao-testkit`'s `e2e` and `interop` targets), `na = "<reason>"`, or `pending = "M<k>"`; a key
  leaves `pending` in the milestone that ships the code it exercises (the 72 `intent_*` keys in M5; the 39 `test_*` keys
  per [inventory §2.5](docs/plan/parity-inventory.md#25-upstream-e2e-names)), and strict mode (M8) allows none. The
  eight ops without an upstream e2e test are submitted by scenario 18, and the M5 table-driven plan test covers all 83
  ops ([§12.7](#127-m5-intent-parity);
  [inventory §8](docs/plan/parity-inventory.md#8-inconsistencies-found-and-how-this-inventory-resolves-them) #9).

### 9.5 Python oracle and interop

- **Oracle** ([architecture §11.5](docs/plan/architecture.md#115-the-python-oracle-ci-only-outputs-committed)):
  `ci/oracle/Dockerfile` builds the monorepo at `parity.commit` with `uv sync` (which builds `bittensor-core-py` through
  maturin, OpenSSL and Perl), cached per tag in GHCR. There is no PyPI fallback: no PyPI version reliably identifies a
  tag (`sdk/python` stayed `11.3.0.dev0` across v464-v476 while its tree changed). It dumps, for every intent sample
  (plus the root-subnet samples of `samples_extra.json`, M5), the composed call tree, effects, warnings, blocks,
  violations, fee, spend and plan JSON; for every read sample, results at head and pinned and the chain items
  `FakeSubstrate` served (`touched`); for every sample, the `FakeSubstrate` state it ran with
  (`vendor/oracle/seeds/`); pure-function vectors (`normalize`, `clip_to_max_weight`, `_conform` with `.5` ties and the
  cutoff path, hyperparameter conversions, 10,000 `str(Balance)` values, error classification, fee filters, timelock
  rounds, PoW prefix and seal, `metagraph._build`, btauth messages, and from M7 the `vault.uos` framing functions
  `sign_tx_payload`, `sign_frame`, `multipart_frames`, `transaction_frames` and `transaction_frames_with_proof`); from
  M7, the `(payload, proof, account, index)` that Python's `LedgerSigner` sends to its device, recorded through a fake
  device as `PYT/unit/test_ledger_signer.py:34-54` does; and the Typer command tree. Vectors are committed to
  `vendor/oracle/` and compared in ordinary `cargo test`, JSON numbers by kind (integers exactly, non-integers as f64),
  never with `serde_json::Value` equality. Regeneration happens only in pin bumps; if the image cannot build, the
  bump is marked `oracle-stale` (held and logged during the goal loop, C5; a PR label after it) and the previous
  vectors stay authoritative.
- **Interop**
  ([architecture §11.6](docs/plan/architecture.md#116-python-interop-e2e-docker-compose-nightly-and-on-pin-bumps)):
  docker compose with the localnet, the oracle image (`btcli` and a Python with the `bittensor` package) plus the lane
  dependencies pinned by `ci/interop/pyproject.toml` and a committed `uv.lock` (FastAPI, uvicorn, httpx, cryptography,
  ansible-vault), and rtao test binaries. Lanes: (1) btauth/1 both directions, sr25519 and ed25519, against the FastAPI
  recipe; (2) a Python miner with an rtao validator and the reverse, with metagraph JSON equality; (3) wallets created
  by each side and used by the other, including legacy Fernet and Ansible files (Keychain compatibility is a Python-free
  local Mac Studio test in an isolated temporary keychain with system settings restored, outside `cargo xtask gates`;
  it lands with the Keychain code in M4 and remains unresolved until equivalent evidence exists, Q23);
  (4) wire bytes at the current spec; (5) reads at a pinned block, value-equal; (6) prepared extrinsics across
  languages; (7) shared caches, config, the address, proxy and multisig books and the multisig call cache (a raw 2-of-3
  multisig opened by btcli is approved and executed by rtao, and the reverse); (8) CLI `tools`, `--dry-run --json` plans
  and the command tree; (9) every hand-written command's `--json` output (mutating ones with `--dry-run --json`)
  value-equal to btcli's, both CLIs reading one pinned block through `rtao-testkit::PinProxy`, a JSON-RPC proxy that
  answers head queries with that block and adds its hash to hash-less `state_*` requests; wall-clock fields are listed
  in `parity/cli_json_volatile.toml`.
- **`core-oracle`** ([architecture §11.8](docs/plan/architecture.md#118-core-oracle-differential-lane)): on one localnet
  block, rtao and the core's blocking client agree on storage values, runtime-API results, `compose_call` bytes for 21
  of the 22 core constructors (`set_root_claim_type` is retired), and `Policy::check` violation sets.
- **Ledger emulator** ([architecture §11.9](docs/plan/architecture.md#119-ledger-emulator-lane-speculos)): Speculos,
  Ledger's device emulator, runs the Polkadot generic app built from a pinned tag, headless, approved through its REST
  API. The lane checks the device's address against pinned values, clear-signs the core's proof vector and golden calls
  (including a payload above 256 bytes) through `LedgerSigner`, checks rejection and tampering, and submits
  Ledger-signed transfers to the pinned localnet, a shielded one on the 12 s localnet. It records the offline cases as
  APDU transcripts that `ledger_replay` replays in `pr-gates`, and `ledger_request_parity` compares rtao's device
  requests with the oracle's dump of Python's. The lane and `ledger_replay` run the core's own `ledger.rs` through
  `LedgerDevice::with_transport`, which rtao's workspace takes from the core patch until PR-U17 ships (G16,
  [§5.10](#510-the-pr-u17-core-patch)). The manual device run per release covers what the emulator cannot: the HID
  transport and OS device access, the app binary users install, real firmware and the review on a real screen. The
  device side was exercised while planning ([research J](docs/plan/research/J-ledger-ci-shim.md)), and a prototype of
  the core patch drove the app with the core's own `ledger.rs` and replayed the recorded exchanges
  ([research J §8](docs/plan/research/J-ledger-ci-shim.md#8-core-patch-prototype)).

### 9.6 Property tests, fuzzing, benchmarks

([architecture §11.7](docs/plan/architecture.md#117-property-tests-fuzzing-benchmarks))

- `proptest`: money parsing and arithmetic (mixed netuids give `UnitMismatch`), `pyfmt` against oracle vectors,
  fixed-point `to_f64`, `NetUidStorageIndex` round trips, eras against the core's `era_birth`, `IntoCoreValue` round
  trips, `conform` invariants, nonce-lease interleavings (no reuse, no gap), shape-hash soundness under mutations,
  typed == dynamic over a port of upstream's `ByteGen`, btauth freshness boundaries, and "the scheduler never
  double-submits inside a rate limit".
- `cargo +nightly fuzz` (short nightly runs): btauth header parsing and `verify`, JSON-RPC frame parsing,
  `MetagraphView`, event splitting, `UnsignedExtrinsic` import, keyfile detection and `keyfile_json`, core decoders
  wrapped at the boundary.
- `criterion` baselines on committed finney payloads (netuids 1, 3, 64) and a synthetic 2,500-UID payload: the three
  metagraph decode paths, V15 parse, key building for 10k keys, map-page decode, sign and assemble, btauth throughput,
  `conform` at 256 and 2,500 entries. Nightly fails above a 30% regression.
- The nightly `fuzz` and `bench` jobs start in M1 with the baselines and targets that exist then (btauth/1 header
  parsing and `verify` from M0; JSON-RPC frame parsing and `MetagraphView` from M1), and each milestone adds its own
  targets as it lands them: M2 keyfile detection, `keyfile_json` and event splitting; M3 the btauth server path; M5
  `UnsignedExtrinsic` JSON import. M9 reviews the corpora and raises per-target time.

### 9.7 CI lanes and what each catches

Full YAML is in [architecture Appendix E](docs/plan/architecture.md#appendix-e-ci-workflows); triggers and gates are in
[§12.2](docs/plan/architecture.md#122-jobs). During the goal loop there are no PRs (Q21): the agent pushes to `main`, so
push triggers do the work, and the `pull_request` and `merge_group` triggers stay in the workflows, inert until
PR-based upkeep starts after the handover (M9-07). "Blocking" means stop the line: a red run on `main` is fixed before
the next step (rule 2 of [Executing this plan](#executing-this-plan)).

| Lane | Trigger | Catches | Gate |
|---|---|---|---|
| `pr-gates` | every push to `main` (PRs and the merge queue after the loop); `cargo xtask gates` runs the same steps locally before each push | formatting and lint failures (including `disallowed-methods` and `disallowed-types`, and `subtensor-linting` on `rtao-core-ext`); tampered vendored artifacts; stale generated code; stale extracted tables; any unexplained parity row; staged code that upstream now exports (probe crates); core API changes (compile probe); pins out of step with the root manifest or the lock, and a core patch that drifted or is no longer needed (`xtask pin --check` without `--parity`: the pins, offline apart from git, and the core-patch rules of [§5.10](#510-the-pr-u17-core-patch)); layering violations, a second `w3f-bls`, a second TLS provider, host crates in neuron graphs; unit, golden, oracle, `FakeNode` and `FakeChain` failures, and Ledger requests that changed without a re-recording (`ledger_replay`) (nextest `ci` profile, which skips the localnet, interop, core-oracle and Ledger emulator targets); licenses and advisories; MSRV 1.89 breaks (`cargo +1.89` on the facade's profiles and `rtao-cli`); feature-combination breaks and the three profile examples; SemVer changes against the last `rtao-v*` tag | blocking (C1), offline, no Python; the SemVer check is informational before 1.0 |
| `e2e` | every push to `main` (a `changes` job skips docs-only pushes; `templates/` counts as code; one run in progress, the newest push pending), nightly, weekly; PRs and the merge queue after the loop | real-chain behavior on the pinned image (pushes); the `template` job: the generated subnet project built against the pushed commit through a path `[patch]` on the rtao git source, then its own CI steps; regressions against `mainnet` (pushes, nightly), `testnet` and `devnet` (nightly) and `main` (weekly) | blocking through the `e2e-required` gate (C1), which fails only when the `changes` job, a localnet job or the template job failed or was cancelled; `testnet`, `devnet` and `main` allowed failures |
| `core-unpatched` | nightly; pushes to `main` that change `upstream.toml` or the root `Cargo.toml` (`core-unpatched.yml`) | a shipped crate that does not build against the unpatched `core_tag`, which is what subnets build, including through a shipped feature that pulls in `rtao-testkit` ([§5.10](#510-the-pr-u17-core-patch)) | blocking |
| `interop` | nightly; pushes to `main` that change `upstream.toml` (pin bumps); PRs labelled `pin-bump` after the loop (`interop.yml`) | cross-language wire, wallet, btauth and JSON differences | blocking on pin bumps |
| `core-oracle` | nightly; pushes to `main` that change `upstream.toml`; PRs labelled `pin-bump` after the loop (`interop.yml`) | rtao disagreeing with the core where both should agree | alert |
| `ledger-emu` | nightly; pushes to `main` that change the Ledger paths (`ci/ledger/`, `vendor/recorded/ledger/`, `rtao-signer`'s Ledger module, `rtao-testkit::ledger_emu`) or `upstream.toml`; PRs labelled `ledger` or `pin-bump` after the loop; manual dispatch with `record` (`ledger-emu.yml`) | the Polkadot generic app rejecting or mis-signing rtao's Ledger requests; Ledger-signed extrinsics refused by the localnet; fresh transcripts for `ledger_replay` | alert; never required (Speculos runs Python, Q2) |
| `metadata-drift` | every 6 hours; upkeep, off during the loop (below) | a new runtime on finney, testnet, devnet or `:main`, classified and regenerated; no PR for a `(spec, :code)` already recorded on `main` or in an open pin-bump PR | after the loop: PR review; `surface-unchanged` auto-merges |
| `pin-bump` | every 30 minutes (one run at a time; exits early when a PR for the tag is open); upkeep, off during the loop (below) | a new finalized release whose `:vS` localnet image is published: re-vendor, fetch finney's metadata, extract, regenerate, refresh oracle and recordings, porting report, conditional core move (with `xtask core-patch rebase` while the core patch exists), after which the Ledger transcripts are re-recorded (`ledger-stale` on failure) | after the loop, merging needs green `pr-gates`, `e2e-required` on `:vS`, `interop` |
| `oracle-refresh` | inside `pin-bump`; manual dispatch with a tag (`oracle.yml`); its PR is upkeep, off during the loop (below) | regenerates `vendor/oracle/` for a tag | part of `pin-bump` |
| `upstream-next` | nightly | Python, catalog and core changes on monorepo `main` and `testnet` before they ship | informational issues |
| `core-canary` | nightly | core API breaks on monorepo `main` (built without the core patch), plus a smoke run on the `:main` localnet image | allowed failure; opens or updates an issue |
| `fuzz`, `bench`, `features-powerset` | nightly | panics on malformed input; performance regressions; breaks in the depth-2 facade feature powerset (315 runs) | bench fails above 30% |
| `live-probe` | every 6 hours (`probe.yml`) | finality lag, V15 availability, digest pins, live decodes on all three metagraph paths, endpoint policy behavior, a runtime without the compiled metadata hash that Ledger signing needs (subtensor#3159) | alert; updates an issue |
| `mainnet-clone` | weekly, free `ubuntu-latest` (`mainnet-clone.yml`) | read-only suite against a clone of mainnet state; resource feasibility remains a prerequisite if the free runner cannot hold the clone (Q23) | allowed failure |
| `release-plz` | pushes to `main`; upkeep, off during the loop (below), whose versions the plan fixes (C7) | after the loop: opens or updates the release PR (version bump from `cargo-semver-checks` against the last `rtao-v*` tag, changelog) and pushes the tag once it merges | — |
| `release` | tag `rtao-v*`, which the agent pushes at each milestone exit (C7) | — (Linux CLI builds on free `ubuntu-latest` using a pinned glibc 2.35 build container; Linux ARM cross-built, macOS artifacts built and verified locally; `SHA256SUMS`, provenance, `compat.json`, release notes, downloadable rustdoc and book assets; all targets required before publication, Q23) | — |

**Upkeep jobs and tokens.** The jobs that push or open PRs (`metadata-drift`, `pin-bump`, `oracle-refresh`'s PR,
`release-plz` and `ledger-emu`'s `record` commit) stay off during the goal loop: their scheduled and push runs exit
early unless the repository variable `RTAO_UPKEEP` is `on`, and each has a `workflow_dispatch` input `dry-run` that does
everything except pushing and opening the PR, which is how C2 counts them. The agent does their work locally with the
same `xtask` commands and commits the result: pin bumps ([§10.3](#103-a-pin-bump-step-by-step)) and drift regeneration
(`xtask drift`) at the latest at each milestone exit, and oracle refreshes and Ledger re-recording
(`cargo xtask ledger-emu record` on its Docker host) when a bump or a change needs them. Its pushes use the user's
credentials, so they start workflow runs. After the handover (M9-07), the upkeep jobs push branches, open PRs and enable
auto-merge with a GitHub App installation token (rtao-bot; `RTAO_BOT_APP_ID`, `RTAO_BOT_PRIVATE_KEY`), because events
created with the repository `GITHUB_TOKEN` start no workflow runs, so bot PRs would get no CI; if the user wants
PR-based upkeep, branch protection on `main` then requires `check`, `no-host` and `e2e-required`, with
the merge queue. `ledger-emu` is never required (Q2). The read-only scheduled jobs (`live-probe`, `upstream-next`,
`core-canary`, `core-unpatched`, the nightly `fuzz`, `bench`, `features-powerset`, `interop`, `core-oracle` and
`ledger-emu`, and the weekly `mainnet-clone`) stay on during the loop, and the agent reads their open issues at each
milestone exit.

### 9.8 Supported platforms

- Supported release targets: `x86_64-unknown-linux-gnu`, `aarch64-unknown-linux-gnu`, `aarch64-apple-darwin`,
  `x86_64-apple-darwin`. Hosted `pr-gates` run on free `ubuntu-latest` Linux x64 only (Q23).
- Linux release builds use a digest-pinned Ubuntu 22.04 build container on that runner for the glibc 2.35 floor
  (Ubuntu 22.04, Debian 12). ARM requires a cross toolchain and target dependencies; cross/QEMU checks must be
  recorded as such, never as native ARM coverage. This setup and its resource use require validation in M1-23.
- Mac Studio supplies local native Apple ARM checks/builds and cross-built Intel macOS binaries. Record the source
  tag, toolchain, artifact hashes and checks, keeping all disposable work under the recorded task root. macOS
  Keychain parity and trustworthy provenance for locally built release assets remain required and unresolved until
  verified; a Linux hosted job is not evidence that these passed. `cargo xtask gates` changes no host settings.
- Neuron images: linux-gnu, x86_64 and aarch64.
- Unsupported: Windows (v11 ships no Windows core wheels), musl (untested).

## 10. Keeping up with upstream

### 10.1 What changes and how often

Six consecutive finalized release pairs (re-checked with `git diff --name-only` between tags):

| Pair | Files changed | `sdk/bittensor-core/**` | `sdk/python/**` |
|---|---:|---:|---:|
| v466→v467 | 32 | 0 | 7 |
| v467→v468 | 146 | 0 | 9 |
| v468→v469 | 97 | 0 | 6 |
| v469→v473 | 282 | 1 (tests only) | 11 |
| v473→v475 | 263 | 5 | 36 |
| v475→v477 | 93 | 0 | 3 |

Cadence was 9 finalized runtimes in 20 days, and spec 478 changes no client-visible metadata except the
`System.Version` constant ([architecture §12.1](docs/plan/architecture.md#121-what-actually-changes-and-how-often),
[CORRECTIONS.md](docs/plan/research/CORRECTIONS.md) #2). Consequences: Python is the high-churn upstream and often
changes behavior without touching the catalogs, so tracking is source-level plus oracle-level; the core rarely changes,
so `core_tag` rarely moves; and "spec bumped, surface unchanged" is the common runtime case.

### 10.2 Jobs

`metadata-drift` (every 6 h), `pin-bump` (every 30 min), `upstream-next` (nightly), `core-canary` (nightly),
`core-unpatched` (nightly, and on pushes that change `upstream.toml` or the root `Cargo.toml`), `oracle-refresh` (inside
pin bumps), `live-probe` (every 6 h), `mainnet-clone` (weekly), `features-powerset` (nightly), `release-plz` (pushes to
`main`) and `release`; see [§9.7](#97-ci-lanes-and-what-each-catches). The jobs that push or open PRs are upkeep: off
during the goal loop (`RTAO_UPKEEP`), while the agent does their work locally, and on the rtao-bot App token after the
handover (M9-07). Renovate (post-loop upkeep, if the user installs it) proposes weekly third-party updates and never
touches `bittensor-core` or `w3f-bls`, which only `xtask pin` (and, for the core patch, `xtask core-patch`) moves
([architecture §12.4](docs/plan/architecture.md#124-dependency-hygiene)).
`deny.toml` bans duplicate versions of the shared SCALE stack (`parity-scale-codec`, `scale-info`, `scale-decode`,
`scale-encode`, `scale-type-resolver`, `frame-metadata`, `sp-core`) and of `tokio` and `tungstenite`, so a core bump
that splits the stack fails in `pr-gates`, and it bans `aws-lc-rs`, `aws-lc-sys` and `native-tls` outright (one rustls
provider).

### 10.3 A pin bump, step by step

The command is `cargo xtask pin --parity vN`
([architecture §12.3](docs/plan/architecture.md#123-what-a-pin-bump-does-xtask-pin---parity-vn)). During the goal loop
the agent runs each bump itself, at the latest at each milestone exit (C5), and commits it to `main` once
`cargo xtask gates` passes; a bump that cannot complete is held, and the progress log names what blocks it
(`oracle-stale`, `recordings-stale`, `ledger-stale`, `core-patch-conflict`). After the loop the `pin-bump` job runs it
and opens a PR, labelled with the same names:

1. Refuse unless release `vN` is final (not a pre-release), finney's finalized runtime reports spec `N` with the
   expected `:code` hash, and the `:vN` localnet image is published (checked with GHCR's anonymous pull token; the
   digest is written to `[localnet].pinned`). Upstream builds that image after the mainnet mirror push (32 minutes
   after promotion for v477), so the next 30-minute poll retries.
2. Set `[parity]` to `vN`; re-vendor `[artifacts]` from the monorepo (never from the live website, which deploys only
   after mainnet promotion).
3. `xtask extract` refreshes the extracted tables; `xtask regen` refreshes catalogs, interfaces, dispatch, descriptors,
   docs and reference pages. The descriptors come from finney's V15 metadata at spec `N`, which `xtask pin` fetches
   itself and commits with its `index.toml` row; the drift job then opens no PR for that `(spec, :code)`.
4. `xtask oracle` builds the oracle image for `vN` and regenerates `vendor/oracle/`; `xtask record` runs upstream's
   recorders through the `ci/oracle/record.py` wrapper against `:vN` into `vendor/recorded/N/`. They are separate
   steps: an oracle failure marks the bump `oracle-stale`, a recorder failure `recordings-stale`.
5. Compute the core-input fingerprint. If it changed, move `core_tag`. While the core patch exists, run
   `cargo xtask core-patch rebase vN` (a conflict is `core-patch-conflict`), or, when `vN` contains PR-U17, delete the
   two core-patch tables and `[core.patch]` and adapt `rtao-testkit` to the API as merged
   ([§5.10](#510-the-pr-u17-core-patch)). Rewrite every reference generated from `[core]` (the root manifest, the
   template, the README and book snippets; a repository test checks them), run
   `cargo update -p bittensor-core -p w3f-bls -p subtensor-macros -p subtensor-linting` and `xtask core-surface`,
   re-record the Ledger transcripts from M7 on (`ledger-stale` on failure), and mark the bump `core-bump` and
   `downstream-patch-change`.
6. `xtask parity report` writes the porting report: changed blobs per `map.toml` row with diffs, the symbol diff joined
   to Rust items, new ops and reads (added to `pending.toml`), oracle-vector diffs, auto-bless candidates.
7. Commit the bump with the report's summary as the commit body (during the goal loop), or open the PR with the report
   as its body (after it). Tests that fail because Python behavior changed point at the exact op through the oracle
   diff.

### 10.4 Drift classification

`xtask drift diff` compares pallets, calls (with exact shape hashes and V16 deprecations), events, errors, storage
(hashers, shapes, modifiers, defaults), constants, runtime APIs and versions, transaction extensions, extrinsic versions
and `System.Version` ([architecture §12.5](docs/plan/architecture.md#125-drift-classification)):

| Class | Examples | Effect |
|---|---|---|
| `surface-unchanged` | only `System.Version` (spec 478) | index row and spec range updated; after the goal loop the drift PR auto-merges after CI |
| `additive` | new calls, events, errors, storage, runtime APIs; a new call in any pallet stays additive, because wrapper-call shape hashes treat the outer `RuntimeCall` as opaque | regenerated descriptors; the coverage gate forces wrap-or-raw-only decisions |
| `behavioural` | moved event index, storage type or default change, runtime-API version bump, changed projection hash | model review; typed paths on live networks fall back automatically meanwhile |
| `breaking` | moved pallet, call or error index; changed arguments of a wrapped call; removed wrapped call or used storage; a new extension with payload; `transaction_version` change; V15 no longer offered; `spec_name` change; any move toward extrinsic v5 | the extension guard and per-call `verify_surface` refuse affected signing on live networks until the regenerated descriptors ship |

### 10.5 Compatibility matrix and run-time guard

`xtask compat` writes `compat.json` and `COMPAT.md` for every release: rtao version, core tag and commit, parity tag
and Python source version (a PyPI release only if one was published from that commit), validated spec range, metadata
pins, localnet digests, btauth version, interop target. "Validated" means the drift diffs across the range contain no
`breaking` class and e2e plus interop passed on the listed digests.
At run time `Client` compares the connected `(spec, metadata hash)` with the compiled-in range and only warns outside
it; per-call `verify_surface` decides what still works
([architecture §12.7](docs/plan/architecture.md#127-compatibility-matrix-and-runtime-guard)). Releases are git tags
`rtao-vX.Y.Z`. During the goal loop the plan fixes the versions: at each milestone exit the agent sets
`[workspace.package] version` and the changelog, tags `rtao-v0.<k>.0` (`rtao-v1.0.0` for M9) and pushes the tag, which
runs `release.yml` (C7); a fix within a milestone takes a patch tag. After the loop release-plz cuts them (upkeep):
its `cargo-semver-checks` run on the release PR picks the bump, and the drift class is a floor on top (a `breaking`
drift merge bumps the major version; additive drift is a patch). A patch release never moves `core_tag`. The changelog
and `compat.json` name the milestone a release completes
([architecture §12.8](docs/plan/architecture.md#128-release-process)). From M7 on, each milestone's release also needs
a passing manual Ledger device run (Q17, M7-11) on its tag or a later patch tag of the same milestone, recorded in
`docs/ledger-device-run.md`; it covers what the Speculos lane cannot ([§9.5](#95-python-oracle-and-interop)).

### 10.6 Maintenance playbook

([architecture §12.9](docs/plan/architecture.md#129-maintenance-playbook)). During the goal loop the PR-opening jobs
are off ([§9.7](#97-ci-lanes-and-what-each-catches)): the agent runs their `xtask` commands itself and commits the
result, so each "drift PR" and "pin-bump PR" below is the agent's commit until the handover (M9-07).

| Upstream event | Automated | Human work |
|---|---|---|
| runtime bump, surface unchanged | drift PR auto-merges; pin-bump PR with zero generated diff | none, or review the porting report |
| additive surface | regenerated descriptors; coverage gate lists new calls | per new call: wrap it in an intent, type a read, or list it as raw-only |
| behavioural surface | drift PR; model checks pinpoint models; live typed paths fall back | review and fix the flagged models |
| breaking surface | drift PR on devnet days before finney; live guards refuse affected calls | fix the affected calls and descriptors, usually together with a core bump |
| Python adds an intent or read | `upstream-next` issue; `pending.toml` entry and generated interface; oracle shows expected behavior | port each new intent or read until its `pending.toml` entry can go |
| Python changes behavior only | blob change, symbol diff and oracle diff name the ops | port the diff; cosmetic edits auto-bless |
| core API change | `core-canary` saw it nightly; `core-surface` names the symbol | adapt `rtao-core`; no other crate changes |
| a staged PR ships in `core_tag` | `staged --check` fails because the module's probe (a symbol at the ungated target, or its vector suite) now passes | flip the `pub use`, delete the staged module |
| a chain rule mirrored by the framework changes (serving, rate limits, freeze window, tempo control, weights checks) | blob change on a chain-rule row of `map.toml`, or a diff of the extracted `chain_rules.json` | update `AxonPublisher`, `HyperparamPlanner` or `WeightScheduler` and their tests |
| btauth/1 → btauth/2 | blob change on `http_auth.py` or `signed-requests.mdx`; vectors fail | verify both versions, sign the configured one |
| third-party breaking release | Renovate PR (after the loop), or a failing build | adapt the one affected crate |
| a pin bump moves `core_tag` while the core patch exists | `xtask core-patch rebase` cherry-picks PR-U17 onto the new tag ([§5.10](#510-the-pr-u17-core-patch)) | on `core-patch-conflict`, resolve the conflict like any upstream change and log it |
| PR-U17 ships in `core_tag` | `xtask pin --check` fails with "PR-U17 shipped: drop the core patch" ([§5.10](#510-the-pr-u17-core-patch)) | delete the two core-patch tables and `[core.patch]`; adapt `rtao-testkit::ledger_emu` to the API as merged |
| Ledger Live ships a new Polkadot app, or Speculos a new release | the device run records the device's app version; `ledger-emu` runs nightly against the pin | bump `ci/ledger/pins.toml` in a commit that re-records the transcripts |

Upstream tracking is standing work from M2 to the end of the project; each pin bump is one unit of it, and from M2 on
a milestone exits only when `upstream.toml` pins the newest finalized release whose `:vS` image is published, or the
progress log names what blocks that bump (common exit check C5, [§12.1](#121-overview)).

## 11. Documentation plan

([architecture §13](docs/plan/architecture.md#13-documentation))

- **rustdoc**: `#![deny(missing_docs)]` on every library crate; crate READMEs as crate docs; `doc(cfg)` on
  feature-gated items; `#[doc(alias = "bittensor.<dotted.path>")]` on every mirrored item, so searching for
  `SetWeights` or `bittensor.reads.neurons.neurons` lands on the Rust item; `release.yml` attaches it with the book as
  downloadable docs archives on every `rtao-v*` tag (Q23). `rtao-cli`'s
  `[[bin]]` sets `doc = false`: the binary `rtao` and the facade library `rtao` both document to `target/doc/rtao/`, and
  `cargo doc --workspace` warns "output filename collision" and lets one overwrite the other (reproduced with cargo 1.99
  on a two-crate workspace; `doc = false` removes it).
- **docify**: every example is a real `#[docify::export]` item in `examples/` or `tests/`. Offline examples (units,
  `pyfmt`, btauth vectors, `FakeChain` reads, intent plans) use `embed_run!`; node-dependent ones are items in
  `crates/rtao-testkit/tests/` (the `e2e` target), so the e2e lane runs them. READMEs compile from `README.docify.md`,
  so examples cannot rot.
- **mdBook** (`book/`) mirrors the bittensor.com taxonomy so an agent that knows upstream's `llms.txt` navigates rtao's
  by analogy: `index`, `quickstart` (template, patch line, connect, typed read, plan and execute on localnet), `sdk`,
  `agents` (upstream's eight-step flow with `rtao tools`, `execute_tool`, `Policy`, `--dry-run`, error codes), `cli`,
  `guides` (validator, miner with btauth, owner tooling, commit-reveal and mechanisms, Null consensus, MEV shield,
  timelock, wallets and signers, Ledger, local development, deployment, observability, upgrades, testing your subnet),
  `concepts` (links to upstream raw markdown, not copies), `migration` (v11 Python → rtao generated from
  `parity/map.toml`; v10 Python → rtao; the core's blocking client → rtao; the divergence register), `reference`,
  `parity`, `compat`.
- **Generated reference** (`xtask regen`): `tx/<op>.md`, `query/<read>.md`, `errors/<code>.md`,
  `errors/chain/<Name>.md`, `hyperparameters/<name>.md`, following upstream's page templates with a `## Rust` section;
  `regen --check` keeps them current. Pages cite sources as path plus item, without line anchors, so line shifts in
  runtime or pallet files change no page.
- **Catalogs** in upstream's schema plus a `rust` object per entry (type, role, output, CLI line, or read method), so
  tools that consume upstream's catalogs read rtao's unchanged.
- **`llms.txt` and `llms-full.txt`** following upstream's conventions, with a mapping block from each `tx/<op>` and
  `query/<read>` page to its upstream raw page, and a short agent card generated from the CLI tree.
- **Upstream log**: every pin bump appends its porting-report summary and oracle diff to `book/src/upstream-log.md`.
- **Per milestone**: each milestone ships the docs for what it ships; M9 completes the book and `llms.txt`. Until 1.0
  the README and release notes state that releases have had per-milestone cold-reader security passes, not the full review
  ([§12.11](#1211-m9-hardening-and-10)).

## 12. Milestones

### 12.1 Overview

The parity machinery exists from M0, so nothing ported early is redone. Each milestone ends with green CI on every lane
it introduces, a release with the next minor version (`rtao-v0.<k>.0`, numbered in completion order; M0's is
`rtao-v0.0.0`, and the changelog and `compat.json` name the milestone a release completes,
[§10.5](#105-compatibility-matrix-and-run-time-guard)), and docs for what it ships. Item assignments (which intent, read
and CLI command lands where) follow [parity-inventory.md](docs/plan/parity-inventory.md), which resolves the ambiguities
of the architecture's milestone text ([architecture §14](docs/plan/architecture.md#14-milestones)).

| # | Milestone | Depends on | Headline exit check |
|---|---|---|---|
| M0 | Foundations and pins | — | goldens reproduced through rtao conversions; btauth/1 vectors green; no-host seam builds |
| M1 | Read path and the first typed read | M0 | finney metagraph equal through three decode paths; transport checklist green |
| M2 | Validator | M1 | 20 fast + 3 slow localnet epochs with every reveal observed; no-host image on stock `rust:1.89-slim`; 24 h testnet soak |
| M3 | Miner | M2 | Python ↔ rtao neurons interoperate in CI; a generated template project passes its own CI |
| M4 | Owner tooling, shield, CLI v0 | M2 | scenario 10 (immediate and queued creation) and shielded `burned_register` green; dry-run JSON equal to btcli |
| M5 | Intent parity | M4 | oracle 83/83; no intent pending; scenario 18 green |
| M6 | Read parity | M2 (reads, namespaces, `blocking`); M4 and M5 for its CLI [I] | read oracle 102/102; `pending.toml` holds no reads |
| M7 | Signers, EVM, PoW GPU, timelock UX | M5 [I] | Ledger vector, replayed transcripts and the Speculos emulator lane, plus a device run; Vault UOS vectors and round trip; extension bridge interop; EVM and timelock cross-checks; PoW e2e |
| M8 | CLI parity | M5, M6, M7 | `cli_tree` parity; strict parity gate |
| M9 | Hardening and 1.0 | M8 | `rtao-v1.0.0`; one full upstream bump through the playbook |
| M10 | Post-parity compatibility crates and `RemoteSigner` (optional) | M9 | interop with a `bittensor==10.5.0` Synapse peer and btlightning; `RemoteSigner` through a reference server |

Two scope splits follow dependencies and size:

- **M2 carries the validator's whole API.** `NeuronCx` holds a `MetagraphHandle` and a `SignedClient`,
  `Validator::on_metagraph` takes a `MetagraphDiff`, and the validator example calls `fan_out`
  ([§7.2](#72-a-validator); architecture §9.1-§9.2), so the validator preset of `MetagraphSync` and the btauth client
  adapter (with `rtao_primitives::axon`) land in M2. M3 keeps the server adapter, the miner preset, `AxonPublisher`, the
  miner runtime, the template and the interop lanes.
- **M4 carries the CLI machinery, M8 the rest of the CLI**, following the inventories' split of CLI work. M4 carries
  the CLI machinery, 8,389 Python lines (`context.py`, `output.py`, `prompt.py`, `call.py`, `main.py`, `globals.py`,
  `tx.py`, `query.py`, `call_names.py`, `intent_prompts.py`, `logs.py`, `secrets.py`), and 40 commands, on top of 11
  intents, the shield pipeline, `PowSolver`, `OwnerOps`, `OwnerMonitor` and 22 reads. M8 carries 45 commands (2,851
  lines in M8-only files, plus the M8 parts of `wallet.py`, `stake.py` and `proxy.py`) and the strict gate.

**Upstream tracking** is standing work from M2 (M2-22 builds the pipeline; during the goal loop the agent runs each bump
itself, and the bot takes over at the handover, M9-07) to the end of the project: pin bumps, re-ports of changed,
already-ported files, and new ops and reads. Its share grows as the ported surface grows
from nothing to everything. Re-checked churn: `sdk/python/bittensor` grew from 48,735 `.py` lines at v445 (2026-08-12)
to 58,269 at v459 and 59,723 at v477 (2026-10-08); `git diff --shortstat v459 v477 -- sdk/python/bittensor` is 42
files, +2,636/−942; 20 release tags in 57 days, 3 of them abandoned.

**Steps and exit checks.** Each milestone lists its steps with stable IDs (`M2-07`), in an order in which every step
depends only on earlier steps and earlier milestones; a **Parallel work** note names the steps that may also run side
by side. Exit checks are numbered, and each is a command, a named test or a CI lane that passes or fails. They use these
conventions:

- unit, golden, oracle, `FakeNode` and `FakeChain` tests run in `cargo nextest run --workspace --all-features --locked
  --profile ci` (the `check` job); a check that names a test means the test exists and passes there;
- localnet scenario *n* is the test function `scenario_<nn>_<name>` (two digits) in `rtao-testkit`'s `e2e` target, run
  by `e2e.yml` as `cargo nextest run -p rtao-testkit --features docker --test e2e --profile e2e-fast` (`e2e-slow` for
  the 12 s lane), where `-E 'test(/^scenario_01_/)'` selects scenario 1;
- interop lane *n* is the test function `lane_<n>_<name>` in `rtao-testkit`'s `interop` target, run by `interop.yml`;
- "green" means that a run of the named workflow or job on the milestone's final commit of `main` succeeded.

**Common exit checks**, which every milestone adds to its own list:

- **C1.** `pr-gates` and, once `e2e.yml` exists (M2-20), `e2e-required` are green on `main` at the milestone's final
  commit, in the runs its push started (there are no PRs), with every job that exists by then. A newer push replaces a
  pending run, so the agent pushes nothing else until both finish.
- **C2.** Every lane the milestone introduces has a green run on `main` (a scheduled lane counts once a manual
  `workflow_dispatch` run passes; a job that pushes or opens PRs counts once its `dry-run` dispatch passes,
  [§9.7](#97-ci-lanes-and-what-each-catches)).
- **C3.** Every step box of the milestone is checked, and the [progress log](#19-progress-log) records the exit.
- **C4.** Every row of [§15](#15-upstream-contributions) whose evidence step belongs to the milestone has a link in its
  Filed column; the Ledger app item outside the monorepo counts as filed (2026-10-13, Q22).
- **C5.** From M2 on, `upstream.toml` pins the newest finalized monorepo release whose `:vS` localnet image is
  published, or the progress log names what blocks that bump (`oracle-stale`, `recordings-stale`, and from M7
  `ledger-stale` and `core-patch-conflict`). During the goal loop the agent performs pin bumps itself with the playbook
  of [§10.3](#103-a-pin-bump-step-by-step), at the latest at each milestone exit.
- **C6.** The docs for what the milestone ships exist: the README and CONTRIBUTING from M0, and from M1 on
  `mdbook build book` succeeds with the chapters the milestone's steps name.
- **C7.** The milestone's release exists: at the milestone's final commit the agent sets `[workspace.package] version`
  and the changelog, tags `rtao-v0.<k>.0` (`rtao-v0.0.0` for M0, `rtao-v1.0.0` for M9) and pushes the tag, without
  asking ([Executing this plan](#executing-this-plan), rule 7); `release.yml` succeeds from M1 on; and from M7 on, the
  manual Ledger device run for that tag (or a later patch tag of the same milestone) passes and is recorded in
  `docs/ledger-device-run.md`, waiting on the user under rule 3
  ([§10.5](#105-compatibility-matrix-and-run-time-guard)).

**Security passes by cold readers** (Q19). M2, M3, M4 and M7 each end with a security pass, and M9 with the full review.
A local cold reader does each one: a fresh-context reviewer (a new agent session, or a person, that took no part in the
implementation) that receives only the repository at the milestone's final commit, the milestone diff
(`git diff <previous release tag>..HEAD`; the whole repository for M9) and this checklist:

1. keys and secrets: where secret material is read, held, written and logged; keyfile permissions;
2. zeroization: `Zeroizing`/`secrecy` on every secret, offloaded KDF buffers included;
3. `Debug` and `Display` on secrets: redacted, with a test;
4. signer roles: no neuron path reaches a coldkey (`KeySource<Role>`), and the `dyn` path keeps Python's runtime check;
5. nonce and submission safety: `NonceLease` settlement, no resent submit (`ReadMethod`), `FateUnknown`
   reconciliation, shield eras and nonce pairs;
6. TLS configuration: one `ring` provider, no process-default lookup in library crates, certificate validation on;
7. btauth/1 verification and the nonce store: the freshness window, the authenticate → policy → commit → limit order,
   the store's bounds, the body limit before buffering;
8. input validation of untrusted JSON, headers, RPC frames, keyfiles and CLI arguments, with size limits;
9. dependencies and advisories: the `cargo deny check` and `cargo audit` results, and every dependency the diff adds.

The milestone's security step names its own focus areas. The reader writes `docs/security/<milestone>.md` (`m2.md`, …,
`m9.md`): the reviewer, the commit range, each checklist item's result, and each finding with its severity and
resolution. Every finding is fixed (the report links the commit) or explicitly accepted (the report gives the reason)
before the milestone exits.

**Upstream filing** (Q18). Each upstream PR and issue of [§15](#15-upstream-contributions) is filed as soon as the step
in its "Evidence at step" column is done (a pre-authorized action; the Ledger app item outside the monorepo was filed
on 2026-10-13, Q22), and its link goes into §15's Filed column (C4).

### 12.2 M0: Foundations and pins

**Goal.** A workspace that builds against the pinned core, with every guard later work relies on. **Depends on**
nothing.
**Deliverables.** Tag `rtao-v0.0.0`; the dual license; `upstream.toml`, `clippy.toml`, `deny.toml`; `rtao-core`,
`rtao-core-ext`, `rtao-primitives`, the `rtao-btauth` core, a minimal `rtao-signer` and `rtao doctor`; `xtask` pin,
import-upstream, extract v0, deps-check, core-surface, staged, lint-upstream and the parity file layer over all 357
`map.toml` rows; `rtao-testkit::fixtures`; the first `pr-gates` jobs and `xtask gates`, their local equivalent; the
filing of PR-U17 from the user's fork of the monorepo, whose evidence predates M0 ([§15](#15-upstream-contributions)).

**Steps.** M0-01…M0-29 are listed in order, with their details, in [§12.14](#1214-first-steps):

| Area | Steps |
|---|---|
| prerequisites and PR-U17, relicensing, workspace and pins | M0-01…M0-08 |
| xtask | M0-09…M0-13 |
| the seam and its guards | M0-14…M0-20 |
| primitives, JSON and goldens | M0-21…M0-23 |
| btauth/1, `rtao doctor`, CI and docs | M0-24…M0-28 |
| exit and tag | M0-29 |

**Exit checks** ([architecture §14](docs/plan/architecture.md#14-milestones)):

1. The golden tests `golden_calls`, `golden_storage_keys`, `golden_signature_payloads`, `golden_extrinsics`,
   `golden_multisig` and `golden_ss58` pass: each `golden.json` section is reproduced through rtao conversions (M0-23).
2. `corpus_replay` passes on `core_tag` (M0-23).
3. `cargo nextest run -p rtao-btauth --profile ci` passes with every btauth/1 vector and negative case (M0-25).
4. `cargo xtask deps-check --profile no-host` passes over a no-host build of `rtao-core` and `rtao-primitives`
   (M0-11).
5. `cargo test -p rtao-core --test lock` passes: exactly one `w3f-bls`, at `core_tag` (M0-14).
6. `cargo xtask import-upstream --verify-only`, `cargo xtask extract --check`, `cargo xtask parity check` (the file
   layer over 357 rows), `cargo xtask core-surface`, `cargo xtask staged --check` and `cargo xtask lint-upstream` pass
   (M0-09…M0-20).
7. The `serde_json/arbitrary_precision` tests of M0-22 pass.
8. `rtao doctor` passes on this repository, and on a fixture workspace without the patch line it fails and prints the
   exact line (M0-26).
9. `LICENSE-MIT`, `LICENSE-APACHE` and `NOTICE` exist; `cargo metadata --no-deps` shows `MIT OR Apache-2.0` or
   `(MIT OR Apache-2.0) AND Apache-2.0` for every package; the `supply` job's license check passes (M0-02).
10. `pr.yml` runs the jobs `check`, `no-host`, `msrv`, `features` and `supply`, `cargo xtask gates` passes locally
    (M0-27), and C1-C7 hold.

### 12.3 M1: Read path and the first typed read

**Goal.** An async client that reads finney correctly and survives the public endpoints. **Depends on** M0.
**Deliverables.** A minor release ([§12.1](#121-overview)); `rtao-rpc` with ring TLS, `rtao-client` (read side,
`rtao_client::config`), `rtao-runtime` with `xtask regen` v1, `rtao-metagraph` (model, view and `SubnetsMetagraphExt`),
`rtao_reads::commitments`, the first two reads, `rtao-testkit` fakes; the `metadata-drift` job; the nightly `fuzz`,
`bench` (criterion baselines), `features-powerset` and `core-canary` jobs; `live-probe`; the run-time compatibility
guard; `release.yml`, which builds the M1 release from the agent's tag, and `release-plz.yml`, upkeep that stays off
during the goal loop.
**Parallel work.** M1-01 and M1-02 are independent of each other; M1-10 and M1-11 need only M1-01; M1-15 needs nothing
from M1; M1-20…M1-24 are independent of each other.

- [ ] **M1-01** Extraction: the M0 extractor extended to `PY/settings.py` and `PY/_transport/rpc.py`, with the seven
      resolved forms of [§8.3](#83-how-parity-is-enforced) and the test `extract_check_covers_named_tables`; the
      generated settings tables of `rtao_catalog::py` and the `py_rpc.rs` constants of `rtao-rpc` come from them.
- [ ] **M1-02** `rtao-core-ext`: `ir::MetadataIr`, `ops::{metadata, storage, runtime_api}`, `value_ext`.
- [ ] **M1-03** `rtao-runtime` and `xtask regen` v1, whose input is finney's V15 and V16 metadata at the pinned spec
      (`xtask pin` fetches it into `vendor/metadata/finney/` with its `index.toml` row and the `[networks.finney]`
      metadata pin, [§10.3](#103-a-pin-bump-step-by-step) step 3): `codegen/*.toml`; generated storage, calls, runtime
      APIs, constants, events, errors, extensions, selective index, shapes; headers without `spec_version`; semantic
      overrides (`PriceRao`, `StakeWeight`, `CompactBytesText`; `DefaultAccount` applied by rule over the pinned
      metadata to every `Default` `AccountId32` entry whose default is 32 zero bytes, pinned by the offline D-001 test
      `zero_account_default_decodes_to_zero_bytes` over the vendored metadata). `core-oracle` v0 and the live D-001
      sanity check (scenario 15) move to M2, where `Localnet` and their CI jobs exist [I].
- [ ] **M1-04** `ShapeHash` (exact and projection, with the outer `RuntimeCall`/`RuntimeEvent`/`RuntimeError` enums
      opaque), `verify_surface`, `SurfaceReport`; `RuntimeHandle`, `RuntimeEntry`, `MetadataLevel`, `CallBytes` and
      `ComposeError` in `rtao-runtime`; cross-check against `PY/_generated/*.py`; `regen --check` joins the `check`
      job.
- [ ] **M1-05** `rtao-testkit`: `FakeState`, `FakeNode` with `Fault`s (also over `wss://` with a self-signed
      certificate), `FakeChain`.
- [ ] **M1-06** `rtao-rpc`: `Session` supervisor over tokio-tungstenite; `SessionConfig` defaults from `rpc.py`;
      `ReadMethod` allowlist with a per-frame `sent` flag; the reconnect table of
      [architecture §3.2.2](docs/plan/architecture.md#322-reconnect-semantics-port-of-rpcpy-each-row-a-fakenode-test-315).
- [ ] **M1-07** `rtao-rpc`: policy classifier ported from `PY/_transport/rpc.py` over the generated `py_rpc.rs`
      constants; `EndpointPool` and archive pool; token bucket; metrics; `impl RpcClientT`.
- [ ] **M1-08** `rtao-rpc` TLS: the ring-only `rustls::ClientConfig` with native roots plus `SessionConfig::extra_roots`
      and ALPN `http/1.1`, passed as `Connector::Rustls`; `max_message_bytes` sets both the frame and the message limit
      (a 20 MiB single-frame response is accepted); the caller-gone rule: a queued frame whose caller has gone is never
      transmitted.
- [ ] **M1-09** `rtao-testkit`: the port of `PYT/unit/test_rpc.py`; the transport rows of the §3.15 checklist; a test
      pinning the tokio-tungstenite 0.30 handshake error API (Q16).
- [ ] **M1-10** `rtao-client`: network presets (`finney`, `test`, `devnet`, `archive`, `local`) from `rtao_catalog::py`,
      extracted from `PY/settings.py` (`rtao-rpc` has no rtao dependencies, so presets live here).
- [ ] **M1-11** `rtao_client::config` (port of `PY/config.py`): the seven btcli files' paths and environment overrides,
      `SETTABLE` (extracted with its type tags), `btcli.json` load and save, and the token-symbol and subnet-name
      caches; writes are atomic and off the reactor, and a corrupt file is renamed `.corrupt.<ts>`. `Client::connect`
      (token symbols), `NeuronConfig` (precedence, M2) and `rtao-cli` (M4) use it; the address, proxy and multisig
      books live in `rtao-cli`'s `books` module (readers in M4, writers with their commands in M5 and M8).
- [ ] **M1-12** `rtao-client`: `HeadTracker` with gap backfill.
- [ ] **M1-13** `rtao-client`: `Client::connect` (genesis check, ss58 42, finalized head, runtime version, extension
      guard, `verify_surface`, head tracker, upgrade watcher, token symbols, and the run-time compatibility guard over a
      compiled-in `compat.json` from `xtask compat` v0, which warns outside the validated spec range,
      [§10.5](#105-compatibility-matrix-and-run-time-guard)) and `Client::from_rpc` (for `FakeChain` and custom
      transports); `ClientError` with `code()` and `policy()`; dropping the last `Client` clone cancels the background
      tasks and closes the socket (`DropGuard`, a `FakeNode` test); `RuntimeCache` (V15 fetch, needle-only V14,
      `MetadataLevel`, LRU, parent-runtime resolution, Python-compatible disk cache).
- [ ] **M1-14** `rtao-client`: `View`, `Snapshot`, `DynView`, `ViewExt` (`storage`, `fetch` coalescing at ≤256 keys per
      chunk, `storage_iter`, `call`, `constant`, `events`, dynamic reads).
- [ ] **M1-15** `rtao_primitives::{drand, ratio::ratio_fraction, hyperparams::proportion_to_raw}`.
- [ ] **M1-16** Metagraph: models; `Metagraph` with `MetagraphView` (exact shape gate), the `DecodeAsType` fallback and
      `from_dynamic` (port of `PY/metagraph.py:297-403`); the three-path differential test; a fuzz target.
- [ ] **M1-17** Reads `metagraph` (Python's raw `get_metagraph` dict, reachable through `dispatch` from M2) and
      `token_symbols`. `xtask regen` v1 emits the `Subnets` namespace accessor on `Client` and `Snapshot`, named from
      the catalog `category`, with no inherent `metagraph` method: the read's `python` field is empty, and an inherent
      method would shadow the curated one.
- [ ] **M1-18** `rtao_reads::commitments`: `NeuronCommitment`, `CommitmentMap`, the status machine and decoders, and
      `fetch_commitments`/`fetch_commitment_map` (`PY/metagraph.py:37-125, 258-291, 406-560`).
      `NeuronCommitment::reveals_at` uses `rtao_primitives::drand::reveal_time` over the core's `GENESIS_TIME` and
      `DRAND_PERIOD`, not `rtao_tx::timelock`, which sits above `rtao-reads`. The curated `metagraph()` is
      `rtao_metagraph::SubnetsMetagraphExt` (`metagraph`, `metagraph_with`), implemented for `rtao_reads::Subnets` and
      exported from `rtao::prelude`, with commitments included by default as in Python.
- [ ] **M1-19** The commitments half of the port of `PYT/unit/test_v446_accounting_and_commitments.py` (the locks half
      lands with its reads in M6-01).
- [ ] **M1-20** Criterion baselines; the neuron-profile crate count recorded (architecture §2.8); `nightly.yml` jobs
      `fuzz` (`cargo +nightly`) and `bench` (fails above 30%) over the targets that exist by then (btauth/1 header
      parsing and `verify`, JSON-RPC frame parsing, `MetagraphView`).
- [ ] **M1-21** `xtask drift` and the `metadata-drift` job, an upkeep job ([§9.7](#97-ci-lanes-and-what-each-catches)):
      its scheduled runs exit early until `RTAO_UPKEEP` is `on`, its `dry-run` dispatch does everything but push and
      open the PR, and the rtao-bot App token it uses after the loop arrives at the handover (M9-07);
      `vendor/metadata/<net>/` for testnet, devnet and `localnet-main` (finney's lands in M1-03); `xtask drift diff`
      unit tests over committed fixtures.
- [ ] **M1-22** `nightly.yml` jobs `features-powerset` and `core-canary` (build, `core-surface` and offline tests
      against monorepo `main`; its `:main` localnet smoke joins in M2-22 with `Localnet`); `probe.yml` (`live-probe`,
      architecture §12.6: finality lag, V15, digest pins, the three metagraph decode paths).
- [ ] **M1-23** Release automation: `release.yml`, run by the milestone tags the agent pushes (`xtask compat`, `xtask
      release-notes`, downloadable rustdoc and book archives from a first `book/` skeleton; per-target binaries,
      which carry `rtao doctor` until CLI v0 in M4). All hosted jobs use free `ubuntu-latest`; Linux builds retain
      glibc 2.35 through a pinned Ubuntu 22.04 container and ARM cross tooling, while macOS builds and verification
      run locally on Mac Studio (§9.8). A draft release is published only after every target, checksum and required
      provenance is present; trustworthy provenance for local Mac artifacts must be resolved, not attributed to the
      Ubuntu build. Also wire `release-plz.yml`, upkeep that is not used during the goal loop
      (off until `RTAO_UPKEEP` is `on`, with a `dry-run` dispatch).
- [ ] **M1-24** Public-endpoint quota measurement to tune the default bucket (Q6) [I: milestone not fixed in the
      architecture]; the measured quotas and the resulting `RateLimit` defaults go into the progress log.

**Exit checks:**

1. `cargo xtask probe` (the `live-probe` checks, M1-22) passes against finney, including
   `client.subnets().metagraph(NetUid(1))` decoding through the view, typed and dynamic paths with equal `to_json()`
   (M1-16…M1-18).
2. `cargo nextest run -p rtao-rpc -p rtao-testkit --profile ci` passes: the `test_rpc.py` port and every transport row
   of the §3.15 checklist, `wss://` through the ring config included (M1-05…M1-09).
3. The `xtask drift diff` unit tests pass over committed fixtures and classify finney 477 against devnet 478 as
   `surface-unchanged`, 477 plus an added call as `additive`, 477 with a changed storage default as `behavioural`, and
   477 with a moved call index or a new extension with a payload as `breaking` (M1-21). (finney, testnet and devnet
   all run 478 since 2026-10-13, so a drift job bootstrapped in M1 never sees a 477→478 change, and other real changes
   depend on upstream timing.)
4. A `metadata-drift` run dispatched with `dry-run` exits 0 against an up-to-date index (M1-21).
5. `cargo xtask regen --check` and `cargo xtask extract --check` pass, `extract_check_covers_named_tables` included
   (M1-01…M1-04).
6. C1-C7; the lanes of C2 are `metadata-drift`, `fuzz`, `bench`, `features-powerset`, `core-canary`, `live-probe`,
   `release-plz` and `release`.

### 12.4 M2: Validator

**Goal.** A production weight-setting validator built only from rtao's public API. **Depends on** M1.
**Deliverables.** A minor release ([§12.1](#121-overview)); `rtao-signer` roles, `rtao-wallet`, the submission
pipeline, `rtao-catalog`, `rtao-tx`, the pass-through generator, 9 intents and 13 reads, `MetagraphSync` (validator
preset) and the btauth client adapter, `NeuronRuntime::validator` and the validator example; every `xtask parity
check` layer; the oracle image and vectors; `Localnet`, `Scenario`, the `e2e` workflow and the `core-oracle` job;
upstream tracking (the pin-bump pipeline, which the agent runs from here on, and the `pin-bump`, `oracle-refresh` and
`upstream-next` jobs); a no-host validator image; validator guide in
the book; the M2 security pass.
**Parallel work.** M2-01…M2-02 (keys and wallets) and M2-03…M2-05 (extraction, catalog, oracle) are independent of each
other; M2-16 needs only M1; M2-24, the soak, runs in the background ([Executing this plan](#executing-this-plan),
rule 3).

- [ ] **M2-01** `rtao-signer`: `SigningRequest`, `SignerCaps`, roles (`Role`, `Coldkey`, `Hotkey`, `KeyView`,
      `KeySource<R>`, `DynKeySource`).
- [ ] **M2-02** `rtao-wallet`: btcli layout, `HotkeyFile` through the staged `keyfile_json` (no `host`), `Wallet` and
      encrypted coldkeys behind `encrypted`, the password chain of `PY/wallets.py:62-118` (steps 1-4 and 7 here; steps
      5-6, the macOS Keychain and the macOS dialog, in M4), create and regen; the keyfile differential test; staged
      `keyfile_env`; the keyfile-detection and `keyfile_json` fuzz targets join the nightly `fuzz` job
      ([§9.6](#96-property-tests-fuzzing-benchmarks)).
- [ ] **M2-03** Extraction: `PY/error_map.py`, `PY/result.py`, `PY/executor.py` constants, the remaining
      `PY/settings.py`, `PY/fee_filters.py` (equal to the runtime's `fee_filters.rs`, parsed with `syn`), custom
      transaction codes (`common/src/transaction_error.rs`), intent field defaults and class variables, read parameter
      defaults and requiredness (`read_params.json`), and the environment variables the Python package reads
      (`env_vars.json`).
- [ ] **M2-04** `rtao-catalog`: `INTENTS`, `READS`, `ErrorCode` (18), `CHAIN_ERRORS` (390), `py::*`, `rt::*`.
- [ ] **M2-05** Oracle image (`ci/oracle/Dockerfile`) and dumps for the M2 ops and reads and the pure functions
      (`conform`, 10,000 `str(Balance)` values, classification), with the per-sample seeds (`vendor/oracle/seeds/`) and
      the reads' `touched` record; `vendor/oracle/`. The image is cached in a GHCR namespace (an external prerequisite)
      and also builds locally.
- [ ] **M2-06** Staged `ops::{receipt, fee, extrinsic_lookup, dispatch_error}`, with a `FakeChain` receipt test in which
      a `Sudo.sudo` fails inside `Sudid` (PR-U1 and PR-U2 evidence,
      [§5.8](#58-known-core-defects-and-how-rtao-handles-them) #4).
- [ ] **M2-07** Submission (`rtao-client`): `NonceManager`/`NonceLease`, one manager per `Client` (`client.nonces()`,
      shared by every clone and executor); era 128 anchored at the finalized head; metadata-hash mode with the
      RFC-0078 digest cached per spec; sign, assemble, submit, watch; `SubmitFailure`; `CallBytes` spec stamping; typed
      `PoolRejection`; `FateUnknown`; reconciliation; the watch statuses of D-024 (`retracted` keeps watching,
      `finalityTimeout(h)` resolves from block h, `dropped` reconciles when `reconcile` is on); the event-splitting
      fuzz target joins the nightly `fuzz` job.
- [ ] **M2-08** `rtao-tx`: intent traits, `Executor` (`plan`, `preflight`, `execute`, `dyn` twins), `Plan`, `Policy`
      with the harness of a table-driven equivalence test against the core's constructors
      ([§5.8](#58-known-core-defects-and-how-rtao-handles-them) #9), whose pairs land with their intents: 3 in M2-12,
      5 in M4-06 and the other 13 in M5-03, for 21 of 22 at the M5 exit. `ExtrinsicResult<O>`, `ChainError`,
      `PolicyError`, `TxError`, `classify` and its name, code and custom-code gates.
- [ ] **M2-09** `rtao-intents` and `rtao-reads`: generated interfaces, `AnyIntent`, `defaults.rs`, `ReadName`,
      `ReadSpec`/`ParamSpec`, and `dispatch`, which fills the extracted `read_params.json` defaults; `RecordingView` and
      the exact-hits test with `parity/read_hits_extra.toml` ([§9.3](#93-fakes-at-the-right-layers)); `pending.toml`
      listing the 74 unported intents and 87 unported reads.
- [ ] **M2-10** The pass-through generator (`generated/passthrough.rs`), driven by `codegen/intent_types.toml` and
      `intent_callmap.toml`; `transfer_all` is its first output, and it later generates M4's nine and M5's 36 bodies, so
      no body is hand-written first [I: building it here means none of the 10 M2 and M4 bodies is written twice].
- [ ] **M2-11** `weights::conform` (`round_ties_even`, Python's order); `TimelockCommitter`; staged `epoch_schedule` and
      `timelock_commit` (PR-U5 evidence), with the safety-net vectors: a skip at a scheduled slot b fires the epoch at
      b + 1; a skip at an owner-triggered slot b with step counter c fires it at b + tempo + 1 − c; a tempo lowered
      below the counter in block t fires it at t + 1.
- [ ] **M2-12** The 9 M2 intents: `set_weights`, `commit_weights`, `reveal_weights`, `serve_axon`, `serve_axon_tls`,
      `serve_prometheus`, `reset_axon`, `transfer`, `transfer_all`. The `Policy` equivalence test (M2-08) gains its
      three M2 pairs: `transfer`, `transfer_all` and `transfer_allow_death` (↔ `Transfer { keep_alive: false }`).
- [ ] **M2-13** The 13 M2 reads: `block_time`, `timestamp`, `blocks_since_last_step`, `blocks_since_last_update`,
      `epoch_status`, `max_weight_limit`, `min_allowed_weights`, `reveal_period`, `weights_rate_limit`, `uid`,
      `commit_reveal_enabled`, `mechanism_count`, `weights`.
- [ ] **M2-14** Parity machinery: the remaining `xtask parity check` layers (symbol, catalog, extraction, chain-source,
      environment, coverage, oracle, completeness with `parity/exports_extra.toml`) and `divergences.toml` from
      Appendix C. From here on, `parity/map.toml` rows (seeded in M0 from
      [python-module-map.md](docs/plan/python-module-map.md)) are blessed as they are ported.
- [ ] **M2-15** `MetagraphSync` with the validator preset (`EveryEpoch { fallback_blocks: 100 }`, `Preset::Validator`,
      `MetagraphHandle`, `MetagraphDiff` with `replaced`/`deregistered` for score resets, last-good snapshot on failure,
      architecture §8.3), because `NeuronCx` holds the handle and `Validator::on_metagraph` takes the diff.
- [ ] **M2-16** The `rtao-btauth` client adapter: `SignedClient` with `fan_out` and `fan_out_stream` (`FanOut` defaults:
      256 concurrent, 3 s connect timeout, 12 s per attempt, a call deadline of timeout + 1 s, no retries), over
      `rtao_primitives::axon::{AxonTarget, axon_base_url}` and `Metagraph::axon_targets()`, re-signing every attempt;
      its loopback `https://` `FakeNode` test through the ring config. `NeuronCx.http` is a `SignedClient`, and the
      facade's `neuron` feature already enables `btauth-client`.
- [ ] **M2-17** `rtao-neuron`: `NeuronConfig`, `NeuronRuntime::validator`, `NeuronCx`, `StateCell` (over
      `std::sync::RwLock`, with a `compile_fail` doctest for a guard held across an `.await`), `Validator` with
      `StepPolicy` (errors and panics counted and backed off, never ending `run()`); `WeightScheduler` (cadence with
      per-hotkey jitter, one-RPC preflight at the best head as Python reads it, the Null-consensus missing-permit
      refusal, commit-reveal version gate, conform, hold-previous guard, path selection, timelock, Python submit
      defaults, reconciliation); reveal watcher that follows the commit's actual reveal epoch and attributes misses
      (`reason`); `ClockGuard`; `EpochClock`; `StateStore`, with the test `validator_restart_resumes_state` (kill and
      restart resumes from persisted state); telemetry (`/metrics`, `/healthz`, `/readyz`); two-stage shutdown.
- [ ] **M2-18** `crates/rtao/examples/validator.rs` (the example of [§7.2](#72-a-validator)) and its line in the
      `features` job.
- [ ] **M2-19** `Localnet` (`xtask localnet bootstrap`, drand readiness, 64 slot accounts, the `Localnet::sudo()` lock)
      and `Scenario`. `Scenario` builds its setup calls with the generated typed builders
      (`rtao_runtime::calls::{subtensor_module, admin_utils}`), wraps root-origin calls with `rtao_client::compose::sudo`
      signed by Alice, and submits through `Client::submit`; it never goes through intents, because `rtao-testkit`
      does not depend on `rtao-intents` and most setup intents land in M4 and M5. Calls:
      `SubtensorModule.register_network`, `start_call`, unshielded `burned_register` and `add_stake`, and the per-subnet
      `AdminUtils` knobs (`sudo_set_weights_set_rate_limit`, `sudo_set_serving_rate_limit`,
      `sudo_set_min_allowed_weights`, `sudo_set_commit_reveal_weights_enabled` for scenario 2,
      `sudo_set_mechanism_count` for 3, `sudo_set_weights_version_key` for 5, `sudo_set_commit_reveal_version` for 6);
      the global knobs come from the bootstrap.
- [ ] **M2-20** `e2e.yml` with the `changes` job and the `e2e-required` gate, lanes `pinned` (every push), `mainnet`
      (pushes, nightly), `testnet` and `devnet` (nightly), `main` (weekly); `interop.yml`'s `core-oracle` job with
      `core-oracle` v0;
      scenarios 1, 2, 3, 5, 6, 7, 8 (the runtime upgrade, without its offline-payload assertion), 15 (the D-001 sanity
      check), 16 (`ci/pr-u4-probe/`) and 17 [I: assigned by content].
- [ ] **M2-21** `parity/e2e_map.toml` (115 keys, each `rtao`, `na` or `pending = "M<k>"`, [§9.4](#94-localnet-e2e)),
      with the six `test_*` keys of [inventory §2.5](docs/plan/parity-inventory.md#25-upstream-e2e-names) that M2 can run
      mapped to rtao tests.
- [ ] **M2-22** Upstream tracking starts ([§12.1](#121-overview)): the pin-bump pipeline end to end
      ([§10.3](#103-a-pin-bump-step-by-step): `xtask pin --parity`, re-vendor, extract, regen, `xtask oracle`,
      `xtask record`, the core-input fingerprint move, `xtask parity report` with auto-bless), which the agent runs
      itself from here on (C5); `pin-bump.yml` and `oracle.yml` around it, upkeep jobs whose `dry-run` dispatches C2
      counts; the `upstream-next` nightly job, and `core-canary`'s `:main` localnet smoke; `xtask pin --force` re-runs
      the pipeline at the pinned tag (exit check 8).
- [ ] **M2-23** The no-host validator Docker image (`FROM rust:1.89-slim-bookworm`, `-p <binary>`, BuildKit cache
      mounts).
- [ ] **M2-24** The 24-hour testnet soak runs that image on a long-lived host (a small VM or a self-hosted runner;
      GitHub-hosted jobs stop after 6 hours) with the soak hotkey, both external prerequisites; its evidence is the
      `/metrics` scrape over the run (commits, reveals, `rtao_weights_reveal_missed_total` by `reason`,
      `rtao_weights_commit_epoch_slip_total`), committed with a short summary to `docs/soak/m2.md`.
- [ ] **M2-25** The validator guide in the book ([§11](#11-documentation-plan)).
- [ ] **M2-26** Security pass by a local cold reader ([§12.1](#121-overview)), in `docs/security/m2.md`; focus: keyfile
      loading and `keyfile_json` (fuzz target green), the password chain, `Zeroizing`/`secrecy` with a test that
      `Debug` output redacts secrets, and the signing path.

**Exit checks:**

1. Scenario 1 passes on the `pinned` lane: a localnet validator runs 20 fast and 3 slow epochs with every reveal
   observed (M2-20).
2. Scenario 6, the commit-reveal version gate, passes (M2-20).
3. Scenario 8 passes, or is skipped with its printed reason when the `:main` runtime's spec is not above the pinned
   one (M2-20).
4. `validator_restart_resumes_state` passes: kill and restart resumes from persisted state (M2-17).
5. The `conform` oracle vectors pass bit-exact, `.5` ties included (M2-05, M2-11).
6. `docker build` of the no-host validator image `FROM rust:1.89-slim-bookworm` succeeds without installing Perl, make,
   CMake, pkg-config, libssl-dev or libsodium-dev (its native code is `ring` and `blake3` SIMD, built by the C compiler
   rustc already uses as its linker), and the built validator connects over `wss://` to a TLS `FakeNode` (M2-23).
7. The `no-host` job passes: `cargo tree -e normal,build` of both no-host profiles contains none of the crates banned in
   [§5.7](#57-the-no-host-neuron-profile).
8. `cargo xtask pin --parity <parity_tag> --force` (re-vendor, extract, regen, oracle, record, parity report) leaves the
   tree unchanged (`git diff --exit-code`) with an empty porting report (M2-22).
9. `docs/soak/m2.md` shows no rtao-attributable reveal miss over the 24-hour testnet soak: every increment of
   `rtao_weights_reveal_missed_total` carries a `reason` that names a failing chain check or `drand_round`
   ([architecture §9.3](docs/plan/architecture.md#93-weightscheduler-the-validators-critical-path) step 12) (M2-24).
10. `docs/security/m2.md` records the pass, and every finding is fixed or explicitly accepted (M2-26).
11. C1-C7; the lanes of C2 are the `e2e` lanes (`pinned`, `mainnet`, `testnet`, `devnet`, `main`), `core-oracle`,
    `pin-bump`, `oracle-refresh` and `upstream-next`.

### 12.5 M3: Miner

**Goal.** A miner behind btauth/1 with metagraph-based admission, and the subnet template. **Depends on** M2.
**Deliverables.** A minor release ([§12.1](#121-overview)); the `rtao-btauth` server adapter, `AxonPublisher`,
`MetagraphSync`'s miner preset and event-triggered refresh, `NeuronRuntime` for miners and the miner example; interop
lanes 1-4; `templates/subnet` with validator and miner members; miner guide; the M3 security pass.
**Parallel work.** M3-03 and M3-04 are independent of M3-01…M3-02.

- [ ] **M3-01** `rtao-btauth` server (`BtAuthLayer`, `VerifiedCaller`, `CallerPolicy`, `MetagraphPolicy`,
      `PerHotkeyLimiter`, `RedisNonceStore`, `Json<T>`): `Verifier::authenticate` and `Authenticated::commit` in the
      order authenticate → policy → commit → limit (D-032); the bounded `InMemoryNonceStore` (503 at the bound, never
      evicts); the body limit applied before buffering, with the runtime's `DefaultBodyLimit` matching it (a 3 MiB
      signed body reaches a `Json<T>` handler); `Json<T>` parses through `serde_json::Value` first
      ([§5.8](#58-known-core-defects-and-how-rtao-handles-them) #10); feature `legacy-v10`. (The client adapter landed in
      M2-16.)
- [ ] **M3-02** The btauth server path joins the nightly fuzz targets.
- [ ] **M3-03** `AxonPublisher` (IP resolution and rejection of non-global addresses, idempotence over the axon and the
      stored `NeuronCertificate`, so a certificate rotation alone republishes through `serve_axon_tls`
      (`cert_rotation_republishes`), `ServingRateLimit` scheduling, pool-rejection mapping, TLS certificate length check
      D-017).
- [ ] **M3-04** `MetagraphSync`'s miner preset (`Interval(600 s)`, minimum 30 s, `Preset::Miner`), the debounced
      event-triggered refresh (`NeuronRegistered`, `AxonServed`, `HotkeySwapped*`), selective metagraph fetches, and
      `rtao_metagraph::persist::{save, load}` (feature `persist`, architecture §8.5); the validator preset landed in
      M2-15.
- [ ] **M3-05** `Miner` trait and runtime (`router(cx, state)`, called after `on_start`, with the `StateCell` the
      handlers share); `PruneRisk` monitor; read `immunity_period`.
- [ ] **M3-06** `crates/rtao/examples/miner.rs` (the example of [§7.3](#73-a-miner)) and its line in the `features` job.
- [ ] **M3-07** Localnet scenario 9: a miner with idempotent `serve_axon` and the 401/403/413/429 refusals.
- [ ] **M3-08** Interop lanes 1-4 (btauth/1 both directions; Python and rtao neurons; wallets without Keychain, which
      needs the separate local macOS validation of M4-14; wire bytes at the current spec); btcli-created keyfile cross vectors committed to
      `vendor/recorded/keyfiles/` (PR-U13 evidence).
- [ ] **M3-09** `templates/subnet` with validator and miner members ([§7.5](#75-the-cargo-generate-template)) and its own
      CI. The CI builds both neuron images, starts the compose localnet, and creates a subnet with a registered, staked
      validator and a miner through `rtao-testkit`'s `Scenario` (a git dev-dependency of the template's e2e test, until
      `rtao localnet subnet` ships in M4). It then runs both binaries. In rtao's own CI the `template` job of `e2e.yml`
      (`cargo xtask template check`) generates the project, appends `[patch."https://github.com/sam0x17/rtao"]` path
      entries for `rtao` and `rtao-testkit` so it builds against the pushed commit rather than the last tag, builds
      both images with the checkout as a BuildKit named context, and runs the generated CI steps; it is part of
      `e2e-required`, and template-only pushes run it.
- [ ] **M3-10** The miner guide in the book ([§11](#11-documentation-plan)).
- [ ] **M3-11** Security pass by a local cold reader ([§12.1](#121-overview)), in `docs/security/m3.md`; focus: the
      btauth/1 verifier (freshness window, nonce-store bounds, body limit enforced before buffering, header-parsing fuzz
      target) and `PerHotkeyLimiter`.

**Exit checks:**

1. Interop lane 2 passes in CI: a Python v11 miner with an rtao validator, and a Python validator with an rtao miner
   (M3-08).
2. Interop lanes 1, 3 and 4 pass (M3-08).
3. `cert_rotation_republishes` and the `AxonPublisher` idempotence tests pass, and so does scenario 9: `serve_axon` is
   never resubmitted when unchanged and is resubmitted when only the certificate rotates (M3-03, M3-07).
4. `cargo xtask template check` passes: a project generated with `cargo generate` passes its own CI, both neuron images
   build, and on the compose localnet the miner answers a btauth/1-signed request from the validator and the
   validator's commit is revealed (`TimelockedWeightsRevealed`) (M3-09).
5. `docs/security/m3.md` records the pass, and every finding is fixed or explicitly accepted (M3-11).
6. C1-C7; the lanes of C2 are `interop` (lanes 1-4) and the `template` job.

### 12.6 M4: Owner tooling, shield, CLI v0

**Goal.** Subnet owners can create, start and configure a subnet, and the CLI covers the operator loop. **Depends on**
M2.
**Deliverables.** A minor release ([§12.1](#121-overview)); 11 owner and registration intents, the MEV-shield pipeline,
the raw-call path and the `Multisig` account object, `PowSolver`, `OwnerOps`, `OwnerMonitor`, 22 reads; the `rtao` CLI
v0 (its machinery and 40 commands); the template's owner member; owner and MEV-shield guides; the M4 security pass.
**Parallel work.** M4-01…M4-02, M4-03…M4-04 and M4-05 are independent of each other; M4-09 needs only M2 and M4-04.

- [ ] **M4-01** Hyperparameter units, split by layer: `rtao_primitives::hyperparams::{Hyperparam, HyperparamTable}`
      carry the ported functions of `PY/hyperparams.py:474-769` (`metadata_kind`, `kind_of`, `doc_of`, `short_of`,
      `normalized`, `annotate`, `value_forms`, `codec_of`, `raw_bounds`, `to_raw`) as methods; the data is generated
      into `rtao_catalog::hyperparams::TABLE` from the extracted `HYPERPARAMS`, `STORAGE_ITEMS` (each item resolved to
      its storage value's type ident and cross-checked against the pinned metadata) and the ident and kind codec tables;
      `rtao_intents::hyperparameters::to_raw` delegates to `TABLE`, so there is one implementation. Python's
      metadata-primary precedence is kept: `kind_of` decides by the storage value's unit-bearing type ident first, and
      `codec_of` takes explicit `bits`/`signed`, then that ident, then the kind default. The `--units` gate.
- [ ] **M4-02** `HyperparamPlanner` with the owner rules: the admin freeze window and the per-setter owner rate limits
      (`LastRateLimitedBlock` for owner hyperparameters, `TransactionKeyLastBlock` for the version key, tempo,
      mechanisms and trims, with the `prod_or_fast!` limits from `rtao_catalog::rt::rate_limits`); one `FakeChain` test
      per setter row.
- [ ] **M4-03** MEV shield pipeline: `ShieldPeriod` with the era check (refuse with `TxError::ShieldEraExpired` when
      B + 2 ≥ F + p, D-025), `ShieldCiphertext`, `lease_pair` leaving the next nonce at n + 2 (D-026), inner-extrinsic
      scan, preflight, routing and downgrade rules; shielded submissions are never retried.
- [ ] **M4-04** The raw path (`submit_call`, `submit_call_dyn`, `ExecuteOptions::shielded`) and the `Multisig` account
      object with its funds preflight (`rtao_tx::multisig`); `Client::{decode_scale, estimate_weight,
      estimate_shielded_carrier_fee}`. CLI v0's `call --multisig` needs them; `call` has no `--allow-raw`, because the
      CLI builds its executor without a `Policy`, as btcli does.
- [ ] **M4-05** `PowSolver` (CPU).
- [ ] **M4-06** Intents (11): `burned_register` (through the shield path), `pow_register` (`host`), `root_register`,
      `register_subnet` with the queued-completion follower, `start_call`, `set_hyperparameter` with
      `HyperparamPlanner`, `set_subnet_identity`, `update_symbol`, `set_mechanism_count`, `trim_subnet`,
      `trim_null_subnet_batch`. Nine of them use generated pass-through bodies. The `Policy` equivalence test (M2-08)
      gains its five M4 pairs: `register_subnet`, `start_call`, `burned_register`, `root_register` and
      `set_hyperparameter`.
- [ ] **M4-07** The 22 M4 reads: `balance`, `balances`, `mev_shield_next_key`, `delegate_take`, `delegated`,
      `blocks_until_next_epoch`, `next_epoch_start_block`, `difficulty`, `subnet_emission_enabled`,
      `hotkey_identities`, `identity`, `most_convicted_hotkey`, `root_basket_owed`, `stake_value_for_coldkey`,
      `stake_value_for_coldkeys`, `validator_basket_summary`, `burn`, `mechanism_emission_split`,
      `subnet_hyperparameters`, `subnet_identity`, `subnet_names`, `subnet_start_schedule`.
- [ ] **M4-08** `OwnerOps`; `OwnerMonitor`.
- [ ] **M4-09** The CLI machinery (the `cli` rows of python-module-map.md marked M4; CLI v0 covers 40 commands,
      [architecture §10.6](docs/plan/architecture.md#106-scope-per-milestone) as resolved by the
      [inventory](docs/plan/parity-inventory.md#7-cli-command-tree)): ports of `context.py`, `globals.py`, `main.py` (the
      command tree; the snake_case alias mechanism of `:449-477`, which then covers every later command including
      generated `tx`/`query`; the hidden v9 aliases of `:116-135` for the groups shipped so far), `output.py` (JSON
      value-equal; tables), `prompt.py`, `intent_prompts.py`, `call.py`, `call_names.py`, `secrets.py` and `logs.py`;
      the prompt module that replaces `masked_input.py`/`_live.py` (`rpassword` hidden entry under
      `PromptPolicy::IfTty`, re-asking on empty or mismatched input as `PY/keyfiles.py:42-62` does; Python's
      per-character `*` echo is rendering, which is not gated); and the `macos_password.py` port behind
      `--keychain-password`/`--macos-password`: Keychain read and write through `/usr/bin/security` with Python's exact
      argv, and the `osascript` dialog. `wallet new-coldkey`/`regen-*` and every coldkey-signed command need them on a
      TTY (D-003).
- [ ] **M4-10** CLI: globals, `config` (5), `doctor`, `localnet`, `explain`, `tools`, `call`; generated `tx` (20 ops) and
      `query` (38 reads); the rtao-only `wallet save-password-env` (architecture §6.2, §10.4; over the M2 staged
      `keyfile_env`).
- [ ] **M4-11** CLI: `wallet` create, new-coldkey, new-hotkey, regen-coldkey, regen-hotkey, regen-coldkeypub,
      regen-hotkeypub, show, list, balance, inspect; `weights` set, commit, reveal; `axon` set, reset.
- [ ] **M4-12** CLI: `hparams` set, get, get-take, check-start, start, set-symbol, get-identity, set-identity, trim,
      trim-null-batch, trim-null, mechanisms count, mechanisms emissions; `subnets` register, create, metagraph.
- [ ] **M4-13** Ports of `PYT/unit/test_pow_registration.py` (the solver and the intent keep the runtime challenge),
      `test_cli_null_pruning.py` (owner pruning must advance on chain, not only return successful receipts) and
      `test_natural_name_key.py` (CLI listings order `coldkey2` before `coldkey10`).
- [ ] **M4-14** Keychain compatibility test locally on Mac Studio (Q23), without Docker or Python, in a temporary
      default keychain (`security create-keychain`, `security default-keychain -s`, `security unlock-keychain`): an
      entry written with Python's exact argv (`security add-generic-password -s bittensor.coldkey -a <wallet> -U -w`)
      unlocks the coldkey through rtao's `--keychain-password`, and an entry saved by rtao reads back with
      `security find-generic-password -s bittensor.coldkey -a <wallet> -w`. The extraction layer pins the service name
      and argv from `PY/macos_password.py`. Before the run record the default keychain and search list; use only
      task-created keychains, restore those settings on success or failure, then remove the disposable keychain after
      dependency checks. The login keychain and user secrets are preserved. The separate harness runs only when this
      isolation is available (`cargo xtask gates` skips it, M0-27); otherwise this parity requirement stays unresolved.
- [ ] **M4-15** Localnet scenarios 4 (trim-null batches), 10 (owner: immediate creation, and the queued path in the
      `global` container: root `sudo_set_network_immunity_period(0)`, then `sudo_set_subnet_limit(<current non-root
      subnet count>)`, then `register_network`, which prunes the lowest-priced subnet and emits
      `NetworkRegistrationQueued`; the follower waits for that registration's `NetworkAdded`; the planner waits for the
      freeze window and the mechanism-count rate limit), 11 (`burned_register` only; `add_collateral` joins in M5);
      interop lane 8 for implemented ops, and lane 5 (reads at a pinned block) for the 38 reads implemented so far;
      `rtao-testkit::PinProxy` and interop lane 9 (CLI JSON at a pinned block) for the 40 CLI v0 commands, which each
      later milestone extends to the commands it ships [I: the proxy and harness are the check that the M4 "JSON
      value-equal" `output.py` work needs].
- [ ] **M4-16** Template: the owner-tool member and the owner image stage (Perl and make); its CI runs `OwnerOps`
      (create, start, one hyperparameter) against the compose localnet.
- [ ] **M4-17** `crates/rtao/examples/owner.rs` (the example of [§7.4](#74-an-owner-operation)) and its line in the
      `features` job; the four `test_*` keys of [inventory §2.5](docs/plan/parity-inventory.md#25-upstream-e2e-names)
      assigned to M4.
- [ ] **M4-18** The owner and MEV-shield guides in the book ([§11](#11-documentation-plan)).
- [ ] **M4-19** Security pass by a local cold reader ([§12.1](#121-overview)), in `docs/security/m4.md`; focus: coldkey
      unlock, CLI prompts and warnings about secrets passed in argv, the MEV-shield envelope, and the raw `call` path.

**Exit checks:**

1. Scenario 10 passes: immediate creation, and the queued path in the `global` container (M4-15).
2. Scenario 11 passes for `burned_register` (`add_collateral` joins in M5) (M4-15).
3. Interop lane 8 passes: `rtao tx <op> --dry-run --json` is value-equal to `btcli`'s for every implemented op (M4-15).
4. Interop lane 9 passes for the 40 CLI v0 commands (M4-15).
5. The Keychain compatibility test passes on Mac Studio, with isolated setup, restored host settings and recorded
   evidence (M4-14); absence of equivalent verification leaves this exit check unresolved.
6. `cargo xtask template check` passes with the owner-tool member (M4-16).
7. `docs/security/m4.md` records the pass, and every finding is fixed or explicitly accepted (M4-19).
8. C1-C7; the lanes of C2 are interop lanes 5, 8 and 9 and the template's owner image stage.

### 12.7 M5: Intent parity

**Goal.** All 83 intents. **Depends on** M4.
**Deliverables.** A minor release ([§12.1](#121-overview)); the remaining 63 intents, the offline-signing path, reads
`proxies` and `root_claim_threshold`, 20 CLI commands, generated `tx` reference pages complete.

- [ ] **M5-01** `prepare_call`/`submit_signature` with Python-compatible `UnsignedExtrinsic` JSON (the raw path landed in
      M4-04), with interop lane 6 (prepared extrinsics, both directions); reads `proxies` and `root_claim_threshold`
      (`set_root_claim_threshold`'s verify read); the multisig book and call-cache writers; the `UnsignedExtrinsic` JSON
      import joins the nightly fuzz targets.
- [ ] **M5-02** The 63 remaining intents, per Python group (each a feature): `staking` 14 (`add_stake`,
      `add_stake_limit`, `remove_stake`, `remove_stake_limit`, `move_stake`, `move_swap_stake`, `swap_stake`,
      `transfer_stake`, `unstake_all`, `unstake_all_alpha`, `set_auto_stake`, `stake_into_basket`, `swap_basket`,
      `swap_basket_many`); `crowdloan` 10; `proxy` 6; `children` 5 (incl. `set_take`); `coldkey` 4; `lock` 4; `multisig`
      4 (adapters and funds preflight); `registration` 3 (`claim_root`, `claim_root_with_hotkey` with the fee preview,
      `swap_hotkey`); `association` 2; `collateral` 2; `evm` 2; `leasing` 2; `root` 2; `batch` 1; `governance` 1
      (`stake_burn`); `identity` 1 (`set_identity`). 36 have generated pass-through bodies. Intent money resolves per
      field through `Money`/`Amount`/`ResolvedAmount` (the unit from the wrapped call argument, the netuid from a sibling
      field), so the root branches of `remove_stake`, `remove_stake_limit`, `move_stake`, `swap_stake`,
      `move_swap_stake` and `transfer_stake` on netuid 0 work; the oracle checks them with the root-subnet samples of
      `vendor/oracle/samples_extra.json`. With the last of them, the generated `tx` reference pages are complete.
- [ ] **M5-03** `Policy` equivalence completes at 21 of the core's 22 constructors (M2-08).
- [ ] **M5-04** The 83-op table-driven plan test over `AnyIntent`, which covers the 72 `intent_*` names: it iterates the
      extracted `INTENT_SAMPLES` (`PYT/harness/samples.py:35`), rewrites `netuid` fields to the scenario's subnet and
      `*_hotkey_ss58` fields to the test's slot hotkey, and asserts that `plan()` returns `Ok`, that `plan.op` equals the
      op, that the call data is non-empty, and that the `(pallet, call)` decoded from its first two bytes at the live
      metadata is one of the op's catalog `wraps`. An op whose `plan()` needs chain state gets a seed step, never an
      exemption. Call-tree equality with Python stays in the offline oracle tests, which use the unrewritten samples.
- [ ] **M5-05** CLI (20): `stake` aliases add, remove, move, move-swap, transfer, swap, burn; `hparams` set-take,
      stake-burn; `proxy` create, add, remove, kill, list, execute; `multisig` add, list, show, remove, pending; `tx`
      complete; hidden v9 aliases of the groups this milestone first ships (inventory §7.3).
- [ ] **M5-06** Localnet scenarios 11 (with `add_collateral`), 12, 13 and 18 (the eight ops without an upstream e2e
      test, two of them Sudo-wrapped), and scenario 8's offline-payload assertion (a payload prepared before the upgrade
      is refused, D-008); `intent_set_root_claim_type` mapped as n/a; interop lane 7 (shared caches, `btcli.json`, the
      multisig book and call cache, and a raw 2-of-3 multisig opened by one side and approved and executed by the
      other; the address and proxy books join in M8-01).

**Exit checks:**

1. The intent oracle vectors pass for 83 of 83 ops, the root-subnet samples of `samples_extra.json` included (M5-02).
2. `cargo xtask parity check` passes with no intent left in `parity/pending.toml` and the coverage gate green (M5-02).
3. The `Policy` equivalence test passes on 21 of the core's 22 constructors (M5-03).
4. Every upstream `intent_*` e2e name is mapped in `e2e_map.toml` and green: the 72 live names through the 83-op plan
   test, and the retired `intent_set_root_claim_type` as n/a (M5-04, M5-06).
5. Scenario 18 passes (M5-06).
6. C1-C7; the lanes of C2 are interop lanes 6 and 7.

### 12.8 M6: Read parity

**Goal.** All 102 reads, namespaces and the blocking facade. **Depends on** M2 for the reads, namespaces and the
`blocking` facade; M4 (CLI v0 machinery) and M5 for its 24 CLI commands [I]: `root claim`, `root allocate`, `root move`,
`root swap`, `conviction add`, `conviction mode`, `conviction accept`, `conviction move`, `collateral add` and
`collateral set-min` construct M5 intents (`ClaimRootWithHotkey`, `StakeIntoBasket`, `MoveStake`, `SwapBasket`,
`LockStake`, `SetPerpetualLock`, `SetRejectLockedAlpha`, `MoveLock`, `AddCollateral`, `SetMinCollateral`), and
`root register` the M4 `RootRegister`. The reads are off the critical path.
**Deliverables.** A minor release ([§12.1](#121-overview)); the remaining 62 reads, the remaining namespace accessors and
methods, `rtao::blocking::{SyncClient, SyncSnapshot, Subtensor, set_weights, close_shared_clients}`, 24 CLI commands,
generated `query` reference pages complete.
**Parallel work.** M6-04 is independent of the other steps.

- [ ] **M6-01** The 62 remaining reads (40 land in M1-M5), the remaining namespace accessors and methods (accessor names
      from the catalog `category`,
      [inventory §8](docs/plan/parity-inventory.md#8-inconsistencies-found-and-how-this-inventory-resolves-them) #12),
      the remaining curated extras of `PY/namespaces.py` (the metagraph and commitments extras land in M1), the
      remaining models, and the locks half of the port of `PYT/unit/test_v446_accounting_and_commitments.py` (the
      commitments half landed in M1-19). With the last read, the generated `query` reference pages are complete.
- [ ] **M6-02** The exact-hits test ([§9.3](#93-fakes-at-the-right-layers), from M2-09) over every read as it is ported,
      with its `parity/read_hits_extra.toml` rows reviewed; the read oracle at head and pinned; interop lane 5 over every
      read sample.
- [ ] **M6-03** `rtao::blocking::{SyncClient, SyncSnapshot, Subtensor, set_weights, close_shared_clients}` over
      `rtao_client::blocking::BlockingRuntime` (facade feature `blocking`; `SyncClient` has all 14 namespaces, where
      Python's has 13, D-027); the frozen legacy runtime-API table for archive reads; the port of
      `PYT/unit/test_facades.py` (namespace parity of the sync facade).
- [ ] **M6-04** The `DecodeAsType` hand-visitor experiment for `[u8; 32]` keys (architecture §8.6) [I].
- [ ] **M6-05** CLI (24): `root` list, claim, allocate, move, swap, register; `subnets` list, show, hyperparameters,
      burn-cost, create-cost, price, conviction; `conviction` list, targeting, show, add, mode, accept, move;
      `collateral` show, list, add, set-min; `query` complete; hidden v9 aliases of the groups this milestone first
      ships. `basket_index.py` is ported into `rtao-cli` as `root_helpers::basket_index`, and `basket_index_data.py` is
      generated into `rtao-cli` from extracted literal tuples, not into `rtao-catalog`, which every neuron build
      compiles.
- [ ] **M6-06** The four `test_*` keys of [inventory §2.5](docs/plan/parity-inventory.md#25-upstream-e2e-names) assigned
      to M6 (`test_subnets_all_batched`, `test_quote_stake_slippage_and_fee`, `test_metagraph_fast_path_matches_runtime`,
      `test_leases_read`) mapped to rtao e2e tests.

**Exit checks:**

1. The read oracle passes for 102 of 102 reads, at head and pinned (M6-02).
2. Interop lane 5 passes: read JSON is value-equal to Python's at a pinned block for every sample (M6-02).
3. `cargo xtask parity check` passes with no read left in `parity/pending.toml`, which is empty once M5 is also done
   (M6-01).
4. C1-C7.

### 12.9 M7: Signers, EVM, PoW GPU, timelock UX

**Goal.** Every signer and auxiliary surface of v11. **Depends on** M5 [I: EVM intents and CLI groups need it; signers
need only M2].
**Deliverables.** A minor release ([§12.1](#121-overview)); Ledger, Vault and extension signers (the extension bridge's
server and client in `rtao-signer`), the Keychain CLI commands, `rtao-evm`, timelock UX, GPU PoW, 34 CLI commands; the
Ledger emulator lane (Speculos) with recorded transcripts that `pr-gates` replay, on the PR-U17 core patch
([§5.10](#510-the-pr-u17-core-patch)) and its `core-unpatched` job; the manual Ledger device run (Q17); signer and
Ledger guides; the M7 security pass.
**Parallel work.** M7-02, M7-05 and M7-06 are independent of each other and of M7-03…M7-04; M7-09…M7-10 need only
M7-02 and may run beside M7-03…M7-08.

- [ ] **M7-01** The `vault.uos` framing functions (`sign_tx_payload`, `sign_frame`, `multipart_frames`,
      `transaction_frames`, `transaction_frames_with_proof`) join the oracle's pure-function dumps, and
      `ci/oracle/dump_ledger.py` dumps the `(payload, proof, account, index)` that Python's `LedgerSigner` sends to a
      recording fake device (the `FakeDevice` pattern of `PYT/unit/test_ledger_signer.py:34-54`) for the golden calls
      with an immortal and a mortal era and for a batch above 256 bytes (`vendor/oracle/ledger.json`).
- [ ] **M7-02** `LedgerSigner` ([architecture §6.5](docs/plan/architecture.md#65-signer-implementations)) over the
      `LedgerBackend` seam (`app_version`, `address`, `sign`): the core's `LedgerDevice` in production, `FakeLedger` for
      the ports of `PYT/unit/test_ledger_signer.py`, and `LedgerSigner::with_backend` behind `rtao-signer`'s feature
      `test-util`. It sends the unhashed payload parts with the core's RFC-0078 proof, signs on the ed25519 path, holds
      one device mutex across whole operations, runs device I/O in `spawn_blocking`, fills `ChainInfo` as architecture
      §3.9.2 specifies (the runtime's `spec_name`) and refuses raw signing; `ledger_request_parity` compares its device
      requests with M7-01's dump.
- [ ] **M7-03** `VaultSigner` (UOS framing; the upstream static assets served by `VaultSessionServer`, a one-shot
      loopback HTTP and WebSocket server on an ephemeral port that speaks the page's JSON protocol,
      `PY/vault/server.py`, upgraded like the extension bridge of M7-04; UOS frames rendered as SVG QR codes in byte
      mode without mode optimization, error correction M and a 4-module quiet zone, `PY/vault/qr.py:15-28`, with a QR
      crate chosen here [I] and tests that decode the symbol back to the frame bytes; `scan_vault_address`).
- [ ] **M7-04** `ExtensionSigner` with `BridgeServer`, `BridgeClient`, `connect_extension_signer`, `ensure_bridge`,
      `run_bridge` and `stop_bridge_daemon`, all in `rtao-signer` under feature `extension` because Python exports them
      as library API. The server serves `bridge.html` with axum and upgrades its WebSocket route through hyper and
      tokio-tungstenite 0.30 (axum's own `ws` feature would pull tokio-tungstenite 0.29, a second `tungstenite`); the
      client uses tokio-tungstenite; `ensure_bridge` spawns `rtao extension bridge --daemon`, writes Python's token and
      pid files, and reuses a running Python daemon. The `extension` commands `bridge`, `stop` and `accounts` land here,
      because `ensure_bridge` spawns the first.
- [ ] **M7-05** `rtao-evm`: keystore-V3 keys, precompile ABIs from `vendor/upstream/evm/abi/`, eth JSON-RPC, association
      proofs.
- [ ] **M7-06** Timelock UX: `rtao_tx::timelock::{seal, open, Timelocked, Reveal, OpenOptions, DrandClient}` with
      `Reveal::{In, At, Round, InBlocks}` (`InBlocks` is an addition, D-033), timelocked commitments (scenario 19);
      `pow-gpu`; the remaining §3.15 rows; the optional `subxt` feature (Q15).
- [ ] **M7-07** CLI (34, counting the three `extension` commands of M7-04): `wallet keychain` save, show, delete, over the
      M4 Keychain code, with optional Linux `keyring`; `timelock` encrypt, decrypt, show; `evm` (25 commands);
      `--signer ledger|vault|extension`; hidden v9 aliases of the groups this milestone first ships.
- [ ] **M7-08** Localnet scenarios 14 (PoW on the 12 s lane) and 19 (timelocked commitment), to which the upstream
      `test_timelocked_commitment_reveals_plaintext` key maps.
- [ ] **M7-09** Ledger emulator harness
      ([architecture §11.9](docs/plan/architecture.md#119-ledger-emulator-lane-speculos)). First the PR-U17 core patch
      ([§5.10](#510-the-pr-u17-core-patch), prototyped in
      [research J §8](docs/plan/research/J-ledger-ci-shim.md#8-core-patch-prototype)), unless `core_tag` already
      contains PR-U17: `xtask core-patch` (`rebase`, `strip`), whose first `rebase <core_tag>` pushes
      `rtao-core-patch/<core_tag>` to the fork and writes the two root `[patch]` tables and `[core.patch]` in
      `upstream.toml`; the core-patch rules of `xtask pin` (`--check` with its probe crate, and `--core-branch`, which
      drops the patch for `core-canary`); the `clippy.toml` entries that keep
      `LedgerDevice::with_transport` and `ApduTransport` inside `rtao-testkit::ledger_emu`; the `core-patch rebase` step
      of `pin-bump.yml`; and `.github/workflows/core-unpatched.yml`. Then `ci/ledger/pins.toml` with the app source
      (`LedgerHQ/app-polkadot` at its newest release tag, `v100.0.30` at `e36ced75` when this plan was written; if that
      tag does not build with the recipe, `Zondax/ledger-polkadot` `v100.0.29` at `644ec638`, the build verified while
      planning), the builder image and Speculos `v0.27.1` by digest, and the ELF sha256 per model and runner
      architecture; `cargo xtask ledger-emu {build-app, up, record, speculos-image}`; `rtao-testkit::ledger_emu`
      (feature `ledger-emu`) with one Speculos container per test, seeded with Zondax's public test mnemonic, a REST
      approver for `nanosp` (buttons) and `flex` (touch), and two transports that `LedgerDevice::with_transport` takes:
      a Speculos TCP transport (Speculos's length framing) and a replay transport; the `ledger_emu` `[[test]]` target
      with cases 1-9 of architecture §11.9; the first recordings in `vendor/recorded/ledger/` and the `ledger_replay`
      test that `pr-gates` runs.
- [ ] **M7-10** The Ledger lane on chain and in CI: cases 10 and 11 of architecture §11.9 (a funded emulator account
      signs a transfer on the fast localnet; a shielded transfer completes with both approvals on the 12 s localnet);
      `.github/workflows/ledger-emu.yml` (nightly on `nanosp` and `flex`, pushes to `main` that change the Ledger paths
      or `upstream.toml`, PRs labelled `ledger` or `pin-bump` after the loop, manual dispatch with `record` and
      `dry-run`); the `pin-bump.yml` step that re-records when the core moved (`ledger-stale` on failure); and
      the `live-probe` check that finney's runtime carries the compiled metadata hash
      ([architecture §12.6](docs/plan/architecture.md#126-live-probes)).
- [ ] **M7-11** The manual Ledger device run (Q17): write `docs/ledger-device-run.md`, a checklist limited to what the
      emulator lane cannot cover (the core's HID transport and OS device access, the app binary Ledger Live installs,
      real firmware, the review on a real screen): build `rtao` with `ledger`, record the device's app version next to
      the lane's pin, read the device's address and confirm it on its screen, sign a localnet transfer on the device and
      reject one request. Once the milestone's tag is pushed (C7), ask the user to run it on that tag (a stop-and-ask
      point: it needs the user's device) and record the result in the file's log. Every later milestone release repeats
      it ([§10.5](#105-compatibility-matrix-and-run-time-guard)).
- [ ] **M7-12** The signer and Ledger guides in the book ([§11](#11-documentation-plan)), including a note that the app
      shows stake amounts typed `TaoBalance` as raw integers
      ([research J §4.6](docs/plan/research/J-ledger-ci-shim.md#46-upstream)), kept until a released app version
      formats them (the fix was filed with Zondax on 2026-10-13, Q22; once it ships, the note says instead that
      `swap_stake_limit` and `move_stake_limit` show their `limit_price`, a price ratio scaled by 1e9, with the TAO
      unit, unless the app's maintainers handle those two calls), and CONTRIBUTING's section on running the Ledger
      emulator lane locally (`cargo xtask ledger-emu up`, with Docker).
- [ ] **M7-13** Security pass by a local cold reader ([§12.1](#121-overview)), in `docs/security/m7.md`; focus: Ledger,
      Vault and the extension bridge's token handling.

**Exit checks:**

1. The Ledger proof vector, the ports of `PYT/unit/test_ledger_signer.py` (its no-context case mapped n/a, because
   `SigningRequest` always carries its context) and `ledger_request_parity` pass (M7-01, M7-02).
2. `ledger_replay` passes in `pr-gates` over the transcripts in `vendor/recorded/ledger/` (M7-09).
3. The `ledger-emu` workflow is green on `nanosp` and `flex`: the `ledger_emu` target passes every case of
   architecture §11.9, including the localnet transfer and the shielded transfer on the 12 s localnet (M7-09, M7-10).
4. A `live-probe` run passes with the compiled-metadata-hash check against finney (M7-10).
5. `docs/ledger-device-run.md` records a passing manual device run (Q17) (M7-11).
6. `VaultSigner` UOS framing (`sign_tx_payload`, `sign_frame`, `multipart_frames`, `transaction_frames`,
   `transaction_frames_with_proof`; `PY/vault/uos.py:54-123`) is byte-equal to the oracle vectors, and a localnet round
   trip passes: `prepare_call`, then a scripted QR responder that decodes the frames and signs with a dev key, then
   `submit_signature` (M7-01, M7-03).
7. The ports of `PYT/unit/test_extension_bridge.py` and `test_extension_signer.py` pass (M7-04).
8. An interop test signs a localnet transfer through rtao's bridge driven by Python's `extension/client.py`, and through
   Python's bridge driven by rtao, with a scripted browser-side WebSocket peer signing with a dev key in place of the
   extension (M7-04).
9. The ports of `PYT/unit/test_evm.py` pass, and EVM keystore-V3 files cross-decrypt with Python (M7-05).
10. `rtao timelock encrypt` output decrypts with Python's `timelock`, and the reverse, for a past drand round (M7-06,
    M7-07).
11. Scenario 14 passes: `pow_register` e2e on the 12 s lane (M7-08).
12. Scenario 19 passes (M7-08).
13. `docs/security/m7.md` records the pass, and every finding is fixed or explicitly accepted (M7-13).
14. `cargo xtask pin --check` passes with `[core.patch]` (while it exists), and `core-unpatched` is green (M7-09).
15. C1-C7.

### 12.10 M8: CLI parity

**Goal.** Every command of the dumped Python command tree. **Depends on** M5, M6, M7.
**Deliverables.** A minor release ([§12.1](#121-overview)); the remaining 45 commands, `PyJsonFormatter`, completions,
the strict parity gate; the CLI book chapter.

- [ ] **M8-01** The remaining 45 commands: `wallet` (16: sign, verify, encrypt, decrypt, unlock, overview, registrations,
      history, transfer, set-identity, get-identity, associate-hotkey, swap-hotkey, swap-coldkey, swap-check,
      announce-coldkey-swap; `create` landed in M4); `stake` (11: unstake-all, unstake-all-alpha, show, list, auto,
      set-auto, child get/set/revoke/take, wizard); `addr` (5); `proxy book` (5); `misc` convert, latency; `crowd` list,
      info, contributors; `upgrade` pending, check, sign; the proxy and address book writers, which join interop lane 7.
- [ ] **M8-02** Hidden v9 aliases of the M8 groups, then a check of every group's hidden v9 and snake_case aliases
      against `cli_tree.json` (each alias lands with its group, inventory §7.3); `PyJsonFormatter` (2-space indent,
      Python float `repr`, non-ASCII preserved); shell completions; the interactive pickers and `stake wizard`
      (`stake_picker.py`).
- [ ] **M8-03** The CLI chapter of the book ([§11](#11-documentation-plan)).
- [ ] **M8-04** Switch the parity gate to strict mode.

**Exit checks:**

1. `cargo xtask parity check` reports `cli_tree` parity with zero missing rows (M8-01, M8-02).
2. Interop lane 9 passes for all 163 hand-written commands, each failure being a registered divergence (M8-01).
3. `cargo xtask parity check` passes in strict mode: zero pending, zero unexplained, no `pending` key in
   `e2e_map.toml` (M8-04).
4. C1-C7.

### 12.11 M9: Hardening and 1.0

**Goal.** A 1.0 that can be maintained. **Depends on** M8.
**Deliverables.** Tag `rtao-v1.0.0` with `compat.json`; the complete book and `llms.txt`; every upstream PR and issue
filed (the Ledger app item outside the monorepo since 2026-10-13, Q22); a security review report; upkeep handed over
to the bots (M9-07).

- [ ] **M9-01** Review the fuzz corpora and raise per-target time; the weekly `mainnet-clone` read-only lane
      (`mainnet-clone.yml`, on free `ubuntu-latest`, Q23). Measure the clone's disk, memory and job-duration needs;
      if they exceed free runner resources, retain this prerequisite as unresolved and seek an existing suitable
      host without adding paid or large-runner requirements. The full read-only suite remains required.
- [ ] **M9-02** Book and `llms.txt` complete; generated reference and migration tables.
- [ ] **M9-03** `rtao-neuron` feature `auto-update` (off by default) with mandatory verification of the build-provenance
      attestation that `release.yml` produces (architecture §9.11), reviewed in the security review of M9-06.
- [ ] **M9-04** Upstream filing audit: PR-U1…U17 and every issue of [§15](#15-upstream-contributions) have their Filed
      links (each was filed when its evidence existed, Q18; the Ledger app item outside the monorepo was filed on
      2026-10-13, Q22), and §15 records each one's state (open, merged, declined).
- [ ] **M9-05** One complete upstream bump executed end to end through the maintenance playbook
      ([§10.6](#106-maintenance-playbook)), by the agent like every bump of the goal loop; it waits for a finalized
      release newer than `parity_tag` ([Executing this plan](#executing-this-plan), rule 3).
- [ ] **M9-06** The full security review: a cold-reader pass ([§12.1](#121-overview)) over the whole repository, covering
      signers, keyfiles, secrets handling, the nonce store, `auto-update` and every area of the per-milestone passes, in
      `docs/security/m9.md`; an external review in addition only if the user arranges one.
- [ ] **M9-07** Handover of upkeep, with the user (a stop-and-ask point: an App and repository settings). The user
      installs the rtao-bot GitHub App (`RTAO_BOT_APP_ID`, `RTAO_BOT_PRIVATE_KEY`; contents and pull-requests write; on
      the fork too while the core patch exists, so `pin-bump` can push patch branches); the agent dispatches one live
      run each of `pin-bump` and `metadata-drift` ([§9.7](#97-ci-lanes-and-what-each-catches)) and records the handover
      and the user's choice on PR-based upkeep in the progress log. The agent's last pushes of the loop come next: the
      M9 final commit, `rtao-v1.0.0`, and the commit that records the device run on that tag (C7) and checks this step's
      box; only after them is the repository variable `RTAO_UPKEEP` set to `on` (earlier, `release-plz` would run on
      those pushes) and, if the user wants PR-based upkeep, branch protection turned on for `main`, requiring
      `check`, `no-host` and `e2e-required`, with the merge queue. If the user declines it, the agent
      first removes `drift.yml`'s `gh pr merge --auto` step (with no required checks it would fail or merge before CI
      runs), so every bot PR, `release-plz`'s included, waits for a person.

**Exit checks:**

1. `rtao-v1.0.0` exists with its compatibility row in `compat.json` and `COMPAT.md` (C7).
2. The pin bump of M9-05 is on `main`, with green `pr-gates`, `e2e-required` on `:vS` and `interop` on its push
   (M9-05).
3. Every row of [§15](#15-upstream-contributions) has a Filed link (M9-04).
4. `docs/security/m9.md` records the review, and every finding is fixed or explicitly accepted (M9-06).
5. rtao-bot is installed, one live run each of `pin-bump` and `metadata-drift` succeeded, and the progress log records
   the handover and the user's choice on PR-based upkeep; right after the agent's last push (the commit that records the
   device run, C7), `RTAO_UPKEEP` is `on` (M9-07).
6. C1-C7; the lane of C2 is `mainnet-clone`.

### 12.12 M10 (optional): post-parity compatibility crates

M10 starts only if the user asks for it. `rtao-compat-synapse`, `rtao-compat-epistula` and `rtao-transport-quic` serve
subnets migrating from v10 or using btlightning. `RemoteSigner` also lands here (facade feature `remote-signer`,
`--signer remote`, D-031): it is an rtao addition with no Python counterpart (Python's `--signer` offers wallet,
extension, ledger and vault), and it needs a wire schema, a reference server and replay tests before it can ship.

- [ ] **M10-01** `rtao-compat-synapse` (v10 Synapse, Axon and Dendrite compatibility).
- [ ] **M10-02** `rtao-compat-epistula`.
- [ ] **M10-03** `rtao-transport-quic` (btlightning).
- [ ] **M10-04** `RemoteSigner`: the wire schema (`POST /sign` with the `SigningRequest` minus secrets, authenticated by
      btauth/1 with a separate client key), a reference server that decodes and policy-checks the call before signing,
      and replay tests; facade feature `remote-signer` and `--signer remote`.

**Exit checks:**

1. Interop with a `bittensor==10.5.0` Synapse peer passes (M10-01).
2. Interop with btlightning's Python bindings passes (M10-03).
3. `RemoteSigner` signs a localnet transfer through the reference server, which refuses a replayed or tampered request
   (M10-04).
4. C1-C7.

### 12.13 Dependency order and external prerequisites

**Critical path**: M0 → M1 → M2 → M4 → M5 → M8 → M9. M3, M6 and M7 are off it and can run beside it, except that M6's
CLI part follows M5 ([architecture §14](docs/plan/architecture.md#14-milestones)). Executed by one agent, the numeric
order M0…M9 satisfies every dependency ([Executing this plan](#executing-this-plan), rule 1).

**External prerequisites and current answers.** M0-01 requested them; the user's 2026-10-14 response is Q23
([§14.1](#141-decisions-answered-2026-10-13)). Each line names the first step that needs it:

- **Testnet funds offered, setup pending.** The user will provide testnet TAO to an address prepared by the agent.
  A testnet subnet with commit-reveal on and a soak hotkey registered and staked to a validator permit still need
  setup (with btcli, or with `Scenario`'s non-sudo calls). The long-lived soak host is not yet identified; the
  24-hour run cannot fit in a GitHub-hosted job's 6-hour limit (M2-24). No funds or completed setup are assumed.
- **GitHub/GHCR authorized.** Use the user's `sam0x17` account and `ghcr.io/sam0x17/rtao-oracle` namespace (M2-05);
  verify package-write access when wiring it rather than asking again for the namespace.
- **Only free `ubuntu-latest` hosted runners.** Linux x64 CI with Docker (M0-27), a fixed glibc 2.35 Linux build
  container and ARM cross tooling (M1-23), and the mainnet-clone attempt (M9-01). No paid, large, native ARM or
  macOS hosted runners are required. Local Mac Studio checks/builds preserve macOS support (§9.8); equivalent
  isolated Keychain verification, local-build provenance and mainnet-clone resource feasibility remain to prove.
- **Downloadable docs assets.** The user declines Pages; `release.yml` attaches rustdoc and the book to releases,
  and no Pages environment or deployment rule is required (M1-23).
- **Ledger available.** The user has a device for manual runs (M7-11); actual model/app compatibility and each run's
  results remain to verify. Device operation still needs the user when the checklist is ready.

The goal loop needs no branch protection, merge queue or bot App: the agent's pushes use the user's credentials, so
they start workflow runs. At the handover (M9-07) the user adds the rtao-bot GitHub App (`RTAO_BOT_APP_ID`,
`RTAO_BOT_PRIVATE_KEY`; contents and pull-requests write), which the upkeep jobs need to push and open PRs, and, for
PR-based upkeep, branch protection on `main` requiring `check`, `no-host` and `e2e-required`, with the
merge queue ([§9.7](#97-ci-lanes-and-what-each-catches)).

Q23 supplies authorization for the listed GitHub/GHCR and free-runner choices. Do not repeat those permission
questions. Prepare a concrete testnet address and device checklist before requesting the user's corresponding
actions; request any still-missing host or credentials only when needed ([Executing this plan](#executing-this-plan),
rule 6). Product parity, the testnet soak, Keychain behavior and release verification requirements remain intact.

### 12.14 First steps

M0's steps, from the current repository state (cargo-new skeleton, edition 2024, MIT, two commits) to the M0 exit, in
order. The headings group them for reading; the IDs set the order. **Parallel work.** M0-09…M0-13 (xtask) can run beside
M0-14…M0-17 (the seam); M0-21 and M0-24 need only M0-14.

**Prerequisites and PR-U17, relicensing, workspace and pins**

- [x] **M0-01** Ask the user for the external prerequisites of
      [§12.13](#1213-dependency-order-and-external-prerequisites) (a stop-and-ask point: they cost money or need the
      user's accounts), and continue with the steps that do not need them. Create the fork of the monorepo (default
      `github.com/sam0x17/subtensor`) and file PR-U17 from it ([§15](#15-upstream-contributions),
      [§5.10](#510-the-pr-u17-core-patch)), whose evidence predates M0 (rule 7 of
      [Executing this plan](#executing-this-plan)). Start from the prototype in
      [research J §8](docs/plan/research/J-ledger-ci-shim.md#8-core-patch-prototype): its diff against `v477`, the
      checks it passed in the monorepo workspace and its emulator run; the review branch carries the same change on
      monorepo `main`.
- [x] **M0-02** Relicense the skeleton to `MIT OR Apache-2.0` (Q1, [§14.1](#141-decisions-answered-2026-10-13)): set
      `license = "MIT OR Apache-2.0"` in `Cargo.toml`; add `LICENSE-MIT`, `LICENSE-APACHE` (the Apache-2.0 text, which
      also meets Apache-2.0 §4(a) for derived material) and `NOTICE` (attribution for the vendored and derived
      RaoFoundation/subtensor material; upstream ships no NOTICE); state the per-file attribution rule in CONTRIBUTING:
      every Rust file copied or derived from upstream (staged, ported, generated from extracted tables) starts with
      `// Derived from RaoFoundation/subtensor <path> @ <commit> (Apache-2.0); modified by rtao.`, while vendored assets
      stay byte for byte and are listed in `NOTICE`. Crates inherit `MIT OR Apache-2.0`; a crate that contains a derived
      file declares `license = "(MIT OR Apache-2.0) AND Apache-2.0"`, a set that `xtask parity check` derives from the
      `rs` column of `parity/map.toml` and from the files that carry the attribution header (M0-13). Track `Cargo.lock`
      (CI runs `--locked` and the lock test parses it).
- [x] **M0-03** Workspace: move the package to `crates/rtao/` (the facade), delete the placeholder `add`, add
      `#![deny(missing_docs)]`; turn the root `Cargo.toml` into a virtual workspace: `[workspace] resolver = "3"`,
      `members = ["crates/*", "xtask"]` (with an empty `xtask` binary crate), `[workspace.package]` (edition 2024,
      `rust-version = "1.89"`, authors, `license = "MIT OR Apache-2.0"`), `[workspace.lints]`.
- [x] **M0-04** `.config/nextest.toml` with the `ci` profile ([architecture §11.1](docs/plan/architecture.md#111-layers));
      its filter, which excludes the `e2e`, `interop`, `core_oracle` and `ledger_emu` targets, lands in M0-08 with the
      targets it names.
- [x] **M0-05** `rust-toolchain.toml` (current stable), `.cargo/config.toml` alias `xtask = "run -p xtask --"`.
- [ ] **M0-06** `upstream.toml` with `[monorepo]`, `[core]` and `[parity]` at the M0 tag, the newest final monorepo
      release that finney runs and whose `:vS` localnet image is published ([§10.3](#103-a-pin-bump-step-by-step)
      step 1): `v478` / `850f2755…` on 2026-10-13, the commit this plan's evidence was checked at, whose `sdk/` equals
      `v477`'s. Also `toolchain = "1.89"`, `spec_version` = that tag's spec, `python_package_version = "11.3.1.dev0"`
      (the in-tree source version), `[networks.finney]` genesis, `[localnet]` image tags `pinned`, `mainnet`,
      `testnet`, `devnet` and `main` (digests written by `xtask pin --parity <tag> --check` once M0-09 exists; without
      `--parity`, `--check` only verifies), and the `[artifacts]` list.
- [ ] **M0-07** `[workspace.dependencies] bittensor-core = { git = "https://github.com/RaoFoundation/subtensor.git",
      tag = "<core_tag>", default-features = false }` at the M0 tag (every monorepo git dependency is declared in this
      table and inherited with `workspace = true`, so a core bump rewrites one manifest), the ring-only `rustls`
      (`default-features = false`, features `ring`, `std`, `tls12`, `logging`), `serde_json` with `arbitrary_precision`
      ([§5.8](#58-known-core-defects-and-how-rtao-handles-them) #10), and the `[patch.crates-io] w3f-bls` line.
- [ ] **M0-08** Skeletons of the other 18 crates (19 with the facade), each with one-line crate docs and the layering of
      [§4.2](#42-layers) in its manifest; `cargo build -p rtao-core` succeeds (for reference, the core alone at this tag
      locked 241 packages and built in 12.9 s wall, 84.6 s CPU, clean release:
      [bench/tagpin](docs/plan/bench/tagpin/tagpin.log)). `rtao-testkit` declares its four `[[test]]` targets (`e2e`,
      `interop`, `core_oracle`, `ledger_emu`) now, as empty files with `test = false` and the `required-features` of
      [architecture §11.1](docs/plan/architecture.md#111-layers), and the `ci` profile gains its filter: nextest rejects a
      `binary()` predicate that matches no target in the workspace (exit 96 with nextest 0.9.143, checked while
      planning), while a declared target satisfies it even when `test = false`, `required-features` or `-p` leave it
      unbuilt. Later milestones fill the files.

**xtask**

- [ ] **M0-09** `xtask pin --check`: partial clone (`git clone --filter=blob:none`) cached under `target/upstream.git`;
      the core-input fingerprint over the core's path-dependency closure, written into `upstream.toml` as `fingerprint`;
      `--check` reads the `[localnet]` digests from GHCR. Without `--parity`, `--check` only verifies the committed
      pins against the root manifest and `Cargo.lock`, offline apart from git (the `pr-gates` step of M0-27, which
      gains the core-patch rules in M7-09).
- [ ] **M0-10** `xtask import-upstream`: copy `[artifacts]` from `parity.commit`, record sha256; also vendor every
      extraction and chain-source input under `vendor/upstream/src/` and the watched-tree listing
      `vendor/upstream/tree.txt`; `--verify-only` for CI.
- [ ] **M0-11** `xtask deps-check` over `cargo metadata` and `cargo tree`: rules 1-8 of [§4.2](#42-layers) and
      `--profile no-host` (both profiles, `cargo tree -e normal,build`).
- [ ] **M0-12** `xtask extract` v0: the literal-subset extractor on `ruff_python_parser` (git, pinned rev; fall back to
      `rustpython-parser` if needed); its first run extracts the btauth/1 goldens of `PYT/unit/test_http_auth.py:19-36`
      (`FIXED_NONCE`, `BODY`, `METHOD`, `PATH`, `GOLDEN_SR25519_PAYLOAD`, `GOLDEN_ED25519_SIGNATURE`).
- [ ] **M0-13** `xtask parity check`, file layer only, over `vendor/upstream/tree.txt`: seed `parity/map.toml` with all
      357 rows of [python-module-map.md](docs/plan/python-module-map.md) as `pending`; presence is checked for every
      row, blob equality for blessed rows; the license rule of M0-02 is checked too.

**The seam and its guards**

- [ ] **M0-14** `rtao-core` re-exports (`keys`, `codec`, `runtime`, `extrinsic`, `digest`, `mlkem`, `timelock`),
      `CoreError` mapping, `DynValue`, `offload`, `BITTENSOR_CORE_TAG`/`BITTENSOR_CORE_COMMIT` generated from
      `upstream.toml`, features `host` and `ledger`; `tests/lock.rs`.
- [ ] **M0-15** `clippy.toml` with the ten `disallowed-methods` entries, `allow-invalid = true` on the seven host-gated
      ones ([§5.4](#54-not-used-on-runtime-paths)).
- [ ] **M0-16** `deny.toml` (license allowlist, advisories, SCALE-stack `multiple-versions = "deny"`, bans on
      `aws-lc-rs`, `aws-lc-sys` and `native-tls`).
- [ ] **M0-17** `rtao-core-ext` skeleton (no tokio, monorepo deny lints) with `verify_strict` as its first module.
- [ ] **M0-18** `xtask core-surface`: generate `crates/rtao-core/tests/core_surface.rs` for every re-exported core item
      and the ten `disallowed-methods` paths ([§5.4](#54-not-used-on-runtime-paths)).
- [ ] **M0-19** `xtask staged --check`: parse the three `UPSTREAM-*` header lines, generate one probe crate per module
      (its own workspace, the patch line, a copy of `Cargo.lock`), and classify failures as in
      [§5.3](#53-staged-in-rtao-core-ext).
- [ ] **M0-20** `xtask lint-upstream`: the monorepo's `subtensor-linting` (git dependency at `core_tag`, declared in
      the root `[workspace.dependencies]`, M0-07) over `rtao-core-ext` only.

**Primitives, JSON and goldens**

- [ ] **M0-21** `rtao-primitives`: ids with checked `NetUidStorageIndex`, `AccountId` (ss58 through the core),
      `DefaultAccount`, wire amounts, `Balance<U>` (no `Add`/`Sub`, `try_sum`), `Money`, `Amount`, `ResolvedAmount`,
      `Spend`, `DynBalance`, `IntoCoreValue`, scale-decode visitors, `pyfmt`, `json::{from_slice, from_str}`;
      proptests, including the properties of the hypothesis suite `test_balance_properties.py` ported with its strategy
      bounds (`RAO` 0..=2^80, `SIGNED_RAO`, `NETUID` 0..=4095) and `// pyt:` provenance (the suite has no literal cases
      to extract; the `str(Balance)` oracle vectors arrive in M2).
- [ ] **M0-22** The `serde_json/arbitrary_precision` decision of [§5.8](#58-known-core-defects-and-how-rtao-handles-them)
      #10 with its M0 tests: direct versus `Value`-first parsing of untagged and flatten floats, a u128/U256
      `to_corpus_json` round trip, and the oracle comparator's integer and float vectors.
- [ ] **M0-23** `rtao-testkit::fixtures`: the `golden.json` and shape-corpus loaders; the replay tests `golden_calls`,
      `golden_storage_keys`, `golden_signature_payloads`, `golden_extrinsics`, `golden_multisig` and `golden_ss58`, one
      per `golden.json` section (`calls`, `storage_keys`, `signature_payloads`, `extrinsics`, `multisig`, `ss58`), through
      `IntoCoreValue` and `compose_call` by name (typed builders arrive with M1's regen); the shape-corpus replay test
      `corpus_replay`.

**btauth/1, doctor, CI and docs**

- [ ] **M0-24** `rtao-signer`: `Signer`, `MultiSignature`, `KeypairSigner`, in M0 because `btauth::sign` takes a
      `&dyn Signer`.
- [ ] **M0-25** `rtao-btauth` core with every vector and negative case.
- [ ] **M0-26** `rtao-cli` with `rtao doctor` (lock check that prints the exact patch line); its `[[bin]] name = "rtao"`
      sets `doc = false` ([§11](#11-documentation-plan)).
- [ ] **M0-27** `.github/workflows/pr.yml` with `check`, `no-host`, `msrv` (`cargo +1.89`), `features`, `supply`
      ([Appendix E](docs/plan/architecture.md#appendix-e-ci-workflows)); actions pinned by SHA. `check` runs
      `xtask pin --check`, `extract --check`, the parity file layer and `xtask lint-upstream`, but not yet
      `regen --check` (M1-04); the `semver` job starts after the first tag. `cargo xtask gates` runs the same steps
      locally (every `pr-gates` step that runs on the host's OS, but no step that changes the host's own
      settings, such as M4-14's separate local temporary-keychain harness); from here on the agent runs it before
      every push (rule 2 of [Executing this plan](#executing-this-plan)).
- [ ] **M0-28** README (approach, patch line, `rtao doctor`), CONTRIBUTING (xtask commands, bless workflow, the
      attribution rule of M0-02).
- [ ] **M0-29** Run the M0 exit checks ([§12.2](#122-m0-foundations-and-pins)), then set the workspace version to
      `0.0.0` and the changelog, tag `rtao-v0.0.0` and push the tag (C7; rule 7).

## 13. Risks

From [architecture §15.1](docs/plan/architecture.md#151-risks) (likelihood / impact are [I]), plus two risks found while
writing and reviewing this plan.

| # | Risk | L / I | Mitigation |
|---|---|---|---|
| R1 | `bittensor-core` is internal and unversioned; its API follows the Python binding | high / medium | one seam crate; `core_surface` probe names the broken symbol; nightly `core-canary`; `core_tag` moves rarely; upstream PRs |
| R3 | downstream `w3f-bls` patch friction | certain until PR-U4 / low | template, `rtao doctor`, patch changes only on core bumps, PR-U4 with localnet evidence |
| R4 | C toolchain for owner and CLI builds (libsodium, vendored OpenSSL needing Perl and make, `secp256k1-sys`) | certain for those profiles / low | no-host neuron profiles, which build on the stock `rust:1.89-slim` image; an owner/CLI builder image; PR-U6 |
| R5 | the Python oracle toolchain (maturin, uv, OpenSSL, Perl) is slow or flaky | medium / medium | CI-only, cached per tag, `oracle-stale` with committed vectors authoritative (no PyPI fallback: no PyPI version reliably identifies a tag) |
| R7 | the parity long tail (23,083-line CLI, intricate preflights) holds more porting work than its line counts show | medium / medium | generated `tx`/`query`; JSON parity before formatting; `pending.toml` makes the gap a number; the CLI machinery and 118 of the 163 commands land in M4-M7, so M8 closes 45 commands and the strict gate |
| R8 | owning the WebSocket session (reconnect, resend, subscriptions) | medium / high | port of `rpc.py` with `test_rpc.py`; socket-level fault injection; `ReadMethod` makes resending a submit unrepresentable |
| R9 | reveal-round prediction: owner `trigger_epoch` after a commit, deferral, clock skew | medium / high for validators | pallet-faithful predicate, `ClockGuard`, reveal watcher with event attribution, 12 s lane, PR-U5 |
| R11 | a bug in the shape hash or the view parser mis-decodes | low / high | exact hashes, fuzzing, three-path differential tests, `RTAO_METAGRAPH_VIEW=0` escape hatch |
| R12 | public endpoint quotas and per-endpoint differences | high / medium | coalesced reads, token bucket, legacy namespace only, live probes, own-node guidance |
| R13 | btauth/1 adoption is still zero among surveyed subnets | high / medium | `legacy-v10` bridge now; compatibility crates in M10 |
| R16 | upstream declines the staged PRs | medium / low | staged code is isolated (about 2k lines) and keeps working |
| R17 | localnet differs from mainnet | certain / medium | metadata pins only from live networks; the 12 s lane; a weekly mainnet-clone read-only lane |
| R18 | secrets: KDF threads, Keychain, nonce-store memory | low / high | `Zeroizing`/`secrecy`, no `Debug` on secrets, bounded in-memory store; per-milestone security passes by local cold readers (M2, M3, M4, M7) and the full review in M9 |
| R19 (new) | `serde_json/arbitrary_precision`, forced on by the core, breaks `#[serde(untagged)]`/`#[serde(flatten)]` parsing of non-integer numbers and makes `serde_json::Value` equality spelling-sensitive, in rtao and in subnet authors' own code | certain / medium [I] | decided in [§5.8](#58-known-core-defects-and-how-rtao-handles-them) #10: rtao enables the feature itself; `Value`-first parsing at every text entry point and in rtao's number-or-string types; `rtao::btauth::server::Json<T>` for subnet authors; oracle JSON compared with integers exact and non-integers as f64; an M0 test pins all of it |
| R20 (new) | upstream churn moves the parity target during development | certain / medium | upstream tracking as standing work from M2 ([§12.1](#121-overview)); pin bumps from M2, by the agent during the goal loop and by the bot after it ([§12.4](#124-m2-validator)); the porting report sizes each bump; auto-bless covers cosmetic edits; the M8 strict gate is evaluated at the `parity_tag` current at M8 |
| R21 (new) | the Ledger emulator lane is not the production path: Speculos never runs the core's HID transport or the binary Ledger Live installs; until PR-U17 ships, the rtao workspace, and with it the device-run build and the release binaries, uses the patched core, whose `open()` path differs from upstream only by the seam; and the lane's tooling moves (API levels, image tags, removed flags) | certain until PR-U17 / medium | PR-U17 filed in M0; `[core.patch] paths` bounds the difference, and `xtask pin --check` fails once the patch is obsolete ([§5.10](#510-the-pr-u17-core-patch)); `core-unpatched` proves that shipped crates build without it; the manual device run per release covers the HID path, the shipped app and the real screen; pins by digest and a source-built ELF checked by sha256 ([§9.5](#95-python-oracle-and-interop)) |
| R22 (new) | a runtime or Ledger app change breaks clear signing for every SDK: finney runtimes 456 and 457 shipped without the compiled metadata hash, and every Ledger transaction failed with `CannotLookup` ([subtensor#3159](https://github.com/RaoFoundation/subtensor/issues/3159)) | low / high for Ledger users | the nightly emulator lane submits Ledger-signed extrinsics to the pinned localnet; `live-probe` checks finney's compiled metadata hash every 6 hours ([§9.7](#97-ci-lanes-and-what-each-catches)); upstream issues filed on evidence |

R2 (release cadence), R6 (extraction breakage), R10 (typed-model misreads), R14 (licensing, settled by Q1) and R15
(git-only Python parser) are rated low likelihood or low impact in architecture §15.1 and are not repeated here.

## 14. Decisions and open questions

### 14.1 Decisions (answered 2026-10-13)

The user answered the plan's open questions on 2026-10-13 and the prerequisite follow-up on 2026-10-14 (Q23).
The answers are final, like the decisions of
[§3](#3-fixed-decisions-and-how-the-design-reads-them); changing one is a stop-and-ask point
([Executing this plan](#executing-this-plan), rule 6).

| # | Question | Answer (verbatim) | Decision | Consequences |
|---|---|---|---|---|
| Q1 | License: switch the skeleton from MIT to `MIT OR Apache-2.0`? rtao vendors and ports Apache-2.0 upstream material (fixtures, transpiled tables, the epoch-schedule copy, static assets, ported logic), and dual licensing keeps a move into the monorepo open | "license: yes to your recommendation" | dual `MIT OR Apache-2.0` ([architecture §2.5](docs/plan/architecture.md#25-edition-msrv-toolchain-lints-license)) | M0 relicenses the skeleton first (M0-02): `license = "MIT OR Apache-2.0"`, `LICENSE-MIT`, `LICENSE-APACHE`, `NOTICE`. The Apache-2.0 obligations for derived material stay (`UP/LICENSE`; the Python package and the core are Apache-2.0, and upstream ships no NOTICE): `LICENSE-APACHE` carries the license text (§4(a)); every Rust file copied or derived from upstream (staged, ported, generated from extracted tables) starts with `// Derived from RaoFoundation/subtensor <path> @ <commit> (Apache-2.0); modified by rtao.` (§4(b), §4(c)), while vendored assets stay byte for byte and are listed in `NOTICE`; crates with derived files declare `(MIT OR Apache-2.0) AND Apache-2.0`, the others `MIT OR Apache-2.0`, and `xtask parity check` derives the set |
| Q2 | "100% Rust": Python appears only in CI (oracle, interop, upstream recorders), never in a build of rtao or a subnet binary, never in regeneration or PR gates. Acceptable? | "python ok for CI" | accepted as stated | the reading of "100% Rust" in [§3](#3-fixed-decisions-and-how-the-design-reads-them) holds; the oracle, interop and recorder lanes stay, and the Ledger emulator lane joins them in M7, because Speculos runs Python inside its image ([§9.5](#95-python-oracle-and-interop)); a Python failure can delay a pin bump (`oracle-stale`, `ledger-stale`) but never blocks a build or a PR gate |
| Q3 | Binary name: keep `rtao` only, or also ship an opt-in `btcli` shim? With shared config files a shim could surprise users who have both | "rtao only" | the binary is `rtao`; no `btcli` shim | [§4.3](#43-crates); a non-goal in [§16](#16-non-goals) |
| Q4 | btauth/1 `<Bytes>`-wrapped signatures: accept by default (Python parity) or reject by default? rtao never produces them | "yes accept both" | verification accepts plain and `<Bytes>`-wrapped signatures by default (`WrappedPolicy::AcceptLikePython`); `WrappedPolicy::Strict` stays available; rtao never produces the wrapped form | an rtao miner accepts every request a Python miner accepts ([architecture §7.1](docs/plan/architecture.md#71-core-no-io-no-tokio)) |
| Q5 | Repository location: `github.com/sam0x17/rtao`? | "yes" | `github.com/sam0x17/rtao` | the manifests of [§7.1](#71-manifests-for-the-three-profiles), the template's invocation and the template job's `[patch."https://github.com/sam0x17/rtao"]` (M3-09) |
| Q17 | Ledger in CI: a self-hosted hardware runner, or a manual release checklist? | "sure, though maybe there is some shim we can use, worth researching" | a manual device run per release stays the final release check, and the researched shim is adopted: Speculos, Ledger's device emulator, runs the real Polkadot generic app in a CI-only lane, and `pr-gates` replay its recorded transcripts in Rust ([research J](docs/plan/research/J-ledger-ci-shim.md)) | M7-09 and M7-10 build the emulator lane ([§9.5](#95-python-oracle-and-interop); [architecture §11.9](docs/plan/architecture.md#119-ledger-emulator-lane-speculos)); the core's Ledger transport cannot be swapped (G16), so PR-U17 is filed in M0 and, until it ships, rtao's workspace carries it as a core patch (Q20, [§5.10](#510-the-pr-u17-core-patch)); M7-11 writes the device-run checklist, limited to what the emulator cannot cover (the HID transport, the app binary users install, real firmware, the review on a real screen), and the user runs it; every release from M7 on repeats it ([§10.5](#105-compatibility-matrix-and-run-time-guard)) |
| Q18 | Upstream engagement: file each upstream PR when its evidence exists, or all of them in M9? | "go ahead and file them once evidence exists, I assume you are talking about upstream fixes we found?" | yes, these are the upstream fixes and improvements found while planning: the confirmed core defects (PR-U2's `Sudo.Sudid` receipt, PR-U5's epoch predicate, PR-U7's shield era), the staged-code moves (PR-U1, U6, U9, U10, U15), the public `Policy` inputs (PR-U8), the `w3f-bls` manifest change (PR-U4), the catalog and metadata publishing improvements (PR-U3, U11, U12, U16), the encrypted keyfile vectors (PR-U13), the `SelectiveMetagraphIndex` request (PR-U14), the Ledger transport seam found while researching Q17 (PR-U17) and the Python-side issues. Each is filed as soon as its evidence exists | filing in the monorepo is pre-authorized ([Executing this plan](#executing-this-plan), rule 7); the one item found later outside it, for the Ledger app, was filed with Zondax on 2026-10-13 at the user's request (Q22); [§15](#15-upstream-contributions) names each one's evidence step and records its link (common exit check C4); M9-04 audits them |
| Q19 | Security passes: M2, M3, M4 and M7 each end with a security pass by a reviewer other than the author. Who performs them? | "we will use local cold readers" | local cold readers: fresh-context reviewers given only the repository, the milestone diff and a security checklist | the procedure is in [§12.1](#121-overview); M2-26, M3-11, M4-19 and M7-13; M9-06 applies it to the whole repository |
| — | Time estimates (an instruction, not a numbered question) | "don't bake time into the plan, I'm going to have codex execute it as a goal step by step, but I wouldn't commit to specific days being specific things, AI is notoriously bad at estimating its own completion times either way" | no time estimates: no durations, effort figures, milestone dates or staffing calendars; CI cadences and upstream release-cadence facts stay | [Executing this plan](#executing-this-plan); [§1.4](#14-delivery-order); step IDs and numbered exit checks in [§12](#12-milestones); [§12.13](#1213-dependency-order-and-external-prerequisites); [§12.14](#1214-first-steps) |
| Q20 | Ledger emulator lane until PR-U17 ships: plan v2 had "a test-only copy of the core's Ledger message code" in `rtao-testkit` | "ok let's just add to the plan that we will cargo patch bittensor-core with that support until it merges" | until a `core_tag` contains PR-U17, the rtao workspace builds `bittensor-core` through a Cargo `[patch]` that adds exactly PR-U17 on top of `core_tag`; the test-only transcription survives only as a fallback if upstream declines PR-U17 | [§5.10](#510-the-pr-u17-core-patch): M0-01 files PR-U17 from the user's fork; M7-09 adds the patch branch `rtao-core-patch/<core_tag>`, the two root `[patch]` tables, `[core.patch]`, `xtask core-patch`, the core-patch rules of `xtask pin --check`, the clippy entries that keep PR-U17's items in `rtao-testkit::ledger_emu` and the `core-unpatched` job (M7 exit 14); the emulator lane and `ledger_replay` run the core's own `ledger.rs` from their first run; subnet builds never see the patch (Cargo ignores `[patch]` in dependencies); a pin bump rebases the patch, or drops it once a tag contains PR-U17; upstream declining PR-U17 is a stop-and-ask point (rule 6) |
| Q21 | Merging: the v2 summary said "Codex may merge its own PRs once the required checks pass" and asked whether every merge should need the user's approval | "we are just going to develop all of this in one goal loop, no PRs, just let codex commit until it is done" | M0 to M9 run as one continuous goal loop: no PRs in the rtao repository; the agent commits directly to `main` and pushes, without asking, until M9 exits | [Executing this plan](#executing-this-plan) rules 1, 2, 6, 7 and 8: `cargo xtask gates` (M0-27) before every push, and a red blocking lane on `main` stops the line; milestone tags pushed without approval (C7); C1 and C5 read pushes and the agent's own pin bumps; `pr-gates` and `e2e` run on pushes, the label-driven lanes on path filters ([§9.7](#97-ci-lanes-and-what-each-catches)); the jobs that push or open PRs stay off (`RTAO_UPKEEP`) with `dry-run` dispatches, and release-plz is not used; no branch protection, merge queue or bot App during the loop; M9-07 hands upkeep over (rtao-bot, `RTAO_UPKEEP=on`, branch protection if the user wants PR-based upkeep) |
| Q22 | Ledger app display: the core patch cannot change how the Ledger app shows amounts, and the app shows stake amounts typed `TaoBalance` as raw integers ([research J §4.6](docs/plan/research/J-ledger-ci-shim.md#46-upstream)); plan v2 left filing with `Zondax/ledger-polkadot` to the user's approval | "for zondax we should just submit a PR or issue"; later the same day: "yeah go ahead and open it now, ideally with a PR fixing it if it is an easy fix" | filed on 2026-10-13, before the plan runs: issue [Zondax/ledger-polkadot#295](https://github.com/Zondax/ledger-polkadot/issues/295), PR [#296](https://github.com/Zondax/ledger-polkadot/pull/296), which adds `TaoBalance` to the app's balance type names; `AlphaBalance` stays unformatted (a per-subnet token, never TAO) | [Executing this plan](#executing-this-plan): rule 6 no longer names the item (any other filing outside the monorepo stays stop-and-ask), and rule 7 pre-authorizes its follow-up (answering maintainer questions, pushing the changes they ask for to the PR branch); C4 counts it as filed; the [§15](#15-upstream-contributions) row; M7-12's guide note stays until a released app version formats the amounts; M9-04 audits it |
| Q23 | External prerequisites and CI/docs choices, 2026-10-14 | "I can provide testnet tao to whatever wallet address you need; just use my github; we will skip sophisticated github runners, I want to just use ubuntu-latest free ones; I don't see why we need github pages, that seems overkill; I have a ledger we can use" | use `sam0x17` for GitHub/GHCR; hosted CI is free `ubuntu-latest` Linux x64 only; downloadable rustdoc/book release assets replace Pages; testnet funding is offered and a Ledger is available | §12.13 records what is still pending; §9.8 and architecture Appendix E preserve platform support through fixed glibc 2.35 build containers, Linux ARM cross tooling and local Mac Studio builds/checks; Keychain parity, trustworthy local-build provenance, clone resources, soak host/setup and manual device results require evidence, not assumed completion |

### 14.2 Resolved by measurement or with upstream

From [architecture §15.2](docs/plan/architecture.md#152-decisions-and-open-questions):

| # | Question | Resolved by | Milestone |
|---|---|---|---|
| Q6 | real public-endpoint quotas, to tune the default token bucket | measurement | M1 [I] |
| Q7 | will RaoFoundation accept PR-U1…U17, especially U4, U5, U6? | upstream; each PR is filed when its evidence exists ([§15](#15-upstream-contributions)) | from M0 (PR-U17) |
| Q8 | does a crates.io `w3f-bls 0.1.3` std build produce ciphertexts pallet-drand decrypts? | `ci/pr-u4-probe/` (scenario 16); the four-file source diff predicts yes | M2 |
| Q9 | should upstream account for `MaxEpochsPerBlock` deferral in `get_next_epoch_start_block` and the reveal predictor? | upstream question, filed at M2-17 ([§15](#15-upstream-contributions)) | M2 |
| Q10 | is there a root setter that forces the queued `register_network` path on localnet (`SubnetLimit`)? | **resolved from source**: root `sudo_set_network_immunity_period(0)` (AdminUtils call 35) then `sudo_set_subnet_limit(<current non-root count>)` (call 37) make the next `register_network` prune the lowest-priced subnet and queue (`UP/pallets/admin-utils/src/lib.rs:1215-1228, 1250-1258`; `UP/pallets/subtensor/src/subnets/subnet.rs:204-213, 233-260`; `coinbase/root.rs:358-361`); scenario 10 runs it in the `global` container ([§9.4](#94-localnet-e2e)) | M4 |
| Q11 | when will the runtime or the SDKs move to extrinsic v5? | drift alert | continuous |
| Q12 | will `SelectiveMetagraphIndex` be exposed through metadata or a runtime API? | upstream (PR-U14, filed at M1-03) | M1 |
| Q13 | will `:v<spec>` localnet images keep appearing for every finalized release? | **resolved**: `docker-localnet.yml` publishes `:v<spec>` on every mainnet mirror move (`UP/.github/workflows/docker-localnet.yml:7-9`); `xtask pin --check` waits for the image, and `wasm-override` is a manual fallback | — |
| Q14 | is `Metagraph.total_stake` intentionally declared `TaoBalance` while carrying stake weight? | upstream question (D-019, filed at M1-16) | M1 |
| Q15 | does `subxt` 0.51 build with `default-features = false` over a custom `RpcClientT` without jsonrpsee? | build | M7 |
| Q16 | is tokio-tungstenite 0.30's handshake error API identical to 0.28's? | `FakeNode` test | M1 |

### 14.3 Gaps found while planning

The drafting step recorded fourteen gaps in the architecture. The review step decided all of them, so none remains
open: each decision lives in the section that owns it, and the items that still need upstream action are tracked as
upstream PRs and questions ([§15](#15-upstream-contributions), [§14.2](#142-resolved-by-measurement-or-with-upstream)).
For references to the drafting numbers (python-module-map.md cites some):

| # | Gap | Decided in |
|---|---|---|
| 1 | `serde_json/arbitrary_precision` forced on every build | [§5.8](#58-known-core-defects-and-how-rtao-handles-them) #10, [§7.3](#73-a-miner), R19 |
| 2 | pass-through generator timing | M2 ([§12.4](#124-m2-validator)) |
| 3 | exact-hits test and the reads whose catalog hits are incomplete | [§9.3](#93-fakes-at-the-right-layers) |
| 4 | 8 ops without a localnet test | scenario 18 and the 83-op plan test ([§9.4](#94-localnet-e2e), [§12.7](#127-m5-intent-parity)) |
| 5 | commitments/metagraph layering cycle | M1 ([§12.3](#123-m1-read-path-and-the-first-typed-read)) |
| 6 | hyperparameter units across primitives and catalog | M4 ([§12.6](#126-m4-owner-tooling-shield-cli-v0)) |
| 7 | `basket_index.py` home | M6 ([§12.8](#128-m6-read-parity)) |
| 8 | `PY/config.py` home | M1 ([§12.3](#123-m1-read-path-and-the-first-typed-read)) |
| 9 | extension bridge and the WebSocket dependency rule | [§4.2](#42-layers) rule 2, M7 ([§12.9](#129-m7-signers-evm-pow-gpu-timelock-ux)) |
| 10 | core constructors without a same-named op | [§5.8](#58-known-core-defects-and-how-rtao-handles-them) #9 |
| 11 | clippy paths behind `host` | [§5.4](#54-not-used-on-runtime-paths) |
| 12 | M0 btauth core and `rtao doctor` scheduling | M0 ([§12.2](#122-m0-foundations-and-pins)) |
| 13 | no `map.toml` kind for vendored assets | `vendored` ([§8.3](#83-how-parity-is-enforced)) |
| 14 | `Sudo.Sudid` receipt field | [§5.8](#58-known-core-defects-and-how-rtao-handles-them) #4 |

### 14.4 Gaps found in the final review

The completeness review that followed the six-lens review found these and decided each; the decision lives in the
section named, and architecture.md carries the same decision.

| # | Gap | Decided in |
|---|---|---|
| 15 | `NeuronCx` and the `Validator` trait (M2) hold a `MetagraphHandle` and a `SignedClient` and take a `MetagraphDiff`, which were scheduled in M3 | M2 ([§12.4](#124-m2-validator), [§12.1](#121-overview)) |
| 16 | localnet scenario 8 (runtime upgrade mid-session) had no milestone | M2, its offline-payload assertion in M5 ([§9.4](#94-localnet-e2e)) |
| 17 | the `pin-bump`, `oracle-refresh`, `upstream-next`, `core-canary`, `live-probe`, `features-powerset`, `release-plz` and `release` workflows, `xtask compat` and the run-time compatibility guard had no milestone, although upstream tracking starts in M2 | M1 and M2 ([§12.3](#123-m1-read-path-and-the-first-typed-read), [§12.4](#124-m2-validator)); M2 exits with a reproducible re-pin at the pinned tag |
| 18 | `e2e_map.toml` had no state for a key whose rtao test does not exist yet, and 15 of the 39 upstream `test_*` names had no milestone | `rtao`/`na`/`pending` states ([§9.4](#94-localnet-e2e)); [inventory §2.5](docs/plan/parity-inventory.md#25-upstream-e2e-names) |
| 19 | the Vault signer's loopback WebSocket page server fell outside dependency rule 2, and no crate rendered its QR frames | [§4.2](#42-layers) rule 2; M7 ([§12.9](#129-m7-signers-evm-pow-gpu-timelock-ux)) |
| 20 | `cargo doc --workspace` lets the `rtao` binary's docs overwrite the facade's in the docs archive | `doc = false` ([§11](#11-documentation-plan)) |
| 21 | `rtao wallet save-password-env`, the three profile examples, `rtao_metagraph::persist`, `auto-update` and `lint-upstream` had no milestone | M4; M2, M3 and M4; M3; M9; M0 ([§12](#12-milestones)) |
| 22 | no rtao CI job generated the template and tested it against the PR's commit (the template's own CI would build the last tag), and template-only PRs skipped the localnet | the `template` job in `e2e.yml`, part of `e2e-required` (M3, [§12.5](#125-m3-miner); [§9.7](#97-ci-lanes-and-what-each-catches)) |
| 23 | the completeness layer checked only the root `__all__`, not the 14 other public `__all__` lists (`evm` alone exports 38 names) | [§8.3](#83-how-parity-is-enforced) completeness row and `parity/exports_extra.toml` (M2) |
| 24 | the 24-hour testnet soak had no host, although GitHub-hosted jobs stop after 6 hours | a long-lived host, an external prerequisite ([§12.4](#124-m2-validator), [§12.13](#1213-dependency-order-and-external-prerequisites)) |
| 25 | nothing compared the `--json` output of the 163 hand-written commands with btcli's, although JSON is the CLI's parity contract (`--json` is global); lanes 5 and 8 cover only reads, `tools` and `tx` plans | interop lane 9 through `PinProxy` (M4, extended per milestone; [§9.5](#95-python-oracle-and-interop)); M8 exit |
| 26 | python-module-map.md marked only 11 Python unit-test files `ported`, while architecture §11.2 ports `test_lock_ownership_gate` and the M7 exit requires ports of `test_ledger_signer`, `test_extension_bridge`, `test_extension_signer` and `test_evm`; five behavior tests the oracle cannot reach (PoW challenge, owner pruning, name ordering, facades, commitments accounting) were `oracle-only` | 21 files `ported`, each in the milestone of its code ([§9.2](#92-upstream-fixtures-reused)) |
| 27 | architecture Appendix E's `nightly.yml` did not parse as YAML (a `: ` inside the plain `run:` scalar of the `core-canary` issue step), so GitHub would reject the whole nightly workflow: `upstream-next`, `core-canary`, `fuzz`, `bench` and `features-powerset` | a block scalar; every workflow block of Appendix E now parses ([architecture Appendix E](docs/plan/architecture.md#appendix-e-ci-workflows)) |

## 15. Upstream contributions

None of these blocks rtao; each staged module is deleted when its PR ships in `core_tag`
([architecture Appendix D](docs/plan/architecture.md#appendix-d-upstream-contribution-plan)). The user decided (Q18,
[§14.1](#141-decisions-answered-2026-10-13)) that each PR and issue is filed as soon as its evidence exists. "Evidence
at step" names the step whose output completes the evidence; filing is pre-authorized
([Executing this plan](#executing-this-plan), rule 7), and the link goes into the Filed column (common exit check C4,
[§12.1](#121-overview)).

| PR | What | Why | Evidence | Evidence at step | Filed |
|---|---|---|---|---|---|
| PR-U1 | public sans-IO protocol functions in a new ungated module `sdk/bittensor-core/src/protocol/{metadata,storage,runtime_api,extrinsic_lookup,dispatch_error,fee}.rs`, which the blocking client also calls (metadata fetch, storage decode, runtime-API encoding, extrinsic lookup, outcome and dispatch-error decode), plus the `TransactionPaymentApi_query_info` probe | private today; fee estimation uses deprecated `payment_queryInfo` | `CORE/client.rs:469-499, 551-576, 942-1048, 1234-1365` | M2-06 (with `metadata`, `storage` and `runtime_api` from M1-02) | — |
| PR-U2 | Python-parity receipts in an ungated `protocol/receipt.rs` (`Sudo.SudoAsDone`, `Utility.BatchInterrupted`/`ItemFailed`, MevShield failures, fee fallback, and `Sudo.Sudid`'s `sudo_result` field) | the core misses nested failures, and reports a failed `Sudo.sudo` as success | `CORE/client.rs:1024-1044` vs `PY/_transport/utils/receipt.py:8-16`; the `Sudid` defect ([§5.8](#58-known-core-defects-and-how-rtao-handles-them) #4); a `FakeChain` receipt test with a failed `Sudid` (M2) and scenario 18's Sudo-wrapped ops (M5) | M2-06; scenario 18 (M5-06) adds live evidence to the open PR | — |
| PR-U3 | export custom transaction codes, invalid-transaction needles, dispatch errors, explanations and help overrides in `errors.json` | removes extraction from `error_map.py`/`result.py` | `PY/error_map.py:488, 524, 555`; `PY/result.py:116-287` | M2-03 | — |
| PR-U4 | `w3f-bls = { version = "=0.1.3", features = ["std"] }` in the core manifest | removes every downstream patch line | `UP/sdk/bittensor-core/Cargo.toml:62`; E0277 without the patch; `ci/pr-u4-probe/` (scenario 16): offline cross-decryption and a localnet reveal; the four-file diff between crates.io `w3f-bls` 0.1.3 and `vendor/w3f-bls` | M2-20 (scenario 16) | — |
| **PR-U5** | `should_run_epoch` uses `> tempo` and the simulator caps the counter at `tempo + 1`; `generate_commit_v2_at(now, …)` | **confirmed bug**: commits can be lost silently after an `EpochSkipped` slot or a tempo lowered below the step counter, for the core's and Python's users | [§5.8](#58-known-core-defects-and-how-rtao-handles-them) #2, #3 | M2-11 | — |
| PR-U6 | split `host` into `keyfiles-json`, `keyfiles-crypto`, `drand-http`, `rpc-client`, `parallel`, `pow` | neurons need only the pure JSON codec | `CORE/lib.rs:10-31`; `CORE/keyfiles/mod.rs:179-370` | M2-02 | — |
| **PR-U7** | `submit_shielded` signs carrier and inner with era ≤ 8 and resolves the inner receipt | **confirmed bug**: the runtime rejects the core's era-64 carrier as `Stale`; fixes the core's own users and retires nothing in rtao, which never calls it | [§5.8](#58-known-core-defects-and-how-rtao-handles-them) #1 | M4-15 (scenario 11) | — |
| PR-U8 | public `PolicySubject`/`PolicyInput` | any intent layer can reuse `Policy::check`; retires nothing in rtao, whose equivalence test already builds the core with `host` | `CORE/transaction.rs:55-90, 929-976` | M2-08 | — |
| PR-U9 | `Runtime::metadata_ir()` shared by the Python binding and rtao, with storage hashers, key idents, event fields and runtime-API signatures | the IR exists only in the PyO3 binding | `UP/sdk/bittensor-core-py/src/runtime.rs:554, 579` | M1-02 | — |
| PR-U10 | pure `encode_password_for_environment`; public `keys::verify_strict` | `set_var` is unsound with threads; strict verification is private | `CORE/keyfiles/mod.rs:126-138`; `CORE/keys/mod.rs:78-100` | M2-02 (`keyfile_env`; `verify_strict` exists from M0-17) | — |
| PR-U11 | `default` values and `mev_shield_*`/`all_amount_fields` class variables in `intents.json` | the catalog carries no defaults today | 0 catalog properties carry `default` | M2-03 | — |
| PR-U12 | publish V15 and V16 metadata as release assets | the drift job can skip live fetches | — | M1-21 | — |
| PR-U13 | committed encrypted keyfile vectors (NaCl, Fernet, Ansible) | upstream has none | interop-recorded cross vectors | M3-08 | — |
| PR-U14 | expose `SelectiveMetagraphIndex` through metadata or a runtime API (issue) | removes `syn` extraction from pallet source | `pallets/subtensor/src/rpc_info/metagraph.rs` | M1-03 | — |
| PR-U15 | move `Value` navigation helpers out of host-gated `client.rs` | no-host consumers need them | `CORE/client.rs:1515-1570` | M1-02 | — |
| PR-U16 | `generate.py` `read_targets` (`:405-480`) scans each read's module-local helpers and module-level `Item(...)`/`Method(...)` descriptors | the catalog `hits` under-report 12 reads; `parity/read_hits_extra.toml` would shrink to the out-of-module items | [inventory §5.15](docs/plan/parity-inventory.md#515-hits-the-catalog-omits) | M2-09 | — |
| PR-U17 | an `ApduTransport: Send + Sync` trait in `sdk/bittensor-core/src/signers/mod.rs`, outside the `ledger` gate, implemented by the HID transport, and `LedgerDevice::with_transport(transport: impl ApduTransport + 'static) -> Self`; `open()` unchanged and `LedgerDevice` still `Send + Sync`; optionally a std-only TCP transport for Speculos; prototyped while planning, 108 added lines including three unit tests ([research J §8](docs/plan/research/J-ledger-ci-shim.md#8-core-patch-prototype)); filed from the user's fork, whose `rtao-core-patch/<core_tag>` branches carry it into rtao's workspace until it ships ([§5.10](#510-the-pr-u17-core-patch)) | the core's Ledger transport is a private concrete HID type, so neither rtao nor upstream can run the Ledger path against Speculos, Ledger's emulator (G16) | `CORE/signers/mod.rs:6-9`, `CORE/signers/ledger.rs:51-65, 158-219`; the emulator runs of [research J](docs/plan/research/J-ledger-ci-shim.md), the last through the prototype | M0 (the evidence predates the plan); M7-09's transcripts add to the open PR | [RaoFoundation/subtensor#3242](https://github.com/RaoFoundation/subtensor/pull/3242), filed 2026-10-14 |

PR-U1 and PR-U2 target an ungated `protocol` module because `staged --check` probes without `host`: a symbol that lands
only behind `host` retires no staged module that the no-host profile uses. PR-U17 retires the core patch of
[§5.10](#510-the-pr-u17-core-patch) rather than a staged module; its evidence predates M0, so it is filed during M0
(C4).

Issues to file (Python package, docs, scripts and runtime), each when the pinning test or evidence named exists:

| Issue | Evidence at step | Filed |
|---|---|---|
| dead `None` checks on zero-account defaults (D-001) | M1-03 | — |
| unpinned gather reads (D-002) | M4-07 (`delegate_take` and `hotkey_identities` under the single-block test) | — |
| `Snapshot.constant` reading the head codec (D-007) | M1-14 | — |
| `attach_signature` ignoring `transaction_version` (D-008) | M5-01 | — |
| `ServeAxonTls` certificates over 65 bytes silently dropped on chain (D-017) | M3-03 | — |
| the docs' hotkey-shield claim (D-018) | M4-03 | — |
| `Metagraph.total_stake` declared `TaoBalance` while carrying stake weight (D-019, question Q14) | M1-16 | — |
| limit-price call arguments typed as `TaoBalance` (D-022) | M1-03 | — |
| `retracted` and `finalityTimeout` treated as fatal although the extrinsic may still land (D-024) | M2-07 | — |
| a shielded submission proceeding after the finality lag expired its era (D-025) | M4-03 | — |
| the nonce cache left at n after a shielded submission (D-026) | M4-03 | — |
| `SyncClient` lacking the `collateral` namespace (D-027) | M6-03 | — |
| nested module errors resolved through the generated table (D-028) | M2-08 | — |
| `scripts/record_golden.py` hard-coding `SubtensorModule.Alpha`, which should become `AlphaV2` | M2-22 (`xtask record` lists it in `dropped.json`) | — |
| the deferral question (Q9) | M2-17 (the reveal watcher attributes misses) | — |
| outside the monorepo, for `Zondax/ledger-polkadot`: stake amounts typed `TaoBalance` appear on the device as raw integers (`DOTAPP/app/src/metadata_parser.c:74-75`); the PR adds `TaoBalance` to the app's balance type names, and `AlphaBalance`, a per-subnet token, stays unformatted (Q22) | — (the emulator runs of [research J](docs/plan/research/J-ledger-ci-shim.md), before the plan runs) | filed 2026-10-13: issue [Zondax/ledger-polkadot#295](https://github.com/Zondax/ledger-polkadot/issues/295), PR [#296](https://github.com/Zondax/ledger-polkadot/pull/296) |

## 16. Non-goals

Deliberately not done, or not yet ([architecture §17](docs/plan/architecture.md#17-what-we-deliberately-do-not-do-yet)):

| Not doing | Why | Revisit when |
|---|---|---|
| subxt codegen or `OnlineClient` on runtime paths (only an opt-in escape hatch) | a second metadata engine, encoder and signer; 8 breaking minors in 23 months | never for core paths |
| `chainHead_v1`/`archive_v1`/`transaction_v1`; custom `subnetInfo_*` RPCs | v11 uses the legacy namespace; `CombinedBackend` fails on finney WS; custom RPCs lag runtime APIs | public endpoints deprecate the legacy namespace |
| extrinsic v5 signing | the core and Python sign v4 | the core or Python adopts v5 |
| a second encoder, positional codec types beyond `MetagraphView`, baked hashers | drift surface | never |
| parsing V16 at run time | the core rejects it; V15 carries everything RFC-0078 needs | the core accepts V16 or V15 disappears |
| modeling `MaxEpochsPerBlock` deferral | safe direction; unpredictable inputs | reveal-miss metrics blame deferral, or upstream models it |
| dropping the downstream `w3f-bls` patch | two sources, E0277 | PR-U4 merges and the decrypt proof passes |
| pure-Rust NaCl, Fernet or Ansible keyfile crypto | the core owns COMPAT bytes; neurons need only plaintext hotkeys | PR-U6 is declined and no-host coldkey unlock becomes a requirement |
| following `CommitRevealWeightsVersion` or `WeightsVersionKey` automatically | both are upgrade levers | never |
| v10 Synapse/Axon/Dendrite emulation, Epistula, btlightning QUIC | outside the v11 parity target | M10, or earlier if a migrating subnet needs it |
| `RemoteSigner` (`--signer remote`) | an rtao addition with no Python counterpart; it needs a wire schema, a reference server and replay tests before it can ship | M10 |
| light client (smoldot) | finney's chain spec lacks a `lightSyncState` | upstream ships a light-client-ready spec |
| crates.io publication | user decision | the user changes the constraint |
| a `btcli` shim next to the `rtao` binary | the user chose `rtao` only (Q3); with shared configuration files a shim could surprise users who have both | the user asks for one |
| a uhid virtual-HID bridge that runs the core's unmodified HID path against the Ledger emulator | unverified on GitHub-hosted runners (it needs the `uhid` module and root), impossible on Docker Desktop, Linux-only; it adds only the HID framing that the device run covers on real hardware | upstream declines PR-U17 and the user chooses it over carrying the core patch or a test-only transcription (rule 6, [§5.10](#510-the-pr-u17-core-patch)) |
| Python at build or run time, or in PR gates | the 100%-Rust goal | never |
| `jsonschema` validation | Python validates through dataclasses | never |
| re-exporting the core's blocking `Client` as `rtao::blocking` | diverges from Python's product behavior | never (it stays a test oracle) |
| Rich-identical human tables; byte-identical JSON as a gate | value equality is the contract | users need byte-identical text |
| reading v10 `.pt` metagraph pickles or `state.npz` | not part of v11 | a migrating subnet asks |
| auto-update on by default | supply-chain risk; when enabled, an asset without a valid GitHub build-provenance attestation from rtao's release workflow is refused (a `SHA256SUMS` match alone proves nothing) | never by default |
| dense weight or bond matrices | 6.25M cells at 2,500 UIDs | never |
| client-side stake-weight recomputation for refusals | false negatives | upstream exposes an exact stake-weight API |
| proposing `sdk/bittensor-rs` upstream before parity | needs a faithful twin first | after M8 |

## 17. Glossary

| Term | Meaning |
|---|---|
| intent | a catalog write operation (83 at `parity_tag`), e.g. `add_stake`: a generated struct plus an `Intent` impl with `build`, `preflight` and hooks mirroring Python's `Intent` class |
| read | a catalog read operation (102), e.g. `delegate_take`, reached through a namespace (`client.delegation()`) or `dispatch` |
| op | the catalog name of an intent (e.g. `add_stake`): the tool name for `execute_tool`, the `rtao tx <op>` subcommand, the `AnyIntent` variant |
| `parity_tag` | the monorepo release rtao mirrors (catalogs, Python sources, goldens, oracle, localnet image); moves on every finalized mainnet release |
| `core_tag` | the monorepo tag Cargo builds `bittensor-core` from, which every downstream `w3f-bls` patch line must match; moves only on a core-input fingerprint change |
| core-input fingerprint | BLAKE3 over the core's source tree, manifest, `vendor/w3f-bls`, inherited workspace dependencies and root patch table |
| `upstream.toml` | the single pin file (`[core]`, `[parity]`, `[localnet]`, `[networks.*]`, `[artifacts]`, and `[core.patch]` while the core patch exists), written only by `xtask pin` and, for `[core.patch]`, `xtask core-patch` |
| core patch | the two Cargo `[patch]` tables in rtao's root manifest that build `bittensor-core` from `rtao-core-patch/<core_tag>` in the user's fork (`core_tag` plus PR-U17) and keep the core's other path dependencies at `core_tag`, until a `core_tag` contains PR-U17; subnet builds never see them ([§5.10](#510-the-pr-u17-core-patch)) |
| staged module | a module of `rtao-core-ext` written as upstream code, with `UPSTREAM-TARGET`, `UPSTREAM-PROBE` and `UPSTREAM-FEATURES` headers, deleted when its probe (a symbol at the ungated target, or its vector suite) passes against the pinned core without `host` (`xtask staged --check`) |
| seam | `rtao-core`, the only crate that names `bittensor_core` in non-test code |
| pass-through body | a generated `build()` for an intent that wraps one call whose arguments map one-to-one onto fields (46 of 83; the generator lands in M2) |
| oracle | the pinned Python SDK run in CI to produce committed JSON vectors that ordinary `cargo test` compares against |
| interop lane | CI lane running `btcli` and Python neurons against rtao over a localnet |
| `core-oracle` lane | CI lane comparing rtao with the core's blocking client on one localnet block |
| shape hash | `blake2_256` of a structural encoding of a metadata type: exact (all fields, order, types) for calls and the metagraph view; projection (declared fields only) for by-name models |
| surface verification | `verify_surface` at connect and on every runtime upgrade; fails closed per call (refuses signing a changed call; typed reads fall back to dynamic) |
| MetagraphView | the borrowed, columnar parser of `get_metagraph`'s SCALE response, used only when its exact shape hash matches the live spec |
| `FakeNode` | a real loopback JSON-RPC WebSocket server with fault injection, for testing the session |
| `FakeChain` | an `RpcClientT` implementation over shared fake state, for testing client, executor, reads and scheduler; a `Client` runs over it through `Client::from_rpc` |
| `RecordingView` | a view wrapper that records every descriptor and block hash a read touches |
| `Scenario` | the localnet builder that creates a subnet, owner, validator, miner and stake per test |
| `NonceLease` | an RAII nonce reservation: settled on inclusion (or pool acceptance), otherwise its drop clears the cache so the next lease asks the pool-aware `account_nextIndex` |
| `FateUnknown` | a transmitted submission whose connection dropped; never resent; resolved by reconciliation |
| `ReadMethod` | the allowlist of RPC methods the session may resend after a reconnect; `author_submit*` is absent |
| `ShieldPeriod` | the only era type of the MEV-shield path: `P4` or `P8` (chain maximum 8); a submission whose era the finality lag would expire before inclusion is refused (D-025) |
| `CallBytes` | encoded call bytes stamped with spec and transaction version; refused across a runtime change |
| `KeySource<Role>` | compile-time signer roles: `HotkeyFile` implements only `KeySource<Hotkey>`, so coldkey intents do not compile in a neuron |
| `Balance<Tao>` / `Balance<Alpha>` / `DynBalance` | typed money (TAO plus alpha does not compile) and Python's netuid-tagged balance for Python-shaped output |
| `pending.toml` | the shrink-only list of ops and reads not yet ported; stubs return `NotYetPorted` |
| `map.toml` | one row per watched upstream file with its Rust home, kind and the git blob SHA last reconciled (357 rows at v477, seeded `pending` in M0) |
| divergence register | `parity/divergences.toml`: each deliberate difference from Python, with class, test and upstream link |
| bless / auto-bless | recording a new upstream blob SHA after a port; automatic only when symbols, tables and mapped oracle vectors are unchanged |
| drift classes | `surface-unchanged`, `additive`, `behavioural`, `breaking` (classification of a new runtime's metadata) |
| no-host profile | neuron builds without `bittensor-core/host`: no libsodium, OpenSSL, `secp256k1-sys`, Perl or CMake; their only native code is `ring` and `blake3` SIMD, built by the C compiler rustc already uses as its linker |
| `rtao doctor` | CLI check of a downstream workspace (the patch line against the workspace's own `bittensor-core` entry, toolchain, endpoints, clock skew, wallet permissions, docker) |
| `StateCell` | the framework's shared neuron state, over `std::sync::RwLock`: its guards are `!Send`, so holding one across an `.await` does not compile |
| `RuntimeHandle` | `rtao_runtime::RuntimeHandle(Arc<RuntimeEntry>)`: a parsed V15 runtime with its surface report, shape cache and RFC-0078 digest, cached per `(genesis, spec)` by `rtao_client::RuntimeCache` |
| `Money` / `Amount` | intent money input: `Money` is an untagged or netuid-tagged amount and `Amount` adds Python's `"all"`; each field resolves to a `DynBalance` by the call argument's unit and a sibling netuid field |
| `SubnetsMetagraphExt` | the extension trait in `rtao::prelude` that provides `client.subnets().metagraph(netuid)` |
| `e2e-required` | the e2e workflow's gate job: during the goal loop a stop-the-line signal and part of C1, after it a required check if the user turns on branch protection; it fails only when the `changes` job, a localnet job or the template job failed or was cancelled |
| goal loop | the continuous execution of M0 to M9 in which the agent commits to `main` and pushes without PRs, after `cargo xtask gates`, the local equivalent of `pr-gates` ([Executing this plan](#executing-this-plan), Q21) |
| `RTAO_UPKEEP` | the repository variable that turns on the jobs that push or open PRs (`metadata-drift`, `pin-bump`, `oracle-refresh`'s PR, `release-plz`, `ledger-emu`'s `record` commit); off during the goal loop, `on` from the handover (M9-07) |
| btauth/1 | v11's signed-HTTP protocol between neurons (eight-line payload, `X-Bittensor-*` headers) |
| Speculos | Ledger's device emulator (LGPL-3.0; a Python program with QEMU, in a Docker image), which runs the Polkadot generic app headless, takes APDUs over TCP, and exposes buttons, touch and screen events through a REST API |
| `ledger-emu` lane | the CI-only lane that runs rtao's `LedgerSigner` against Speculos, records APDU transcripts that `ledger_replay` replays in `pr-gates`, and submits Ledger-signed extrinsics to a localnet ([architecture §11.9](docs/plan/architecture.md#119-ledger-emulator-lane-speculos)) |
| `LedgerBackend` | `rtao-signer`'s device seam under `LedgerSigner`: the core's `LedgerDevice` in production, fakes and emulator backends in tests |
| step ID | the stable ID of a milestone step, `M<k>-<nn>` (e.g. `M2-07`); steps run in ID order ([Executing this plan](#executing-this-plan)) |
| exit check | a numbered, objective check (a command, a named test or a CI lane) that must pass before a milestone exits; C1-C7 apply to every milestone ([§12.1](#121-overview)) |
| cold reader | a fresh-context security reviewer, run locally, that took no part in the implementation and receives only the repository, the milestone diff and the security checklist ([§12.1](#121-overview)) |
| stop-and-ask point | an action the executing agent takes only after the user approves it ([Executing this plan](#executing-this-plan), rule 6) |

## 18. References

### 18.1 Documents and sources

- Design: [architecture.md](docs/plan/architecture.md) (sections 0-17, appendices A-F);
  [parity-inventory.md](docs/plan/parity-inventory.md); [python-module-map.md](docs/plan/python-module-map.md).
- Research ([research/](docs/plan/research/)): [A1](docs/plan/research/A1-v11-intents-transport.md) v11 intents and
  transport; [A2](docs/plan/research/A2-v11-reads-metagraph-codegen.md) reads, metagraph, codegen;
  [B](docs/plan/research/B-bittensor-core-rust.md) bittensor-core; [C1](docs/plan/research/C1-chain-extrinsics-hyperparams-tx.md)
  chain transactions; [C2](docs/plan/research/C2-chain-state-runtime-apis-shapes.md) chain state and runtime APIs;
  [D](docs/plan/research/D-neuron-protocols.md) neuron protocols; [E](docs/plan/research/E-existing-rust-efforts.md)
  existing Rust efforts; [F](docs/plan/research/F-rust-stack-options.md) Rust stack;
  [G](docs/plan/research/G-owner-validator-miner-lifecycle.md) operator lifecycle;
  [H](docs/plan/research/H-testing-and-upstream-tracking.md) testing and upstream tracking;
  [I](docs/plan/research/I-legacy-sdk-and-docs-corpus.md) legacy SDK and docs corpus;
  [J](docs/plan/research/J-ledger-ci-shim.md) Ledger signing in CI (the emulator shim, after the user's decisions);
  [CORRECTIONS.md](docs/plan/research/CORRECTIONS.md) (overrides the reports); feasibility checks
  [bittensor-core as a git dependency](docs/plan/research/feasibility-bittensor-core-gitdep.md),
  [subxt against finney](docs/plan/research/feasibility-subxt.md),
  [core plus subxt coexistence](docs/plan/research/feasibility-veneer-coexistence.md).
- Panel ([panel/](docs/plan/panel/)): proposals [A](docs/plan/panel/arch-A-mirror-python.md),
  [B](docs/plan/panel/arch-B-type-driven.md), [C](docs/plan/panel/arch-C-operator-first.md),
  [D](docs/plan/panel/arch-D-minimal-custom-surface.md); judges
  [chain fidelity](docs/plan/panel/judge-chain-fidelity.md), [maintainability](docs/plan/panel/judge-maintainability.md),
  [performance and DX](docs/plan/panel/judge-performance-dx.md).
- Measurements ([bench/](docs/plan/bench/)): [decode benchmark](docs/plan/bench/decode-bench/bench-run-3.txt),
  [tag-pinned core build](docs/plan/bench/tagpin/tagpin.log), [no-patch reproduction project](docs/plan/bench/nopatch/).
- Upstream: RaoFoundation/subtensor at
  <https://github.com/RaoFoundation/subtensor/tree/850f2755bb05a3851f948489f0ab27b0645cebce> (`sdk/bittensor-core`,
  `sdk/python/bittensor`, `sdk/python/tests`, `pallets/subtensor`, `runtime/src`, `docs/guides/signed-requests.mdx`,
  `docs/migration.mdx`, `docs/guides/local-development.mdx`); tag `v477` = `5c6e83e2`.
- Docs and catalogs: <https://www.bittensor.com/llms.txt>, <https://www.bittensor.com/llms-full.txt>,
  <https://www.bittensor.com/catalog/intents.json>, <https://www.bittensor.com/catalog/reads.json>,
  <https://www.bittensor.com/catalog/errors.json> (byte-identical to `CAT/` at `850f2755`).

### 18.2 Facts re-checked for this plan

Checked against `UP/` at `850f2755` (and at tag `v477` where noted) while writing this plan. The architecture's own
verification log is [Appendix F](docs/plan/architecture.md#appendix-f-verification-log-re-checked-on-disk-by-the-synthesizer).

| # | Claim | Evidence | Result |
|---|---|---|---|
| 1 | the core signs shielded carrier and inner extrinsic with era 64; the runtime rejects `submit_encrypted` with era > 8 or immortal as `Stale` | `CORE/client.rs:32, 622-654`; `UP/runtime/src/check_mortality.rs:19, 91-100`, same at `v477` | confirmed |
| 2 | core epoch predicate `> MAX_TEMPO_U64` vs pallet `> tempo` with a `tempo + 1` cap; Python reaches the core predicate | `CORE/timelock/epoch_schedule.rs:54`; `run_coinbase.rs:1232, 412-416`, same at `v477`; `PY/intents/weights.py:308` | confirmed |
| 3 | the core inherits `w3f-bls` from the workspace; the monorepo patches it to `vendor/w3f-bls` | `UP/sdk/bittensor-core/Cargo.toml:51-53, 62`; `UP/Cargo.toml:299, 334-335` | confirmed |
| 4 | Python checks 6 nested-result events, the core 3 | `PY/_transport/utils/receipt.py:8-16`; `CORE/client.rs:1025-1029` | confirmed; plus the `Sudid` field defect, verified by code reading (row 19) |
| 5 | `commit_reveal_version` 4 in Python; the chain requires equality and charges no fee | `PY/intents/weights.py:33, 411, 472-477`; `subnets/weights.rs:318-320, 337`; `macros/dispatches.rs:1946-1948` | confirmed |
| 6 | Python quantizes with `round()` (half-to-even) and clips with `epsilon = 1e-7` | `PY/intents/weights.py:54-114` | confirmed |
| 7 | policy constants and the non-idempotent method set | `PY/_transport/rpc.py:52-66` | confirmed (the set is at `:66`, which the architecture now cites) |
| 8 | V14 parse yields no APIs; V16 rejected; Python's V15 needle | `CORE/runtime/mod.rs:171-198`; `PY/_transport/runtime.py:33-34` | confirmed |
| 9 | `host` gates `client`, `keyfiles`, `pow`, `transaction`; `default = ["host"]`; hidapi is `ledger`-only | `CORE/lib.rs:10-31`; `UP/sdk/bittensor-core/Cargo.toml:82-126` | confirmed; the architecture now records the hidapi nuance (§1.7) |
| 10 | extrinsic assembly is v4 only | `CORE/codec/extrinsic.rs:268-273` | confirmed |
| 11 | fee: core `payment_queryInfo` with era 64; Python immortal zero-signature `TransactionPaymentApi_query_info`; Python anchors eras at the finalized head | `CORE/client.rs:551-576`; `PY/_transport/interface.py:485-499, 714-731` | confirmed |
| 12 | catalog and e2e counts (83/75/8/74/7/2, 139/116 wraps, 102 reads 81/11/10, 14 categories, 18 codes, 390 errors, 112 = 73 + 39) | `CAT/*.json`; `UP/sdk/bittensor-core/tests/e2e-manifest.json` | confirmed |
| 13 | release-pair churn table | `git diff --name-only` between tags v466…v477 | confirmed |
| 14 | Python package 59,723 lines; CLI 23,083; commands 7,872; intents 8,335; executor 1,319 | `wc -l` over `PY/` | confirmed |
| 15 | `v477` = `5c6e83e2`; `sdk/` unchanged to `850f2755`; toolchain 1.89 | `git rev-parse`, `git diff`; `UP/rust-toolchain.toml` | confirmed |
| 16 | 22 core trusted constructors, 4 without a same-named catalog op | `CORE/transaction.rs:251-826`; `CAT/intents.json` | confirmed (new finding); 3 of the 4 pair through a mapping, and `set_root_claim_type` is a retired call absent from spec-477 metadata ([§5.8](#58-known-core-defects-and-how-rtao-handles-them) #9) |
| 17 | the ten `disallowed-methods` paths exist | `CORE/client.rs:150`; `CORE/transaction.rs:1004, 1011`; `CORE/timelock/mod.rs:160, 242, 276, 298`; `CORE/timelock/epoch_schedule.rs:45, 108`; `CORE/keyfiles/mod.rs:126` | confirmed |
| 18 | `arbitrary_precision` breaks untagged/flatten float parsing and makes `Value` equality spelling-sensitive | `UP/sdk/bittensor-core/Cargo.toml:40-42`; serde_json 1.0.154 with and without the feature | measured (new finding); decided in [§5.8](#58-known-core-defects-and-how-rtao-handles-them) #10 |
| 19 | the core's receipt check looks up `result` or falls back to the whole attribute map; named event fields decode to a map keyed by field name; `variant` is a top-level field lookup | `CORE/client.rs:1025-1037, 1515-1527`; `CORE/codec/decode.rs:428-435` | confirmed by code reading, not executed: a failed `Sudid` is reported as success |
| 20 | root-origin ops list `Sudo.sudo` among their catalog `wraps`, so the M5 plan test can check the first two call bytes | `CAT/intents.json` (`set_root_claim_threshold`, `set_subnet_emission_enabled`) | confirmed |
| 21 | steps 5 and 6 of Python's password chain are the macOS Keychain and the macOS dialog, both through `macos_password.py` | `PY/wallets.py:62-118` | confirmed |
| 22 | the Vault signer runs a loopback HTTP and WebSocket server and renders UOS frames with `qrcode` in byte mode (`optimize=0`), error correction M, border 4; `bittensor.vault` exports `VaultSessionServer` and `scan_vault_address` | `PY/vault/server.py:26, 69-80, 181-242`; `PY/vault/qr.py:15-28`; `PY/vault/__init__.py` | confirmed |
| 23 | `cargo doc --workspace` with a library `rtao` and a binary `rtao` in another package warns "output filename collision" and writes both to `target/doc/rtao/`; `doc = false` on the binary removes it | a two-crate workspace, cargo 1.99.0 | reproduced |
| 24 | `frame_system`'s `can_set_code` refuses a `spec_version` that does not increase | polkadot-sdk fork checkout `cacb431`, `substrate/frame/system/src/lib.rs:2320-2349` | confirmed |
| 25 | 15 of the 39 upstream `test_*` e2e names were in no plan document; architecture §9.1's `NeuronCx` holds `metagraph: MetagraphHandle` and `http: SignedClient` | `UP/sdk/bittensor-core/tests/e2e-manifest.json`; `e2e.rs:288, 440, 476, 542, 1159-1330` | confirmed |
| 26 | the Python package has no `__main__` module, so `python -m bittensor` does not run | `find UP/sdk/python -name __main__.py` finds only `codegen/__main__.py` | confirmed |
| 27 | 19 Python modules declare `__all__`; besides the root, 14 are public and outside `_transport/` and `cli/` (`evm` 38 names, `intents` 97, `reads` 21, `extension` 16, `vault` 5, …) | `grep -l __all__` over `PY/`; `ast.literal_eval` of each list | confirmed |
| 28 | the core's Ledger transport cannot be swapped: `mod hid` is private, `LedgerDevice` holds the concrete HID `Transport` and `open()` is its only constructor; APDU building is private | `CORE/signers/mod.rs:6-9`; `CORE/signers/ledger.rs:51-65, 158-219`; `CORE/signers/hid.rs:41-72` | confirmed (G16) |
| 29 | Python's `LedgerSigner` takes `spec_name` from the runtime version and decimals and symbol from its arguments (9, `"TAO"`), and sends the unhashed payload parts | `PY/_transport/contract.py:60`; `PY/ledger.py:37-41, 115-163` | confirmed; architecture §3.9.2 now does the same |
| 30 | a runtime built without the compiled metadata hash rejects a `CheckMetadataHash`-Enabled extrinsic with `UnknownTransaction::CannotLookup` | polkadot-sdk fork checkout `cacb431`, `substrate/frame/metadata-hash-extension/src/lib.rs:150-155` | confirmed by code reading; matches subtensor#3159 |

## 19. Progress log

Append-only, newest last ([Executing this plan](#executing-this-plan), rule 4). One line per event: a milestone exit
with its release tag, a deviation from a step's text (what and why), a decision taken under rule 5, an upstream filing
with its link, a wait on a prerequisite or on the user.

| Date | Step or milestone | Event |
|---|---|---|
| 2026-10-14 | M0-01 | Filed PR-U17 [subtensor#3242](https://github.com/RaoFoundation/subtensor/pull/3242) from `sam0x17/subtensor`, branch `ledger-apdu-transport` at `eafe84e8a97376e7902118ebb204b9c53e3df55e`. Current Rust 1.89 checks passed (74 library tests, portable/wasm, binding and clippy); retained patch and results in `docs/upstream/pr-u17.md`. Upstream workflows ended in startup failures with no Rust check jobs; external deployment awaits authorization. User instructions for the fork identity and CI-only Python superseded conflicting upstream preflight instructions; compatible Rust checks ran against the pushed commit. Exact PR rehearsal directory removed after dependency checks. |
| 2026-10-14 | M0-01 | Requested §12.13's external prerequisites; continuing independent steps while the response and PR-U17 validation are pending (execution rules 3 and 6). Task-scoped temporary root recorded in `docs/execution/state.md`. |
| 2026-10-14 | M0-01 / Q23 | User offered testnet TAO to a prepared address, authorized `sam0x17` GitHub/GHCR, selected only free `ubuntu-latest` hosted jobs, declined Pages and confirmed a Ledger is available. Updated CI/release/docs commitments and §12.13; retained full parity, soak, macOS Keychain and release-provenance requirements. Testnet setup/host, local platform verification and mainnet-clone resource feasibility remain pending; no new step is checked by this decision. |
| 2026-10-13 | — | Plan v2: the user's decisions recorded (§14.1), time estimates removed, step IDs and numbered exit checks added |
| 2026-10-13 | — | Q17 resolved by research ([research J](docs/plan/research/J-ledger-ci-shim.md)): the Speculos emulator lane and transcript replay (M7-09, M7-10), PR-U17 to file in M0, the device run limited to what the emulator cannot cover (M7-11) |
| 2026-10-13 | — | Plan v2 cold read: M1-03 fetches the finney metadata that regen reads; M2-08's `Policy` equivalence pairs land with their intents (M2-12, M4-06); M0-01 files PR-U17; the issue outside the monorepo needs the user's approval (C4, M9-04); merging release-plz's release PR is a stop-and-ask point; required checks join branch protection as their jobs appear (§12.13) |
| 2026-10-13 | M0-04, M0-08 | nextest 0.9.143 rejects a `ci` filter whose `binary()` names no declared target (exit 96), so M0-08 declares the four `rtao-testkit` test targets as empty files and the filter lands with them |
| 2026-10-13 | — | Q20 decided by the user: until PR-U17 ships, rtao's workspace builds `bittensor-core` through a Cargo patch that adds it (§5.10; M0-01, M7-09, M7 exit 14); the test-only transcription remains only as the fallback if upstream declines PR-U17 |
| 2026-10-13 | — | Q21 decided by the user: M0 to M9 run as one goal loop without PRs; the agent commits to `main` after `cargo xtask gates` (M0-27), stops the line on red CI and tags milestone exits itself (rules 1-8, C1, C2, C5, C7); the upkeep jobs stay off until the handover (M9-07, M9 exit 5) |
| 2026-10-13 | — | The PR-U17 core patch prototyped in Docker ([research J §8](docs/plan/research/J-ledger-ci-shim.md#8-core-patch-prototype)): the core's own `ledger.rs` drove the app in Speculos through `LedgerDevice::with_transport`; the root manifest gains a second table that keeps the core's other path dependencies (`w3f-bls`, `subtensor-macros`) at `core_tag`; the obsolescence probe names the trait and the constructor, runs in its own target directory and must also compile against the patch; E0405 joins the unresolved-name codes (§5.3, §5.10, M0-01, M7-09) |
| 2026-10-13 | — | Q22 decided by the user: the Ledger app fix was filed with `Zondax/ledger-polkadot` before the plan runs (issue [Zondax/ledger-polkadot#295](https://github.com/Zondax/ledger-polkadot/issues/295), PR [#296](https://github.com/Zondax/ledger-polkadot/pull/296)); rule 6 no longer names it, rule 7 pre-authorizes its follow-up, and C4 counts it as filed (§15, M7-12, M9-04, M9 exit 3) |
| 2026-10-13 | — | Plan v2.1 cold read: a milestone's tag waits for its other exit checks, and only the device run (from M7) and M9-07's last settings follow it (rule 3; M9-07 and M9 exit 5 put the device-run record of `rtao-v1.0.0` in the agent's last push); patch tags are pre-authorized (rule 7); `cargo xtask gates` changes nothing on the host, so the Keychain test runs only in CI (M0-27, M4-14); the obsolescence probe built against the fork carries the second core-patch table, and the first `core-patch rebase` takes PR-U17's commits from its review branch (§5.10). The Q21 and Q22 rows supersede the v2 cold read's items on release-plz's release PR, branch protection and the Zondax approval |
| 2026-10-13 | — | After the v2.1 cold read: the Zondax issue [Zondax/ledger-polkadot#295](https://github.com/Zondax/ledger-polkadot/issues/295) and PR [#296](https://github.com/Zondax/ledger-polkadot/pull/296) replace the placeholders, and rule 7's follow-up includes pushing the changes maintainers ask for to the PR branch; `850f2755` was released as `v478` and finney and testnet moved to spec 478 the same day, so M0-06 pins the newest release finney runs (`v478` today) and the `v477` of the examples stands for it (§10.3 step 1 would refuse `v477` now); every monorepo git dependency is declared in the root `[workspace.dependencies]`, so a core bump rewrites all of them (M0-07, M0-20, §5.6, §10.3; architecture §1.2); `e2e-required` also fails when its `changes` job fails; M0-06's digests come from `xtask pin --parity <tag> --check`; M7-12's note covers the `limit_price` unit; M9-07 removes the drift auto-merge if the user declines branch protection |
