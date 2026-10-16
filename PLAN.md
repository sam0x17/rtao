# rtao: plan to build Bittensor subnets in Rust

**Status.** Plan v2.3, 2026-10-14. Q30 limits execution to client development and hosted CI (see Current execution scope below). Original planning context: the user's answers to the open questions are recorded as decisions
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

## Current execution scope (Q30, 2026-10-14)

The user has limited this work to client development and hosted CI after repeated isolated builds consumed hundreds
of gigabytes on a shared laptop. This section supersedes earlier execution rules, milestone steps, exit checks and
architecture requirements wherever they require expensive local work. It also supersedes the local-build and
release portions of Q17, Q21, Q23 and Q29. Historical results remain recorded; removed checks are never reported as
passing.

- Develop and review source locally. Use free `ubuntu-latest` GitHub jobs for workspace tests, Clippy, feature and
  MSRV checks, dependency audits and documentation builds. A full local `cargo xtask gates` run is no longer a
  prerequisite to commit or push. Record hosted results for the pushed commit before claiming acceptance.
- Do not run local full nodes, localnets, testnet validators/miners, Docker or Python oracle containers, soaks,
  hardware or Keychain rehearsals, fuzz campaigns, benchmarks or release builds. Do not use Mac Studio as a second
  build farm. Client APIs may support those environments without deploying them during development.
- Do not start concurrent Cargo jobs, duplicate workspace/build snapshots, target trees or extra toolchains on the
  laptop. Local compilation is exceptional and limited to one narrowly selected command when necessary to diagnose
  a source issue; prefer hosted CI. Keep temporary work in one recorded task root and remove completed disposable
  work after checking live dependencies. Preserve authored source, wallets, recovery material and unrelated projects.
  Do not clear shared global caches to meet this project's budget.
- CI uses bounded jobs, explicit timeouts and cancellation of superseded runs. Ordinary acceptance relies on Rust
  units, property tests, goldens, committed oracle vectors, `FakeNode`, `FakeChain` and relevant integration tests.
  Container-based oracle, interoperability or localnet checks are optional targeted hosted jobs for a concrete
  client change. Their absence is recorded as a coverage limit. They must fit free runners; a resource limit does
  not create a requirement to reproduce them locally or find another host.
- **Q30 optional oracle policy.** A missing first hosted oracle batch remains explicitly unverified during strict
  client-source parity checks. Existing outputs must pass verification, and complete Git history must prove no
  prior outputs were removed. Source parity completion never certifies independent Python oracle execution.
- Remove mainnet cloning and full-node operation from this work. Defer release automation, per-milestone tags,
  binary distribution, cross-platform build rehearsals, device runs, automated upkeep and deployment setup. macOS,
  ARM and hardware behavior stays in the design; report gaps in executed platform coverage honestly.

The technical behavior and parity inventories remain implementation references. Development milestones complete
with source, docs, review and applicable hosted checks under C1-C7 below. Optional or deferred infrastructure cannot
block client development or trigger a new local validation campaign.

The active workflow is `.github/workflows/pr.yml`. The eleven former infrastructure, scheduled, oracle and release
workflows are retained outside GitHub's active directory in `.github/deferred-workflows/`. Their triggers cannot
run; any future targeted check needs a reviewed, bounded hosted job.

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
   names what it does, such as `pin: v477 to v478`) and carry no AI attribution trailers (the user's standing
   preference). Before a push, review the diff and run only cheap relevant source checks. Pushes start the hosted
   `pr-gates` jobs; do not duplicate that matrix locally (Q30). Check the latest runs before the next step:
   `gh run list --branch main`. Fix red applicable jobs first. A development milestone completes under the active
   client checks and C1-C7 of [§12.1](#121-overview). Q30 already removes or defers the costly checks it names;
   do not ask again to waive them. Record any different unmet active requirement before asking the user.
3. **Waiting.** While hosted checks run, continue source review or independent client development. Do not start a
   local build to duplicate a pending CI run. Milestone tags, device runs and release settings are deferred by Q30.

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
7. **Pre-authorized** (no need to ask): committing and pushing client development to `main` under rule 2;
   dispatching bounded free hosted CI lanes (`gh workflow run`); creating the fork of the
   monorepo; auditing the existing upstream PRs/issues of [§15](#15-upstream-contributions) and retaining additional
   proposals locally in the current task; maintaining PR-U17's review branch and the
   `rtao-core-patch/*` branches in the fork, never force-pushed ([§5.10](#510-the-pr-u17-core-patch)); following up the
   Ledger app issue and PR filed with `Zondax/ledger-polkadot` on 2026-10-13 (Q22): answering maintainer questions and
   pushing the changes they ask for to the PR's branch (`fix/tao-balance` in the user's fork
   `sam0x17/ledger-polkadot`), never force-pushed; running the local cold-reader security passes
   ([§12.1](#121-overview)).
8. **Prerequisites** (GitHub access for development; optional GHCR/device access for deferred checks) and Q23's
   answers are listed in [§12.13](#1213-dependency-order-and-external-prerequisites); M0-01 asks for
   them, and each step that needs one names it. Q23 authorizes `sam0x17` and only free `ubuntu-latest` hosted jobs;
   docs can be downloaded as CI artifacts, with no Pages environment. Q29 and Q30 remove testnet setup and host
   prerequisites. Bot installation, branch protection, merge queue and release publishing are deferred by Q30.

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

Delivery is operator-first; each development milestone ends with reviewed client source and applicable hosted checks ([§12](#12-milestones)). The plan states order,
dependencies and objective exit checks, not durations ([§14.1](#141-decisions-answered-2026-10-13)):

| Capability | Milestone | Depends on |
|---|---|---|
| weight-setting validator, no Perl/OpenSSL/libsodium | M2 | M0, M1 |
| btauth/1 miner, subnet template | M3 | M2 |
| owner tooling, MEV shield, CLI v0 | M4 | M2 |
| full v11 client parity (M8), client hardening (M9) | M8, M9 | M5, M6, M7 |

Track upstream changes relevant to the implemented client from M2 onward. Source/pin updates use applicable hosted
checks; costly refreshes are optional targeted hosted work. Bot handover and scheduled upkeep are deferred (Q30).

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
`github.com/sam0x17/rtao`; Q30 defers manual device runs and makes the Speculos emulator an optional hosted check; until PR-U17 ships, rtao's own workspace builds `bittensor-core` through
a Cargo patch that adds it ([§5.10](#510-the-pr-u17-core-patch)); current work audits existing upstream filings and
keeps additional proposals local. The Ledger app fix was filed with Zondax on 2026-10-13, before the plan runs; the per-milestone
security passes are done by local cold readers; the agent develops M0 to M9 as one goal loop, committing to `main`
without PRs ([Executing this plan](#executing-this-plan)); and the plan carries no time estimates, because an AI coding
agent executes it step by step.

## 2. Goals and how the design delivers them

The user's design goals, mapped to the mechanisms that deliver them. "Fast" is read as latency on hot paths and "highly
performant" as throughput, resource use and build cost at subnet scale [I: the plan's reading of two overlapping goals].

| Goal | Mechanisms | Depth |
|---|---|---|
| **100% Rust, no Python** | every crate, binary and build step is Rust; chain bytes come from the Rust core; product behavior is ported to Rust; extraction and regeneration use `rustpython-parser` and `syn` inside `xtask`; the `rtao` CLI replaces `btcli` and shares its files. Python runs only in CI lanes (oracle, interop, upstream recorders, and the Ledger emulator, whose Speculos runs Python inside its image), never in a build, in regeneration or in `pr-gates`; these are optional targeted hosted checks under Q30 ([§9.7](#97-ci-lanes-and-what-each-catches)); the user accepted this reading (Q2, [§14.1](#141-decisions-answered-2026-10-13)) | [architecture §0.3 #23](docs/plan/architecture.md#03-twenty-five-decisions-that-define-the-design), [§11.5](docs/plan/architecture.md#115-the-python-oracle-ci-only-outputs-committed), [§17 #13](docs/plan/architecture.md#17-what-we-deliberately-do-not-do-yet) |
| **Fast** (hot-path latency) | metagraph decoded by a borrowed, shape-gated `MetagraphView`: 6.5 µs at 256 UIDs against 330 µs for the core `Value` decoder and 1,380 µs for `DecodeAsType`, and 57 µs at 2,500 UIDs; signing inline (about 0.03 ms); `Runtime::parse` about 1 ms once per spec, off the reactor; a weights preflight is one `state_queryStorageAt` instead of about 18 RPCs; O(1) `by_hotkey` | [architecture §8.2](docs/plan/architecture.md#82-the-metagraph-three-decode-paths-one-projection), [§8.6](docs/plan/architecture.md#86-performance-claims-corrected), [§3.6](docs/plan/architecture.md#36-reading-state), [bench-run-3.txt](docs/plan/bench/decode-bench/bench-run-3.txt) |
| **tokio / async** | own tokio-tungstenite session behind `subxt_rpcs::client::RpcClientT`, with TLS from one explicitly built `ring` rustls config (never the process default); return-position `impl Future + Send` on generic paths and boxed futures only on `dyn` seams; Argon2i unlock, Ledger HID, PoW, metadata parse, any decode above 64 KiB on every path, timelock encryption, the RFC-0078 digest (once per spec) and fsync'd file writes go through `spawn_blocking`; dropping the last `Client` clone cancels its tasks and closes the socket; async `NonceStore`; axum, tower and reqwest adapters; `CancellationToken` supervision; the core's blocking reqwest client is banned from runtime paths by `clippy` | [architecture §3.1](docs/plan/architecture.md#31-async-model), [§3.2](docs/plan/architecture.md#32-transport-rtao-rpcsession), [§1.8](docs/plan/architecture.md#18-deliberately-not-used-on-runtime-paths) |
| **Highly performant** (scale, resources, build) | coalesced reads and a client token bucket for public endpoints that allow about 1 request per second; an `ArcSwap` metagraph handle with no per-request decode; sparse weight and bond rows (a dense 2,500-UID matrix would be 6.25M cells); selective metagraph presets; no-host neuron builds (the core without `host` locks 241 packages and builds in 12.9 s; with `host` it pulls 330 crates, and its vendored OpenSSL needs Perl and make); a cold build first fetches about 293 MB (subtensor) and 480 MB (polkadot-sdk) of git history, which the template's BuildKit cache mounts pay once; targeted hosted performance comparisons are informational | [architecture §3.2.5](docs/plan/architecture.md#325-rate-limits-rtao-addition-and-request-volume), [§8.3](docs/plan/architecture.md#83-sync-service-c), [§8.4](docs/plan/architecture.md#84-miner-admission), [§2.8](docs/plan/architecture.md#28-build-weight), [§11.7](docs/plan/architecture.md#117-property-tests-fuzzing-benchmarks) |
| **Correct + tested** | upstream goldens replayed through rtao's wrappers; Python oracle vectors for composed calls, plans, reads and pure functions (u16 weight rows bit for bit, 10,000 `str(Balance)` vectors); a socket-level `FakeNode` that ports `test_rpc.py`; 19 rtao localnet scenarios, with the 112 upstream e2e manifest names and the 3 `PYT/e2e` tests mapped (115 keys); Python interop lanes; proptest and fuzzing; type-level safety (`KeySource<Role>`, `Balance<Tao>` vs `Balance<Alpha>`, `NonceLease`, `ShieldPeriod`, spec-stamped `CallBytes`, `ReadMethod`); fail-closed per-call surface verification | [§9](#9-testing-strategy), [architecture §11](docs/plan/architecture.md#11-testing) |
| **Well-documented** | `deny(missing_docs)`; every example is a compiled `docify` item; an mdBook that mirrors bittensor.com's taxonomy; generated reference pages per op, read, error and hyperparameter; `#[doc(alias = "bittensor.…")]` on every mirrored item; a v11 → rtao migration table generated from `parity/map.toml`; `llms.txt` | [§11](#11-documentation-plan), [architecture §13](docs/plan/architecture.md#13-documentation) |
| **Easy to maintain** | rtao owns only I/O, generators, ported behavior and staged upstream code (about 14k generated, 13k I/O, 27k ported, 7k framework, 8k tooling and 2k staged lines [I]); one seam crate absorbs core API changes; ported modules mirror Python file for file; provenance by git blob SHA in `parity/map.toml`; layering enforced by `xtask deps-check`; `clippy` `disallowed-methods` | [architecture §0.4](docs/plan/architecture.md#04-what-rtao-owns-budget), [§2.3](docs/plan/architecture.md#23-dependency-layers-and-enforced-rules) |
| **Modular** | 19 crates in nine layers with enforced dependency rules; facade features and role profiles; one feature per Python intent group; the btauth/1 core has no I/O; signers, EVM, PoW GPU and the subxt escape hatch sit behind features | [§4.2](#42-layers), [§4.4](#44-facade-features-and-role-profiles) |
| **Easy to keep up to date** with Python and subtensor | two pins; source and metadata diffs; generated-artifact and core API probes; reviewed pin updates with hosted client checks; optional targeted hosted oracle refresh; compatibility records and per-call runtime guards; scheduled automation deferred (Q30) | [§10](#10-keeping-up-with-upstream), [architecture §12](docs/plan/architecture.md#12-upstream-tracking) |

## 3. Fixed decisions and how the design reads them

The user's decisions are final. The design interprets them as follows.

| User decision (quoted) | How the design reads it | Consequences | Depth |
|---|---|---|---|
| "don't worry about crates.io, we just need to build something that works" | Publishability is not a goal. Current work delivers client source through git; release tagging, binary distribution and publishing are deferred (Q30) | git and `[patch]` dependencies are acceptable; documentation is downloadable from hosted CI without Pages | [architecture §12.8](docs/plan/architecture.md#128-release-process) |
| "git dependencies are fine" | `bittensor-core` is pinned by git tag; every final workspace repeats `[patch.crates-io] w3f-bls` at the same tag; `xtask`-only git dependencies such as `subtensor-linting` are allowed | subnet repositories carry one patch line until PR-U4 lands; the template and `rtao doctor` keep it right | [architecture §1.2](docs/plan/architecture.md#12-pinning-core_tag-and-parity_tag), [§1.3](docs/plan/architecture.md#13-the-w3f-bls-patch-end-to-end) |
| "probably we can build what we need as a veneer over bittensor-core if that now exists" | It exists: `sdk/bittensor-core`, version 0.1.0, `publish = false`, edition 2021, Apache-2.0 (`UP/sdk/bittensor-core/Cargo.toml:1-7`, re-checked). "Veneer" means every chain-defined and compatibility byte comes from the core, while product behavior comes from Python, because the core's product layer (blocking `Client`, `Executor`, `Policy`) diverges from Python on era, anchor, shield era, fee RPC, receipts, error codes and intent coverage | one seam crate; a staging crate for upstream-bound code; the core client used as a test oracle only | [architecture §1](docs/plan/architecture.md#1-boundary-with-bittensor-core), [§5](#5-bittensor-core-taken-wrapped-staged-avoided) |
| scope = "matching everything that is today in python" | Full parity with the v11 package at `parity_tag`: 83 intents, 102 reads, 18 error codes and 390 classified chain errors, transport semantics, wallets, keyfiles and passwords, signers (Ledger, Vault, browser extension), EVM helpers, timelock, btauth/1, the metagraph, and the CLI (163 hand-written commands plus generated `tx`/`query`). JSON output is value-equal to Python's. Phased but complete; the parity gate turns strict at M8 | parity machinery starts in M0 and is complete in M2; full client parity at M8, client hardening at M9; distribution deferred by Q30 ([§12](#12-milestones)) | [§8](#8-python-parity), [architecture §4](docs/plan/architecture.md#4-intents-and-reads-parity) |

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
6. Exactly one built `w3f-bls` uses `core_tag`; `Cargo.lock` permits only its pinned entry and the recorded dormant registry 0.1.9 entry (Q26).
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
| `xtask` | pin, import-upstream, extract, regen, parity, staged, core-surface, deps-check, drift, oracle, record, docs, compat, localnet (bootstrap, run, smoke, up/down), localnet-image, probe, release-notes, pr-u4-proof, fuzz, bench (optional hosted), template (check), upstream (parity-tag), ci (issue), lint-upstream, ledger-emu (build-app, up, record, speculos-image), gates (hosted `pr-gates`, M0-27; no full local run, Q30), core-patch (rebase, strip; M7-09) (the subcommands Appendix E and the executing rules call) | — | — | M0 |

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

1. `crates/rtao-core/tests/lock.rs` requires one built `w3f-bls` in the workspace's all-feature/all-target normal/build graph, from the same git
   source and tag as `bittensor-core`'s declaration, equal to `rtao_core::BITTENSOR_CORE_TAG` (while the core patch
   exists, the lock takes `bittensor-core` itself from `[core.patch]`, which `xtask pin --check` verifies, and the
   patch's second table keeps `w3f-bls` at that tag, [§5.10](#510-the-pr-u17-core-patch)). The lock inventory permits one pinned 0.1.3 entry and at most the exact recorded registry 0.1.9 entry, checksum included. Activation, unexpected identities and missing pinned provenance fail (Q26).
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
compile probe names every core item `rtao-core` and `rtao-core-ext` use, with its exact signature; an optional hosted
`core-canary` compiles client source against a reviewed monorepo `main` commit, without starting a node; and the golden vectors are
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
  bounds and calls, but not a trait `impl` header written by full path. The `core-unpatched` job (on relevant source/pin changes that affect `upstream.toml` or the root `Cargo.toml`) catches that case too: it deletes both tables in its
  disposable checkout and runs `cargo check --workspace --exclude rtao-testkit --exclude xtask --all-features` without
  `--locked` (Cargo then re-locks only `bittensor-core`), which proves what subnets build; because `--all-features`
  still builds an excluded crate that a feature reaches, it also fails if a shipped feature pulls in `rtao-testkit`
  ([§4.2](#42-layers) rule 8). The hosted template compile/test job (M3-09) checks downstream source; real-chain template evidence is optional.
- **Coverage.** From its first run the emulator lane drives the core's own `ledger.rs` (APDU building, 250-byte
  chunking, status-word mapping) against the real app, through `LedgerDevice::with_transport` over `rtao-testkit`'s
  Speculos TCP transport, and `ledger_replay` in `pr-gates` uses a replay transport the same way
  ([§9.5](#95-python-oracle-and-interop)); the prototype did both. Deferred hardware coverage includes: the HID transport and OS device access (`hid.rs`, `LedgerDevice::open`), the app binary users install, real
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
    let cfg = rtao::neuron::NeuronConfig::load_for(NetUid(1)).await?;       // env > ~/.bittensor/btcli.json > default
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
    NeuronRuntime::miner(NeuronConfig::load_for(NetUid(1)).await?, MyMiner).run().await // [I] named by analogy with ::validator
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

`templates/subnet/` contains validator/miner members and an owner tool, with the `w3f-bls` patch once in the workspace
root. Shared config/state formats, hotkey-only neuron profiles and graceful shutdown behavior follow the API design.
Its ordinary CI compiles relevant profiles and runs client tests against the changed source, with fixtures and fakes.

Dockerfile, compose and service-unit recipes may remain as operator examples. Building images, running a localnet,
registering a subnet, staking accounts or starting neuron processes is optional targeted hosted integration work;
none is required to develop the template or pass its ordinary CI. Do not run those recipes locally. No deployment
or service installation is part of this task. The owner member joins in M4, and Ledger feature source in M7.

Invocation: `cargo generate --git https://github.com/sam0x17/rtao templates/subnet`.


## 8. Python parity

### 8.1 Definition of done

Client implementation completeness at M8 means all 83 intents, 102 reads and the CLI surface have implementations
or documented deliberate divergences, with no unexplained source/catalog rows or `NotYetPorted` stubs. Hosted tests
cover plans, encoding, reads, malformed input and implemented behavior through fixtures and fakes. Existing pinned
oracle vectors replay, JSON integer comparisons remain exact and every divergence has a regression test.

Source completeness and executed cross-language/chain coverage are recorded separately. Fresh oracle comparisons,
read/CLI interoperability and upstream e2e mappings remain useful evidence, but their container-based execution is
optional targeted hosted work under Q30. The mapping records available tests and their actual status; do not claim
a green e2e test, hardware compatibility or full externally verified parity from source mapping alone. Do not create
local nodes, containers or a full campaign to clear a development milestone.

The output contract remains JSON value equality, including valid-chain Balance formatting and registered native-u64
differences. Human tables and JSON text layout do not require byte identity. Document any unexecuted oracle,
interop, platform or device coverage when reporting implementation progress.


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
| symbol layer | `rustpython-parser` diff of old and new blobs (names, classes, dataclass fields and defaults, `ClassVar`s, functions and parameters), joined to Rust items through the `#[doc(alias = "bittensor.…")]` index | "Python `X` changed → Rust `crates/…:line`", or "no Rust counterpart" |
| catalog layer | generated registries vs vendored `CAT/*.json`: names, classes, signer, origin, verify, wraps, normalized schemas; read params (with the extracted `read_params.json` defaults), docs, categories, hits (the catalog hits must be a subset of each read's hit set, [§9.3](#93-fakes-at-the-right-layers)); error codes and names; extracted defaults vs generated `defaults.rs` | catalog drift, schema mismatch |
| extraction layer | `xtask extract --check` re-extracts every literal table from the vendored sources (settings, error maps, retry substrings, shield bounds, RPC policy constants, the hyperparameter table, `PROXY_TYPES`, samples). Beyond literals it accepts seven resolved forms: constant folding, module-level names, environment defaults, descriptor chains (`st.<Pallet>.<Item>`), `default_factory` lambdas, builtin type tags and `Path.home()` paths; an M1 test checks that every named table extracts | a hand-copied constant drifting; a table restructured into non-literal code (fails with file and line) |
| chain-source layer | `syn` over runtime sources: custom transaction-error codes, `call_filter_group!` fee filters (Python mirror must equal it), `SelectiveMetagraphIndex` arms, `freeze_struct` hashes, `ProxyType` vs `PROXY_TYPES`, `MAX_SHIELD_ERA_PERIOD` (into `rtao_catalog::rt`), the `prod_or_fast!` rate limits absent from metadata (into `rt::rate_limits`), and the chain rules the framework mirrors (tempo bounds, freeze window, rate-limit arms) into `chain_rules.json` | runtime changes that Python or the framework mirrors by hand, including code-only changes that produce no metadata drift |
| coverage gate | every call of Python's 13 covered pallets is wrapped by some intent, listed in Python's `RAW_ONLY`, or in `raw_only_extra.toml` | a new chain call nobody decided about |
| oracle layer | committed `vendor/oracle/*.json` compared in ordinary `cargo test` | any behavioral difference in plans, composed calls, reads, pure functions |
| completeness | `pending.toml` may only shrink (except when a pin bump, the change that moves `[parity]`, adds new upstream ops); every divergence needs a test and an upstream link; every `__all__` name of `PY/__init__.py` has a module-map row or a `#[doc(alias)]`, and every name of the 14 other public `__all__` lists (`intents`, `reads`, `evm`, `extension`, `vault`, `http_auth.py`, `keyfiles.py`, `ledger.py`, `timelock.py`, `wallet.py`, …) has a `#[doc(alias)]` or a reasoned row in `parity/exports_extra.toml`; strict from M8 | silent scope loss, including library API that the oracle never calls |
| environment layer | `parity/env.toml` maps every environment variable the Python package reads (28 names plus `BT_PW_*` at v477, extracted with file and line) to its rtao consumer or a divergence | a new, renamed or vanished environment variable |
| bless and auto-bless | `xtask parity bless <py-path>` records the new blob SHA after a human port; a pin bump auto-blesses only when the symbol diff, the extracted tables and the mapped oracle vectors are all unchanged; catalog blobs are compared normalized (without `line`, `end_line` and `url`), so a source-line shift auto-blesses; a `vendored` blob is auto-blessed only when its consumers' tests pass | keeps 2-4 releases a week manageable |
| early warning | optional source diff of monorepo branches against the pin, when relevant to client work | observed upstream changes; scheduled campaigns deferred |

For `oracle-only` file rows, source reconciliation authenticates the reviewed
upstream file and its optional consumer. It does not require a first recorded
oracle corpus. Q30 keeps oracle execution and differential coverage explicitly
unverified until those optional hosted checks actually run; native product
behavior remains tracked by its own source/export/divergence rows.

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
| D-007 | `Snapshot.constant/block_time/is_fast_blocks` | constants and derived timing use the retained snapshot runtime | improved |
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
| D-035 | `Balance` local arithmetic | all balances use native u64 rao with checked arithmetic; negative and arbitrary-size Python balances are unsupported, while valid-chain formatting remains compatible (Q25) | stricter |
| D-036 | secret constructor and keyfile errors | fixed diagnostics and error categories; supplied secrets excluded from Display, Debug and source chains, with owned dependency diagnostics wiped | stricter |
| D-037 | password and keyfile reads | regular opened descriptors and a 16 MiB content cap; regular symlink targets, password precedence and whole-file trimming remain supported (M2-S05) | stricter |
| D-038 | btauth retention equality | `Verifier::new` requires retention strictly greater than max_age + allowed_skew, closing the inclusive freshness/expiry endpoint | stricter |
| D-043 | generated query plural account parameters | resolves each explicit `_ss58s` account reference before dispatch, preserving order/duplicates and refusing unknown entries; prepared source, hosted validation pending | addition |
| D-044 | CLI PoW preparation and stale dry runs | refreshes public work before strict planning and after unlock for actual requests; dry runs never mine and reject stale challenges; hosted validation pending | stricter |
| D-061 | epoch-consensus setter example | suggests accepted Yuma/Null spellings instead of the numeric values rejected by Python's own setter; raw values stay exact; hosted validation pending | improved |
| D-062 | owner-intent preflight | existing native planner checks effective owner, admin freeze and cooldown before key acquisition; matching Null continuation skips only cooldown; Python has no corresponding blocks hook; hosted validation pending | stricter |
| D-063 | btauth replay identity | preserves exact signed sender text, then keys replay storage by decoded account and full signed nonce across SS58 prefixes; Python keys by literal sender text; hosted validation pending | stricter |

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
| localnet e2e | real chain behavior | optional targeted hosted job (Q30) | hosted Docker only |
| `core-oracle` | rtao and `bittensor_core::Client` agree where they should | optional targeted hosted job | hosted containers only |
| Python interop | cross-language wire bytes, wallets, btauth/1, JSON | optional targeted hosted job | hosted containers only |
| Ledger emulator | the Ledger path against the real Polkadot generic app under Speculos: address, clear signing with the core's proof, rejection and tampering, Ledger-signed extrinsics accepted by a localnet | optional targeted hosted job | hosted containers only |
| mainnet clone | removed from current work (Q30) | none | no clone or full node |
| fuzz, bench | robustness; performance regressions | optional bounded hosted jobs | free CI only |
| live probes | public endpoints and finney's shape | optional targeted hosted job | read-only network access from CI |

Localnet, interop, core-oracle and Ledger emulator tests live in the `e2e`, `interop`, `core_oracle` and `ledger_emu`
`[[test]]` targets of `rtao-testkit`, each gated by `required-features`; the nextest `ci` profile filters them out, so
the hosted `--all-features` `pr-gates` run never starts them. Native macOS verification is deferred by Q30.

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
  In hosted Rust tests it runs the port of `PYT/unit/test_rpc.py` and the 19-row Python transport checklist
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

Q30 makes this a reference for optional targeted hosted checks. None of the setup or scenario campaigns below runs
on the laptop or Mac Studio, and they are no longer common milestone exit requirements.

([architecture §11.4](docs/plan/architecture.md#114-localnet-e2e-docker))

- Harness (`rtao-testkit::Localnet`, M2): the plain `docker` CLI, as upstream does; readiness by healthcheck or
  `Imported #1`, then `Drand.LastStoredRound > 0` within 30 s (commit-reveal and timelock depend on pallet-drand
  fetching quicknet pulses over public HTTPS; on timeout `LocalnetError::DrandUnreachable`, and the job retries once
  with a fresh container); the e2e job first checks egress to the drand endpoint; upstream's env contract
  (`E2E_ENDPOINT`, `LOCALNET_IMAGE`, `SKIP_PULL`); images pinned by digest from `upstream.toml`.
- Select one pinned image and the smallest relevant scenario set for an optional hosted dispatch. Use explicit
  timeouts; run a slow timing case only when that client change needs it. There are no automatic push, nightly,
  weekly or multi-network campaigns, and `e2e-required` is not a development gate. Do not open automatic issues or
  seek another host when a free runner cannot complete a check.
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
  to scenarios 4, 14 and 19. Each key holds `rtao = "<target>::<test fn>"`, `na = "<reason>"`, or `pending = "M<k>"`.
  Under Q30, reviewed exact ordinary plan/read fixtures and complete client contracts can supply source mappings;
  optional hosted `e2e`/`interop` targets remain scenario source. New client-contract mappings require explicit
  `scope = "client-contract"`, `runtime = "unverified"` and exact reviewed `runtime_limits`. The gate binds each
  ordinary counterpart to its specific upstream key, all-feature test body and meaningful operations. Matrix
  evidence classes distinguish plan, read, client contract and hosted scenario source; none asserts runtime
  mutation, rollback or calibrated fees. Applicable ordinary tests must still pass in hosted CI under C1. A key
  leaves `pending` in the milestone that ships the code it exercises (the 72 `intent_*` keys in M5; the 39 `test_*` keys
  per [inventory §2.5](docs/plan/parity-inventory.md#25-upstream-e2e-names)), and strict mode (M8) allows none. The
  eight ops without an upstream e2e test are submitted by scenario 18, and the M5 table-driven plan test covers all 83
  ops ([§12.7](#127-m5-intent-parity);
  [inventory §8](docs/plan/parity-inventory.md#8-inconsistencies-found-and-how-this-inventory-resolves-them) #9).

### 9.5 Python oracle and interop

Committed vectors remain ordinary Rust test inputs. Q30 moves regeneration, container and emulator execution to
optional targeted hosted jobs. The lane design below is a reference, with no local or device execution requirement.

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
  by each side and used by the other, including legacy Fernet and Ansible files (Keychain argv and failures use injected process tests; real Keychain verification is deferred by Q30);
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
  [§5.10](#510-the-pr-u17-core-patch)). A future separately requested device check could cover: the HID
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
- Optional bounded hosted `cargo +nightly fuzz` runs: btauth header parsing and `verify`, JSON-RPC frame parsing,
  `MetagraphView`, event splitting, `UnsignedExtrinsic` import, keyfile detection and `keyfile_json`, core decoders
  wrapped at the boundary.
- `criterion` baselines on committed finney payloads (netuids 1, 3, 64) and a synthetic 2,500-UID payload: the three
  metagraph decode paths, V15 parse, key building for 10k keys, map-page decode, sign and assemble, btauth throughput,
  `conform` at 256 and 2,500 entries. Use bounded hosted comparisons for specific regressions; shared-runner timing is informational.
- Optional hosted jobs use the baselines and targets that exist then (btauth/1 header
  parsing and `verify` from M0; JSON-RPC frame parsing and `MetagraphView` from M1), and each milestone adds its own
  targets as it lands them: M2 keyfile detection, `keyfile_json` and event splitting; M3 the btauth server path; M5
  `UnsignedExtrinsic` JSON import. Q30 removes default nightly campaigns and M9 time escalation.

### 9.7 CI lanes and what each catches

All substantial validation runs on free `ubuntu-latest` hosted runners (Q23, Q30). Local development does not repeat
these matrices. Fix a red required hosted check before claiming the affected change is accepted.

| Lane | Trigger | Coverage | Acceptance |
|---|---|---|---|
| `pr-gates` | pushes to `main` and relevant PRs | formatting, Clippy, units, doctests, goldens, committed oracle vectors, `FakeNode`, `FakeChain`, source/pin/parity/dependency guards, supply audit, selected features and MSRV | applicable jobs on the pushed commit |
| targeted oracle / interop / localnet | manual dispatch or a narrow relevant path change | a concrete cross-language or real-chain question | optional hosted evidence; report unexecuted coverage |
| targeted fuzz / performance / broader features | manual dispatch for a relevant risk | bounded robustness or performance checks and selected feature combinations | optional; timeouts and no default 315-run powerset |
| docs | relevant changes | rustdoc and book build | required when touched; downloadable artifacts, no Pages |

Cancel superseded development runs. Do not copy hosted build caches back to the laptop. Mainnet-clone, full-node,
scheduled multi-network campaigns, release, device and automatic-upkeep jobs are outside current development.
Existing workflow files must follow this scope before being enabled. Never add or toggle the `CI` label to run CI.

### 9.8 Supported platforms

The API design targets Linux x64/ARM and macOS Apple/Intel. Current hosted acceptance covers free `ubuntu-latest`
Linux x64. Record cross-compilation and platform coverage only when executed; a Linux run does not establish native
macOS, Keychain, ARM or hardware behavior. Local Mac Studio builds, cross-toolchain setup and release artifacts are
deferred by Q30. Windows and musl remain unsupported. Neuron image deployment is outside current development.

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

Current hosted jobs are the bounded client lanes of [§9.7](#97-ci-lanes-and-what-each-catches). Automatic upstream
campaigns, upkeep and release publishing are deferred (Q30). Refresh source/pins when client development needs it;
run costly regeneration or interoperability work through targeted free hosted jobs. Mainnet cloning is removed.
Do not reproduce a hosted pipeline locally while waiting for CI.

Dependency guards retain the approved Q26-Q28 rules: exactly one built core-pinned BLS implementation, the recorded
dormant BLS lock entry, and exact inactive lru/ruzstd advisory exceptions with raw audit output and activation guards.
All other source, license, advisory and layering checks remain applicable in hosted CI. Do not clear global caches
or add local toolchains to reproduce the supply/feature matrix.

### 10.3 A pin bump, step by step

The command is `cargo xtask pin --parity vN`
([architecture §12.3](docs/plan/architecture.md#123-what-a-pin-bump-does-xtask-pin---parity-vn)).
Q30 makes this a reference for source/pin maintenance, with costly oracle, recorder and emulator stages restricted
to optional targeted hosted jobs. Commit a reviewed source update and inspect applicable hosted checks. Record a
blocked refresh explicitly; no local full gate or milestone-driven campaign is required. Automatic upkeep is deferred:

1. Refuse unless release `vN` is final (not a pre-release), finney's finalized runtime reports spec `N` with the
   expected `:code` hash, and the `:vN` localnet image is published (checked with GHCR's anonymous pull token; the
   digest is written to `[localnet].pinned`). Upstream builds that image after the mainnet mirror push (32 minutes
   after promotion for v477), so the next 30-minute poll retries.
2. Set `[parity]` to `vN`; re-vendor `[artifacts]` from the monorepo (never from the live website, which deploys only
   after mainnet promotion).
3. `xtask extract` refreshes the extracted tables; `xtask regen` refreshes catalogs, interfaces, dispatch, descriptors,
   docs and reference pages. The descriptors come from finney's V15 metadata at spec `N`, which `xtask pin` fetches
   itself and commits with its `index.toml` row; the drift job then opens no PR for that `(spec, :code)`.
4. Optional targeted hosted refresh: if changed behavior needs new vectors, run `xtask oracle` or upstream
   recorders within one bounded free job, preserve input identities and review the resulting diff. A stale or
   unavailable result is recorded; no local image/node fallback is permitted.
5. Compute the core-input fingerprint. If it changed, move `core_tag`. While the core patch exists, run
   `cargo xtask core-patch rebase vN` (a conflict is `core-patch-conflict`), or, when `vN` contains PR-U17, delete the
   two core-patch tables and `[core.patch]` and adapt `rtao-testkit` to the API as merged
   ([§5.10](#510-the-pr-u17-core-patch)). Rewrite every reference generated from `[core]` (the root manifest, the
   template, the README and book snippets; a repository test checks them), run
   `cargo update -p bittensor-core -p w3f-bls -p subtensor-macros -p subtensor-linting` and `xtask core-surface`,
   optionally refresh changed Ledger transcripts in bounded hosted CI (`ledger-stale` if absent), and mark the bump `core-bump` and
   `downstream-patch-change`.
6. `xtask parity report` writes the porting report: changed blobs per `map.toml` row with diffs, the symbol diff joined
   to Rust items, new ops and reads (added to `pending.toml`), oracle-vector diffs, auto-bless candidates.
7. Commit a reviewed source update with its report and inspect applicable hosted checks. Automatic PR setup is deferred. Tests that fail because Python behavior changed point at the exact op through the oracle
   diff.

### 10.4 Drift classification

`xtask drift diff` compares pallets, calls (with exact shape hashes and V16 deprecations), events, errors, storage
(hashers, shapes, modifiers, defaults), constants, runtime APIs and versions, transaction extensions, extrinsic versions
and `System.Version` ([architecture §12.5](docs/plan/architecture.md#125-drift-classification)):

| Class | Examples | Effect |
|---|---|---|
| `surface-unchanged` | only `System.Version` (spec 478) | reviewed index/spec-range update; automatic merging remains deferred |
| `additive` | new calls, events, errors, storage, runtime APIs; a new call in any pallet stays additive, because wrapper-call shape hashes treat the outer `RuntimeCall` as opaque | regenerated descriptors; the coverage gate forces wrap-or-raw-only decisions |
| `behavioural` | moved event index, storage type or default change, runtime-API version bump, changed projection hash | model review; typed paths on live networks fall back automatically meanwhile |
| `breaking` | moved pallet, call or error index; changed arguments of a wrapped call; removed wrapped call or used storage; a new extension with payload; `transaction_version` change; V15 no longer offered; `spec_name` change; any move toward extrinsic v5 | the extension guard and per-call `verify_surface` refuse affected signing on live networks until the regenerated descriptors ship |

### 10.5 Compatibility matrix and run-time guard

`xtask compat` derives `compat.json` and `COMPAT.md` from recorded source/test evidence: rtao version, core tag and commit, parity tag
and Python source version (a PyPI release only if one was published from that commit), validated spec range, metadata
pins, localnet digests, btauth version, interop target. "Validated" means the drift diffs across the range contain no
`breaking` class and e2e plus interop passed on the listed digests.
At run time `Client` compares the connected `(spec, metadata hash)` with the compiled-in range and only warns outside
it; per-call `verify_surface` decides what still works
([architecture §12.7](docs/plan/architecture.md#127-compatibility-matrix-and-runtime-guard)).
Release tags, cross-platform binaries, publishing and device runs are deferred by Q30. Development milestones
record client test coverage without claiming release acceptance.

### 10.6 Maintenance playbook

([architecture §12.9](docs/plan/architecture.md#129-maintenance-playbook)). The table below describes how
source changes are handled. Q30 defers its scheduled automation and keeps costly refreshes in optional hosted jobs;
it does not authorize local containers, nodes, oracle builds or recording campaigns.

| Upstream event | Source/test signal | Client work |
|---|---|---|
| runtime bump, surface unchanged | reviewed metadata/source diff; no generated-interface change | none, or review the porting report |
| additive surface | regenerated descriptors and coverage report list new calls | per new call: wrap it in an intent, type a read, or list it as raw-only |
| behavioural surface | metadata/model diff; live typed paths fall back | review and fix the flagged models |
| breaking surface | reviewed metadata change; runtime guards refuse affected calls | fix the affected calls and descriptors, usually together with a core bump |
| Python adds an intent or read | source/pending/interface diff and available oracle vectors | port each new intent or read until its `pending.toml` entry can go |
| Python changes behavior only | blob change, symbol diff and oracle diff name the ops | port the diff; cosmetic edits auto-bless |
| core API change | an optional hosted core-source check can detect it; `core-surface` names the symbol | adapt `rtao-core`; no other crate changes |
| a staged PR ships in `core_tag` | `staged --check` fails because the module's probe (a symbol at the ungated target, or its vector suite) now passes | flip the `pub use`, delete the staged module |
| a chain rule mirrored by the framework changes (serving, rate limits, freeze window, tempo control, weights checks) | blob change on a chain-rule row of `map.toml`, or a diff of the extracted `chain_rules.json` | update `AxonPublisher`, `HyperparamPlanner` or `WeightScheduler` and their tests |
| btauth/1 → btauth/2 | blob change on `http_auth.py` or `signed-requests.mdx`; vectors fail | verify both versions, sign the configured one |
| third-party breaking release | reviewed dependency update or a failing hosted build | adapt the one affected crate |
| a pin bump moves `core_tag` while the core patch exists | `xtask core-patch rebase` cherry-picks PR-U17 onto the new tag ([§5.10](#510-the-pr-u17-core-patch)) | on `core-patch-conflict`, resolve the conflict like any upstream change and log it |
| PR-U17 ships in `core_tag` | `xtask pin --check` fails with "PR-U17 shipped: drop the core patch" ([§5.10](#510-the-pr-u17-core-patch)) | delete the two core-patch tables and `[core.patch]`; adapt `rtao-testkit::ledger_emu` to the API as merged |
| Ledger Live ships a new Polkadot app, or Speculos a new release | review changes relevant to current client work | update pins only for a needed reviewed change; record stale/absent transcripts, or refresh them in an explicitly selected bounded hosted check |

Track upstream changes relevant to implemented client behavior. Record pin drift and blocked refreshes under C5;
a new upstream release does not force a local infrastructure campaign (Q30).

## 11. Documentation plan

([architecture §13](docs/plan/architecture.md#13-documentation))

- **rustdoc**: `#![deny(missing_docs)]` on every library crate; crate READMEs as crate docs; `doc(cfg)` on
  feature-gated items; `#[doc(alias = "bittensor.<dotted.path>")]` on every mirrored item, so searching for
  `SetWeights` or `bittensor.reads.neurons.neurons` lands on the Rust item; hosted CI retains it with the book as
  downloadable docs artifacts from relevant hosted checks (Q23/Q30). `rtao-cli`'s
  `[[bin]]` sets `doc = false`: the binary `rtao` and the facade library `rtao` both document to `target/doc/rtao/`, and
  `cargo doc --workspace` warns "output filename collision" and lets one overwrite the other (reproduced with cargo 1.99
  on a two-crate workspace; `doc = false` removes it).
- **docify**: every example is a real `#[docify::export]` item in `examples/` or `tests/`. Offline examples (units,
  `pyfmt`, btauth vectors, `FakeChain` reads, intent plans) use `embed_run!`; node-dependent ones are items in
  `crates/rtao-testkit/tests/` (the `e2e` target), so an optional targeted hosted check can run the selected example. READMEs compile from `README.docify.md`,
  so examples cannot rot.
- **mdBook** (`book/`) mirrors the bittensor.com taxonomy so an agent that knows upstream's `llms.txt` navigates rtao's
  by analogy: `index`, `quickstart` (template, patch line, connect, typed read, plan and execute on localnet), `sdk`,
  `agents` (upstream's eight-step flow with `rtao tools`, `execute_tool`, `Policy`, `--dry-run`, error codes), `cli`,
  `guides` (validator, miner with btauth, owner tooling, commit-reveal and mechanisms, Null consensus, MEV shield,
  timelock, wallets and signers, Ledger, local development, deployment, observability, upgrades, testing your subnet),
  `concepts` (links to upstream raw markdown, not copies), `migration` (v11 Python → rtao generated from
  `parity/map.toml`; v10 Python → rtao; the core's blocking client → rtao; the divergence register), `reference`,
  `parity`, `compat`.
- **Generated reference** (`xtask docs` in hosted CI): `tx/<op>.md`, `query/<read>.md`, `errors/<code>.md`,
  `errors/chain/<Name>.md`, `hyperparameters/<name>.md`, following upstream's page templates with a `## Rust` section;
  `docs --check` keeps the hosted artifact current without storing another generated documentation tree locally.
  Pages cite sources as path plus item, without line anchors, so line shifts in
  runtime or pallet files change no page.
- **Catalogs** in upstream's schema plus a `rust` object per entry (type, role, output, CLI line, or read method), so
  tools that consume upstream's catalogs read rtao's unchanged.
- **`llms.txt` and `llms-full.txt`** following upstream's conventions, with a mapping block from each `tx/<op>` and
  `query/<read>` page to its upstream raw page, and a short agent card generated from the CLI tree.
- **Upstream log**: every pin bump appends its porting-report summary and oracle diff to `book/src/upstream-log.md`.
- **Per milestone**: each milestone ships the docs for what it ships; M9 completes the book and `llms.txt`. Until 1.0
  the README records completed source reviews and unexecuted platform/interop/hardware coverage without a release-readiness claim.

## 12. Milestones

### 12.1 Overview

The parity machinery exists from M0, so nothing ported early is redone. Development milestones end with reviewed
client source, docs and green applicable hosted checks. Releases and infrastructure campaigns are deferred by Q30. Item assignments (which intent, read
and CLI command lands where) follow [parity-inventory.md](docs/plan/parity-inventory.md), which resolves the ambiguities
of the architecture's milestone text ([architecture §14](docs/plan/architecture.md#14-milestones)).

| # | Milestone | Depends on | Headline exit check |
|---|---|---|---|
| M0 | Foundations and pins | — | goldens reproduced through rtao conversions; btauth/1 vectors green; no-host seam builds |
| M1 | Read path and the first typed read | M0 | metagraph fixture comparison through three decode paths; transport checklist passes in hosted CI |
| M2 | Validator | M1 | scheduler, nonce, reveal and conform tests against fixtures/FakeChain; no-host profile compiles in hosted CI |
| M3 | Miner | M2 | miner, admission and publisher tests; template profile compilation in hosted CI |
| M4 | Owner tooling, shield, CLI v0 | M2 | owner, shield, multisig and CLI tests; injected Keychain tests in hosted CI |
| M5 | Intent parity | M4 | intent plan, encoding, policy and offline signing tests; accurate parity inventory |
| M6 | Read parity | M2 (reads, namespaces, `blocking`); M4 and M5 for its CLI [I] | read, exact-hits and single-block tests; existing vector replay |
| M7 | Signers, EVM, PoW GPU, timelock UX | M5 [I] | signer, proof, APDU, framing, bridge, EVM and timelock fixture tests; hardware deferred |
| M8 | CLI parity | M5, M6, M7 | CLI tree and strict source/parity checks; unexecuted interoperability coverage recorded |
| M9 | Client hardening | M8 | applicable hosted tests, security review and accurate coverage; distribution deferred |
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

**Upstream tracking** follows client source needs from M2 onward. Review changes to implemented behavior and
record pin drift. Use applicable hosted source/pin tests; costly oracle or recording refreshes are optional targeted
hosted jobs. A milestone does not force a new upstream pin or the installation of scheduled upkeep (Q30).

**Steps and exit checks.** Q30 removes costly local execution requirements and defers release/device/upkeep work.
Named localnet, oracle, container, emulator and interop checks below are optional targeted hosted evidence; they
cannot block a development milestone or require a replacement local run. Keep their unexecuted coverage explicit.
Each milestone lists its steps with stable IDs (`M2-07`), in an order in which every step
depends only on earlier steps and earlier milestones. **Parallel work** notes describe logical independence;
Q30 prohibits simultaneous local Cargo jobs and duplicate build snapshots. Exit checks are numbered, and each is a command, a named test or a CI lane that passes or fails. They use these
conventions:

- unit, golden, oracle, `FakeNode` and `FakeChain` tests run in `cargo nextest run --workspace --all-features --locked
  --profile ci` (the `check` job); a check that names a test means the test exists and passes there;
- localnet scenario *n* is the test function `scenario_<nn>_<name>` (two digits) in `rtao-testkit`'s `e2e` target, run
  by `e2e.yml` as `cargo nextest run -p rtao-testkit --features docker --test e2e --profile e2e-fast` (`e2e-slow` for
  the 12 s lane), where `-E 'test(/^scenario_01_/)'` selects scenario 1;
- interop lane *n* is the test function `lane_<n>_<name>` in `rtao-testkit`'s `interop` target, run by `interop.yml`;
- "green" means that a run of the named workflow or job on the milestone's final commit of `main` succeeded.

**Common exit checks**, which every milestone adds to its own list:

- **C1.** Applicable `pr-gates` jobs pass on the final pushed commit. Workspace, feature, MSRV, supply and doc
  matrices run in hosted CI. Local matrix completion is not required.
- **C2.** Record every optional hosted check actually run and its result. Missing oracle, interop, localnet, hardware
  or platform coverage is stated explicitly and is not described as passing.
- **C3.** Active client steps are checked when complete and the progress log records the exit. Q30-deferred
  infrastructure steps are marked deferred, without a false completion checkbox.
- **C4.** Existing upstream filings retain accurate links and reviewed evidence. Filing every planned proposal is
  not a prerequisite to client development; new filings need a reproduced defect and a reviewed fix or report.
- **C5.** Source, lock, metadata and generated identities agree. Record pin drift and blocked refreshes; use hosted
  jobs for costly oracle or recording refreshes. Do not run a local campaign to force a milestone exit.
- **C6.** Docs describe implemented behavior and actual coverage; applicable hosted docs checks pass.
- **C7.** Release tags, binary artifacts, provenance setup, device runs and automated-upkeep handover are deferred
  by Q30. Development completion does not assert a release or 1.0 readiness.

**Security passes by cold readers** (Q19). M2, M3, M4 and M7 each end with a security pass, and M9 with the full review.
A local cold reader does each one: a fresh-context reviewer (a new agent session, or a person, that took no part in the
implementation) that receives only the repository at the milestone's final commit, the milestone diff
(`git diff <previous milestone commit>..HEAD`; the whole repository for M9) and this checklist:

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

**Upstream filing** (Q18, current task scope). Audit existing PRs/issues and their evidence in
[§15](#15-upstream-contributions), preserving accurate links (C4). Keep additional proposals local; finishing an
evidence step does not trigger a filing. The Ledger app item was filed on 2026-10-13 (Q22).

### 12.2 M0: Foundations and pins

**Goal.** A workspace that builds against the pinned core, with every guard later work relies on. **Depends on**
nothing.
**Deliverables.** Reviewed client source and hosted checks; the dual license; `upstream.toml`, `clippy.toml`, `deny.toml`; `rtao-core`,
`rtao-core-ext`, `rtao-primitives`, the `rtao-btauth` core, a minimal `rtao-signer` and `rtao doctor`; `xtask` pin,
import-upstream, extract v0, deps-check, core-surface, staged, lint-upstream and the parity file layer over all 357
`map.toml` rows; `rtao-testkit::fixtures`; the first `pr-gates` jobs and `xtask gates` as hosted orchestration; the
filing of PR-U17 from the user's fork of the monorepo, whose evidence predates M0 ([§15](#15-upstream-contributions)).

**Steps.** M0-01…M0-29 are listed in order, with their details, in [§12.14](#1214-first-steps):

| Area | Steps |
|---|---|
| prerequisites and PR-U17, relicensing, workspace and pins | M0-01…M0-08 |
| xtask | M0-09…M0-13 |
| the seam and its guards | M0-14…M0-20 |
| primitives, JSON and goldens | M0-21…M0-23 |
| btauth/1, `rtao doctor`, CI and docs | M0-24…M0-28 |
| record hosted development acceptance | M0-29 |

**Exit checks** ([architecture §14](docs/plan/architecture.md#14-milestones)):

1. The golden tests `golden_calls`, `golden_storage_keys`, `golden_signature_payloads`, `golden_extrinsics`,
   `golden_multisig` and `golden_ss58` pass: each `golden.json` section is reproduced through rtao conversions (M0-23).
2. `corpus_replay` passes on `core_tag` (M0-23).
3. `cargo nextest run -p rtao-btauth --profile ci` passes with every btauth/1 vector and negative case (M0-25).
4. `cargo xtask deps-check --profile no-host` passes over a no-host build of `rtao-core` and `rtao-primitives`
   (M0-11).
5. `cargo test -p rtao-core --test lock` passes: exactly one built `w3f-bls`, at `core_tag`, with only the guarded recorded dormant lock entry allowed (M0-14, Q26).
6. `cargo xtask import-upstream --verify-only`, `cargo xtask extract --check`, `cargo xtask parity check` (the file
   layer over 357 rows), `cargo xtask core-surface`, `cargo xtask staged --check` and `cargo xtask lint-upstream` pass
   (M0-09…M0-20).
7. The `serde_json/arbitrary_precision` tests of M0-22 pass.
8. `rtao doctor` passes on this repository, and on a fixture workspace without the patch line it fails and prints the
   exact line (M0-26).
9. `LICENSE-MIT`, `LICENSE-APACHE` and `NOTICE` exist. Package metadata declares `MIT OR Apache-2.0` for original
   crates and `(MIT OR Apache-2.0) AND Apache-2.0` for crates with upstream-derived files. The authenticated CPython
   datetime port in `rtao-primitives` and name suggestions in `rtao-cli` additionally require `AND PSF-2.0`; the pinned Unicode 15 printable table in
   `rtao-core-ext` additionally requires `AND Unicode-DFS-2016`. `xtask parity check` verifies those exact expressions
   and retained attribution; the hosted `supply` license check passes (M0-02). This clarifies Q1's existing third-party
   obligations without changing the original-code license.
10. Applicable `pr.yml` jobs `check`, `no-host`, `msrv`, selected `features` and `supply` pass in hosted CI
    (M0-27), and C1-C7 hold. No full local gate run is required.

### 12.3 M1: Read path and the first typed read

**Goal.** An async client that reads finney correctly and survives the public endpoints. **Depends on** M0.
**Deliverables.** Reviewed client source, docs and applicable hosted checks: the RPC session with ring TLS,
read client/config/runtime/cache, code generation, metagraph models/view, commitments, the first reads,
source/metadata diff tooling and FakeNode/FakeChain. Performance campaigns, scheduled monitoring, release
workflows and bot setup are deferred.
**Parallel work.** M1-01 and M1-02 are independent of each other; M1-10 and M1-11 need only M1-01; M1-15 needs nothing
from M1; M1-20…M1-24 are independent of each other.

- [x] **M1-01** Extraction: the M0 extractor extended to `PY/settings.py` and `PY/_transport/rpc.py`, with the seven
      resolved forms of [§8.3](#83-how-parity-is-enforced) and the test `extract_check_covers_named_tables`; the
      generated settings tables of `rtao_catalog::py` and the `py_rpc.rs` constants of `rtao-rpc` come from them.
- [x] **M1-02** `rtao-core-ext`: `ir::MetadataIr`, `ops::{metadata, storage, runtime_api}`, `value_ext`.
- [x] **M1-03** `rtao-runtime` and `xtask regen` v1, whose input is finney's V15 and V16 metadata at the pinned spec
      (`xtask pin` fetches it into `vendor/metadata/finney/` with its `index.toml` row and the `[networks.finney]`
      metadata pin, [§10.3](#103-a-pin-bump-step-by-step) step 3): `codegen/*.toml`; generated storage, calls, runtime
      APIs, constants, events, errors, extensions, selective index, shapes; headers without `spec_version`; semantic
      overrides (`PriceRao`, `StakeWeight`, `CompactBytesText`; `DefaultAccount` applied by rule over the pinned
      metadata to every `Default` `AccountId32` entry whose default is 32 zero bytes, pinned by the offline D-001 test
      `zero_account_default_decodes_to_zero_bytes` over the vendored metadata). `core-oracle` v0 and the live D-001
      sanity check (scenario 15) move to M2, where `Localnet` and their CI jobs exist [I].
- [x] **M1-04** `ShapeHash` (exact and projection, with the outer `RuntimeCall`/`RuntimeEvent`/`RuntimeError` enums
      opaque), `verify_surface`, `SurfaceReport`; `RuntimeHandle`, `RuntimeEntry`, `MetadataLevel`, `CallBytes` and
      `ComposeError` in `rtao-runtime`; cross-check against `PY/_generated/*.py`; `regen --check` joins the `check`
      job.
- [x] **M1-05** `rtao-testkit`: `FakeState`, `FakeNode` with `Fault`s (also over `wss://` with a self-signed
      certificate), `FakeChain`.
- [x] **M1-06** `rtao-rpc`: `Session` supervisor over tokio-tungstenite; `SessionConfig` defaults from `rpc.py`;
      `ReadMethod` allowlist with a per-frame `sent` flag; the reconnect table of
      [architecture §3.2.2](docs/plan/architecture.md#322-reconnect-semantics-port-of-rpcpy-each-row-a-fakenode-test-315).
- [x] **M1-07** `rtao-rpc`: policy classifier ported from `PY/_transport/rpc.py` over the generated `py_rpc.rs`
      constants; `EndpointPool` and archive pool; token bucket; metrics; `impl RpcClientT`.
- [x] **M1-08** `rtao-rpc` TLS: the ring-only `rustls::ClientConfig` with native roots plus `SessionConfig::extra_roots`
      and ALPN `http/1.1`, passed as `Connector::Rustls`; `max_message_bytes` sets both the frame and the message limit
      (a 20 MiB single-frame response is accepted); the caller-gone rule: a queued frame whose caller has gone is never
      transmitted.
- [x] **M1-09** `rtao-testkit`: the port of `PYT/unit/test_rpc.py`; the transport rows of the §3.15 checklist; a test
      pinning the tokio-tungstenite 0.30 handshake error API (Q16).
- [x] **M1-10** `rtao-client`: network presets (`finney`, `test`, `devnet`, `archive`, `local`) from `rtao_catalog::py`,
      extracted from `PY/settings.py` (`rtao-rpc` has no rtao dependencies, so presets live here).
- [x] **M1-11** `rtao_client::config` (port of `PY/config.py`): the seven btcli files' paths and environment overrides,
      `SETTABLE` (extracted with its type tags), `btcli.json` load and save, and the token-symbol and subnet-name
      caches; writes are atomic and off the reactor, and a corrupt file is renamed `.corrupt.<ts>`. `Client::connect`
      (token symbols), `NeuronConfig` (precedence, M2) and `rtao-cli` (M4) use it; the address, proxy and multisig
      books live in `rtao-cli`'s `books` module (readers in M4, writers with their commands in M5 and M8).
- [x] **M1-12** `rtao-client`: `HeadTracker` with gap backfill.
- [x] **M1-13** `rtao-client`: `Client::connect` (genesis check, ss58 42, finalized head, runtime version, extension
      guard, `verify_surface`, head tracker, upgrade watcher, token symbols, and the run-time compatibility guard over a
      compiled-in `compat.json` from `xtask compat` v0, which warns outside the validated spec range,
      [§10.5](#105-compatibility-matrix-and-run-time-guard)) and `Client::from_rpc` (for `FakeChain` and custom
      transports); `ClientError` with `code()` and `policy()`; dropping the last `Client` clone cancels the background
      tasks and closes the socket (`DropGuard`, a `FakeNode` test); `RuntimeCache` (V15 fetch, needle-only V14,
      `MetadataLevel`, LRU, parent-runtime resolution, Python-compatible disk cache).
- [x] **M1-14** `rtao-client`: `View`, `Snapshot`, `DynView`, `ViewExt` (`storage`, `fetch` coalescing at ≤256 keys per
      chunk, `storage_iter`, `call`, `constant`, `events`, dynamic reads).
- [x] **M1-15** `rtao_primitives::{drand, ratio::ratio_fraction, hyperparams::proportion_to_raw}`.
- [x] **M1-16** Metagraph: models; `Metagraph` with `MetagraphView` (exact shape gate), the `DecodeAsType` fallback and
      `from_dynamic` (port of `PY/metagraph.py:297-403`); the three-path differential test; a fuzz target.
- [x] **M1-17** Reads `metagraph` (Python's raw `get_metagraph` dict, reachable through `dispatch` from M2) and
      `token_symbols`. `xtask regen` v1 emits the `Subnets` namespace accessor on `Client` and `Snapshot`, named from
      the catalog `category`, with no inherent `metagraph` method: the read's `python` field is empty, and an inherent
      method would shadow the curated one.
- [x] **M1-18** `rtao_reads::commitments`: `NeuronCommitment`, `CommitmentMap`, the status machine and decoders, and
      `fetch_commitments`/`fetch_commitment_map` (`PY/metagraph.py:37-125, 258-291, 406-560`).
      `NeuronCommitment::reveals_at` uses `rtao_primitives::drand::reveal_time` over the core's `GENESIS_TIME` and
      `DRAND_PERIOD`, not `rtao_tx::timelock`, which sits above `rtao-reads`. The curated `metagraph()` is
      `rtao_metagraph::SubnetsMetagraphExt` (`metagraph`, `metagraph_with`), implemented for `rtao_reads::Subnets` and
      exported from `rtao::prelude`, with commitments included by default as in Python.
- [x] **M1-19** The commitments half of the port of `PYT/unit/test_v446_accounting_and_commitments.py` (the locks half
      lands with its reads in M6-01).
- [x] **M1-20** Maintain benchmark and fuzz target source for implemented parsers/decoders. Existing regressions
      run as ordinary hosted tests. Benchmark measurement and fuzz campaigns are optional bounded hosted checks
      for a concrete client change; do not add scheduled campaigns or local targets/toolchains.
- [x] **M1-21** Maintain `xtask drift diff` and its committed fixture tests. Metadata update tooling preserves
      source and hash identities. Automatic monitoring, bot tokens and PR generation are deferred; a targeted
      read-only hosted probe may supply evidence when needed.
- **M1-22 optional hosted.** Select a relevant feature check, core-source canary or read-only endpoint probe
  for a concrete client concern. No mandatory powerset, nightly campaign, node smoke or scheduled probe setup.
- **M1-23 deferred by Q30.** Release automation, binary builds, cross-platform rehearsals and local provenance
  setup are outside current development. Keep useful source and docs; do not build release artifacts locally.

- **M1-24 optional hosted.** If a client rate-limit defect needs evidence, make a bounded read-only endpoint
  measurement in CI and record it. Do not run a local quota/throughput campaign.
**Exit checks (hosted CI):**

1. Transport/TLS tests, the port of `test_rpc.py`, runtime/cache tests and metagraph three-path tests pass against
   committed fixtures and FakeNode/FakeChain.
2. `xtask drift diff` classifies committed unchanged/additive/behavioural/breaking fixtures correctly.
3. Regeneration, extraction and applicable source/pin guards pass. Current/MSRV and relevant feature checks pass.
4. A live endpoint/canary/performance check, if run, is recorded as optional evidence. Missing platform or live
   network evidence is documented. C1-C7 apply; releases and scheduled campaigns are deferred.


### 12.4 M2: Validator

**Goal.** Validator APIs and a weight-setting example built only from rtao's public API. **Depends on** M1.
**Deliverables.** Reviewed client source, docs and applicable hosted checks: signing roles/wallets,
submission/receipts/nonces, catalog/tx, intents and reads, validator scheduler/runtime APIs and example,
MetagraphSync, btauth client, fixture replay and security review. Oracle refresh, image and localnet
integration are optional targeted hosted evidence; deployment and upkeep are outside this milestone.
**Parallel work.** M2-01…M2-02 (keys and wallets) and M2-03…M2-05 (extraction, catalog, oracle) are independent of each
other; M2-16 needs only M1. Q29 waives M2-24's testnet soak.

- [x] **M2-01** `rtao-signer`: `SigningRequest`, `SignerCaps`, roles (`Role`, `Coldkey`, `Hotkey`, `KeyView`,
      `KeySource<R>`, `DynKeySource`).
- [x] **M2-02** `rtao-wallet`: btcli layout, `HotkeyFile` through the staged `keyfile_json` (no `host`), `Wallet` and
      encrypted coldkeys behind `encrypted`, the password chain of `PY/wallets.py:62-118` (steps 1-4 and 7 here; steps
      5-6, the macOS Keychain and the macOS dialog, in M4), create and regen; the keyfile differential test; staged
      `keyfile_env`; maintain keyfile-detection and `keyfile_json` fuzz target source for optional targeted hosted checks
      ([§9.6](#96-property-tests-fuzzing-benchmarks)).
- [x] **M2-03** Extraction: `PY/error_map.py`, `PY/result.py`, `PY/executor.py` constants, the remaining
      `PY/settings.py`, `PY/fee_filters.py` (equal to the runtime's `fee_filters.rs`, parsed with `syn`), custom
      transaction codes (`common/src/transaction_error.rs`), intent field defaults and class variables, read parameter
      defaults and requiredness (`read_params.json`), and the environment variables the Python package reads
      (`env_vars.json`).
- [x] **M2-04** `rtao-catalog`: `INTENTS`, `READS`, `ErrorCode` (18), `CHAIN_ERRORS` (390), `py::*`, `rt::*`.
- **M2-05 optional hosted refresh.** Preserve committed oracle vectors/seeds for implemented ops, reads and
  pure functions. If refreshed vectors are needed, build the pinned oracle only in a bounded free hosted job and
  review the resulting diff. No local image build or GHCR setup is a client milestone prerequisite.
- [x] **M2-06** Staged `ops::{receipt, fee, extrinsic_lookup, dispatch_error}`, with a `FakeChain` receipt test in which
      a `Sudo.sudo` fails inside `Sudid` (PR-U1 and PR-U2 evidence,
      [§5.8](#58-known-core-defects-and-how-rtao-handles-them) #4).
- [x] **M2-07** Submission (`rtao-client`): `NonceManager`/`NonceLease`, one manager per `Client` (`client.nonces()`,
      shared by every clone and executor); era 128 anchored at the finalized head; metadata-hash mode with the
      RFC-0078 digest cached per spec; sign, assemble, submit, watch; `SubmitFailure`; `CallBytes` spec stamping; typed
      `PoolRejection`; `FateUnknown`; reconciliation; the watch statuses of D-024 (`retracted` keeps watching,
      `finalityTimeout(h)` resolves from block h, `dropped` reconciles when `reconcile` is on); the event-splitting
      fuzz target source is available for optional targeted hosted checks.
- [x] **M2-08** `rtao-tx`: intent traits, `Executor` (`plan`, `preflight`, `execute`, `dyn` twins), `Plan`, `Policy`
      with the harness of a table-driven equivalence test against the core's constructors
      ([§5.8](#58-known-core-defects-and-how-rtao-handles-them) #9), whose pairs land with their intents: 3 in M2-12,
      5 in M4-06 and the other 13 in M5-03, for 21 of 22 at the M5 exit. `ExtrinsicResult<O>`, `ChainError`,
      `PolicyError`, `TxError`, `classify` and its name, code and custom-code gates.
- [x] **M2-09** `rtao-intents` and `rtao-reads`: generated interfaces, `AnyIntent`, `defaults.rs`, `ReadName`,
      `ReadSpec`/`ParamSpec`, and `dispatch`, which fills the extracted `read_params.json` defaults; `RecordingView` and
      the exact-hits test with `parity/read_hits_extra.toml` ([§9.3](#93-fakes-at-the-right-layers)); `pending.toml`
      listing the 74 unported intents and 87 unported reads.
- [x] **M2-10** The pass-through generator (`generated/passthrough.rs`), driven by `codegen/intent_types.toml` and
      `intent_callmap.toml`; `transfer_all` is its first output, and it later generates M4's nine and M5's 36 bodies, so
      no body is hand-written first [I: building it here means none of the 10 M2 and M4 bodies is written twice].
- [x] **M2-11** `weights::conform` (`round_ties_even`, Python's order); `TimelockCommitter`; staged `epoch_schedule` and
      `timelock_commit` (PR-U5 evidence), with the safety-net vectors: a skip at a scheduled slot b fires the epoch at
      b + 1; a skip at an owner-triggered slot b with step counter c fires it at b + tempo + 1 − c; a tempo lowered
      below the counter in block t fires it at t + 1.
- [x] **M2-12** The 9 M2 intents: `set_weights`, `commit_weights`, `reveal_weights`, `serve_axon`, `serve_axon_tls`,
      `serve_prometheus`, `reset_axon`, `transfer`, `transfer_all`. The `Policy` equivalence test (M2-08) gains its
      three M2 pairs: `transfer`, `transfer_all` and `transfer_allow_death` (↔ `Transfer { keep_alive: false }`).
- [x] **M2-13** The 13 M2 reads: `block_time`, `timestamp`, `blocks_since_last_step`, `blocks_since_last_update`,
      `epoch_status`, `max_weight_limit`, `min_allowed_weights`, `reveal_period`, `weights_rate_limit`, `uid`,
      `commit_reveal_enabled`, `mechanism_count`, `weights`.
- [x] **M2-14** Strict client-source parity machinery: the remaining `xtask parity check` layers (symbol, catalog, extraction, chain-source,
      environment, coverage, available committed oracle evidence, completeness with `parity/exports_extra.toml`) and `divergences.toml` from
      Appendix C. From here on, `parity/map.toml` rows (seeded in M0 from
      [python-module-map.md](docs/plan/python-module-map.md)) are blessed as they are ported. Q30 permits an explicitly
      unverified first oracle batch; historical evidence and removal checks remain mandatory.
- [x] **M2-15** `MetagraphSync` with the validator preset (`EveryEpoch { fallback_blocks: 100 }`, `Preset::Validator`,
      `MetagraphHandle`, `MetagraphDiff` with `replaced`/`deregistered` for score resets, last-good snapshot on failure,
      architecture §8.3), because `NeuronCx` holds the handle and `Validator::on_metagraph` takes the diff.
- [x] **M2-16** The `rtao-btauth` client adapter: `SignedClient` with `fan_out` and `fan_out_stream` (`FanOut` defaults:
      256 concurrent, 3 s connect timeout, 12 s per attempt, a call deadline of timeout + 1 s, no retries), over
      `rtao_primitives::axon::{AxonTarget, axon_base_url}` and `Metagraph::axon_targets()`, re-signing every attempt;
      its loopback `https://` `FakeNode` test through the ring config. `NeuronCx.http` is a `SignedClient`, and the
      facade's `neuron` feature already enables `btauth-client`.
- [x] **M2-17** `rtao-neuron`: `NeuronConfig`, `NeuronRuntime::validator`, `NeuronCx`, `StateCell` (over
      `std::sync::RwLock`, with a `compile_fail` doctest for a guard held across an `.await`), `Validator` with
      `StepPolicy` (errors and panics counted and backed off, never ending `run()`); `WeightScheduler` (cadence with
      per-hotkey jitter, one-RPC preflight at the best head as Python reads it, the Null-consensus missing-permit
      refusal, commit-reveal version gate, conform, hold-previous guard, path selection, timelock, Python submit
      defaults, reconciliation); reveal watcher that follows the commit's actual reveal epoch and attributes misses
      (`reason`); `ClockGuard`; `EpochClock`; `StateStore`, with the test `validator_restart_resumes_state` (kill and
      restart resumes from persisted state); telemetry (`/metrics`, `/healthz`, `/readyz`); two-stage shutdown.
- [x] **M2-18** `crates/rtao/examples/validator.rs` (the example of [§7.2](#72-a-validator)) and its line in the
      `features` job.
- **M2-19 optional hosted infrastructure.** Retain `Localnet`/`Scenario` as a test-harness design reference
  (§9.4). Implement only the setup needed by a specific targeted hosted integration test; no local node setup,
  full-node operation or campaign is required for the client milestone.
- **M2-20 optional hosted integration.** An explicitly dispatched job may select scenarios relevant to the
  changed client code from §9.4 (1, 2, 3, 5, 6, 7, 8, 15, 16 or 17) or a `core-oracle` comparison. Use one pinned
  image, a small selection and a timeout. No automatic push/nightly/multi-network campaign or blocking
  `e2e-required` gate is required.
- [x] **M2-21** `parity/e2e_map.toml` (115 keys, each `rtao`, `na` or `pending = "M<k>"`, [§9.4](#94-localnet-e2e)),
      with the six `test_*` keys of [inventory §2.5](docs/plan/parity-inventory.md#25-upstream-e2e-names) that M2 can run
      mapped to rtao tests.
- [x] **M2-22** Maintain reviewed source/pin/extraction/regeneration/parity update tooling and hosted guards.
      Record relevant upstream changes and blocked refreshes. Oracle/recording stages are optional targeted hosted
      work; forced full-pipeline replays, bot deployment and scheduled upkeep are deferred.
- **M2-23 optional hosted image check.** Keep the no-host validator image recipe as source. If the recipe
  changes, a targeted free hosted job may build it and check TLS through FakeNode; do not build it locally.
- **M2-24 waived by Q29.** No testnet soak, funding, subnet setup or host is required. Preserve prepared wallet
  and recovery material. Client tests run in hosted CI; costly infrastructure checks are optional under Q30.
- [x] **M2-25** The validator guide in the book ([§11](#11-documentation-plan)).
- [ ] **M2-26** Security pass by a local cold reader ([§12.1](#121-overview)), in `docs/security/m2.md`; focus: keyfile
      loading and `keyfile_json` (ordinary malformed-input regressions passing in hosted CI), the password chain, `Zeroizing`/`secrecy` with a test that
      `Debug` output redacts secrets, and the signing path.

**Client-source step checkpoint, 2026-10-16.** The checked implementation and
validator-guide steps are verified at
`3772699612278d4592648d07232241430e96791b` by hosted
[run 38897437100](https://github.com/sam0x17/rtao/actions/runs/38897437100).
This records their individual source, fixture and guide completion. M2-26 and
the M2 milestone exit remain open because M2-S02 is unaccepted; the retained
owning-boundary proposal does not close that finding.

M2-25 covers the validator guide and its compiled source example. Checking it
does not certify all §11 documentation contracts. The newer crate-README,
complete-book, LLM-text and migration changes require their own hosted validation.
Optional hosted checks and the waived soak retain their declarations above.

**Exit checks (hosted CI):**

1. Signing, wallet, nonce/submission, receipt, reconciliation and validator scheduling/restart tests pass against
   fixtures and FakeNode/FakeChain, including commit-reveal refusal and unsafe-era cases.
2. Existing conform and native-balance vectors pass; source/pin/parity/generated guards pass.
3. No-host dependency/profile checks pass in hosted CI and the validator example compiles there.
4. `docs/security/m2.md` records findings and their resolution or explicit acceptance.
5. Any targeted oracle, localnet, image or interoperability result is optional recorded evidence. No soak,
   deployment, forced pipeline replay or infrastructure lane is required. C1-C7 apply.


### 12.5 M3: Miner

**Goal.** A miner behind btauth/1 with metagraph-based admission, and the subnet template. **Depends on** M2.
**Deliverables.** Reviewed client source, docs and applicable hosted checks: btauth server/admission/limits,
miner runtime APIs, AxonPublisher, metagraph persistence, immunity read and template source. Compile and test
template profiles in hosted CI; cross-language and real-chain checks are optional hosted evidence.
**Parallel work.** M3-03 and M3-04 are independent of M3-01…M3-02.

- [ ] **M3-01** `rtao-btauth` server (`BtAuthLayer`, `VerifiedCaller`, `CallerPolicy`, `MetagraphPolicy`,
      `PerHotkeyLimiter`, `RedisNonceStore`, `Json<T>`): `Verifier::authenticate` and `Authenticated::commit` in the
      order authenticate → policy → commit → limit (D-032); the bounded `InMemoryNonceStore` (503 at the bound, never
      evicts); the body limit applied before buffering, with the runtime's `DefaultBodyLimit` matching it (a 3 MiB
      signed body reaches a `Json<T>` handler); `Json<T>` parses through `serde_json::Value` first
      ([§5.8](#58-known-core-defects-and-how-rtao-handles-them) #10); feature `legacy-v10`. (The client adapter landed in
      M2-16.)
- [ ] **M3-02** Maintain btauth parser/server fuzz target source and ordinary regression tests. A targeted
      fuzz run is optional hosted work with a time limit; no local or scheduled campaign.
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
- **M3-07 optional hosted integration.** Scenario 9 can check idempotent serving and 401/403/413/429 behavior
  against a pinned localnet when a specific client change needs it; ordinary publisher/admission tests use fakes.
- **M3-08 optional hosted interop.** Target the relevant btauth, neuron, wallet or wire-byte case from lanes
  1-4 and retain reviewed keyfile vectors. Real Keychain acceptance is deferred; M4-14 tests injected process
  behavior. No cross-language container campaign is a milestone prerequisite.
- [ ] **M3-09** Maintain validator/miner template members and compile/test them against the changed client
      source in hosted CI. Test setup and generated files with fixtures. Image builds and compose integration
      are optional targeted hosted checks; the generated project does not require deployment to pass ordinary CI.
- [ ] **M3-10** The miner guide in the book ([§11](#11-documentation-plan)).
- [ ] **M3-11** Security pass by a local cold reader ([§12.1](#121-overview)), in `docs/security/m3.md`; focus: the
      btauth/1 verifier (freshness window, nonce-store bounds, body limit enforced before buffering, header-parsing fuzz
      target) and `PerHotkeyLimiter`.

**Exit checks (hosted CI):**

1. Hosted btauth canonicalization, authentication/admission/limits/replay and miner runtime/persistence tests pass.
2. `cert_rotation_republishes`, publisher idempotence and serving refusal tests pass against fakes/fixtures.
3. Generated template source and relevant profiles compile and ordinary tests pass against the changed client.
4. `docs/security/m3.md` records findings and their resolution or explicit acceptance.
5. Any cross-language, image or real-chain check is optional hosted evidence; no node or compose deployment is
   required. C1-C7 apply.


### 12.6 M4: Owner tooling, shield, CLI v0

**Goal.** Subnet owners can create, start and configure a subnet, and the CLI covers the operator loop. **Depends on**
M2.
**Deliverables.** Reviewed client source, docs and applicable hosted checks ([§12.1](#121-overview)); 11 owner and registration intents, the MEV-shield pipeline,
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
- [ ] **M4-10** CLI: globals, `config` (5), `doctor`, `explain`, `tools`, `call`; the node-managing `localnet` command
      is deferred under Q30. Generated `tx` (20 ops) and
      `query` (38 reads); the rtao-only `wallet save-password-env` (architecture §6.2, §10.4; over the M2 staged
      `keyfile_env`).
- [ ] **M4-11** CLI: `wallet` create, new-coldkey, new-hotkey, regen-coldkey, regen-hotkey, regen-coldkeypub,
      regen-hotkeypub, show, list, balance, inspect; `weights` set, commit, reveal; `axon` set, reset.
- [ ] **M4-12** CLI: `hparams` set, get, get-take, check-start, start, set-symbol, get-identity, set-identity, trim,
      trim-null-batch, trim-null, mechanisms count, mechanisms emissions; `subnets` register, create, metagraph.
- [ ] **M4-13** Ports of `PYT/unit/test_pow_registration.py` (the solver and the intent keep the runtime challenge),
      `test_cli_null_pruning.py` (owner pruning must advance on chain, not only return successful receipts) and
      `test_natural_name_key.py` (CLI listings order `coldkey2` before `coldkey10`).
- [ ] **M4-14** Verify Keychain service names, argv, deferred password behavior and errors through injected process
      tests in hosted CI. Real Keychain and temporary-default-keychain rehearsals are deferred by Q30; do not change
      host settings or invoke the user's Keychain during development.

- **M4-15 optional hosted integration.** Select an owner/shield scenario (4, 10 or 11), a relevant implemented
  read/plan case (interop 5/8), or CLI JSON case (interop 9 through PinProxy) only when the changed client needs
  evidence beyond ordinary fixtures. Record what ran; no local nodes, compose stack or full command campaign.
- [ ] **M4-16** Maintain the template owner-tool source and compile its public `OwnerOps` example in hosted
      CI. A targeted hosted image or owner-flow integration check is optional; no compose deployment is required.
- [ ] **M4-17** `crates/rtao/examples/owner.rs` (the example of [§7.4](#74-an-owner-operation)) and its line in the
      `features` job; the four `test_*` keys of [inventory §2.5](docs/plan/parity-inventory.md#25-upstream-e2e-names)
      assigned to M4.
- [ ] **M4-18** The owner and MEV-shield guides in the book ([§11](#11-documentation-plan)).
- [ ] **M4-19** Security pass by a local cold reader ([§12.1](#121-overview)), in `docs/security/m4.md`; focus: coldkey
      unlock, CLI prompts and warnings about secrets passed in argv, the MEV-shield envelope, and the raw `call` path.

**Exit checks (hosted CI):**

1. Hosted owner/planner/shield/raw/multisig tests pass with committed metadata, fixtures and FakeChain.
2. CLI parsing, prompt/error/JSON/book/config tests pass for implemented commands; injected Keychain tests pass.
   Real Keychain and hardware compatibility are documented as unverified.
3. The owner example and template member compile in relevant hosted profile checks.
4. `docs/security/m4.md` records findings and their resolution or explicit acceptance.
5. Targeted owner, shield and cross-language JSON integration checks are optional hosted evidence. No local
   rehearsal or deployment is required. C1-C7 apply.


### 12.7 M5: Intent parity

**Goal.** All 83 intents. **Depends on** M4.
**Deliverables.** Reviewed client source, docs and applicable hosted checks ([§12.1](#121-overview)); the remaining 63 intents, the offline-signing path, reads
`proxies` and `root_claim_threshold`, 20 CLI commands, generated `tx` reference pages complete.

- [ ] **M5-01** `prepare_call`/`submit_signature` with Python-compatible `UnsignedExtrinsic` JSON (the raw path landed in
      M4-04), with ordinary payload fixture tests and optional targeted hosted interop lane 6; reads `proxies` and `root_claim_threshold`
      (`set_root_claim_threshold`'s verify read); the multisig book and call-cache writers; the `UnsignedExtrinsic` JSON
      import has fuzz target source for optional targeted hosted checks.
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
      extracted `INTENT_SAMPLES` (`PYT/harness/samples.py:35`, already all 83 catalog operations), rewrites positive
      subnet identities to two fixture subnets while retaining root 0 and distinct destination relationships,
      and uses the slot hotkey except where a different destination hotkey is required. It asserts that `plan()` returns `Ok`, that `plan.op` equals the
      op, that the call data is non-empty, and that the `(pallet, call)` decoded from its first two bytes at the live
      metadata is one of the op's catalog `wraps`. An op whose `plan()` needs chain state gets a seed step, never an
      exemption. PoW uses a fresh deterministic difficulty-one nonce-zero proof for the public FakeChain challenge,
      without a mining worker. Ordinary hosted fixture coverage maps the 72 catalog `intent_*` keys; retired calls
      remain n/a. Call-tree equality with Python stays in the offline oracle tests, which use the unrewritten samples.
- [ ] **M5-05** CLI (20): `stake` aliases add, remove, move, move-swap, transfer, swap, burn; `hparams` set-take,
      stake-burn; `proxy` create, add, remove, kill, list, execute; `multisig` add, list, show, remove, pending; `tx`
      complete; hidden v9 aliases of the groups this milestone first ships (inventory §7.3).
- **M5-06 optional hosted integration.** Scenarios 11-13/18 and interop 6-8 can supply targeted evidence for
  changed intent, offline signing or multisig code. Ordinary plan/encoding/import/receipt tests run against
  committed fixtures and FakeChain; no campaign or node deployment is required.
**Exit checks (hosted CI):**

1. Hosted plan/encoding/policy tests cover implemented intents, including the 83-op table-driven test and the
   21 supported core-constructor pairs. Existing oracle vectors replay successfully.
2. Offline payload import/authentication, signing attachment, proxy/multisig and receipt tests pass with fixtures
   and FakeChain, including malformed/stale payload refusal.
3. No implemented intent remains incorrectly listed as pending; parity/source/generated guards pass. Missing
   fresh oracle or chain evidence is recorded without a full parity claim beyond executed coverage.
4. Targeted interop/localnet checks are optional hosted evidence. C1-C7 apply.


### 12.8 M6: Read parity

**Goal.** All 102 reads, namespaces and the blocking facade. **Depends on** M2 for the reads, namespaces and the
`blocking` facade; M4 (CLI v0 machinery) and M5 for its 24 CLI commands [I]: `root claim`, `root allocate`, `root move`,
`root swap`, `conviction add`, `conviction mode`, `conviction accept`, `conviction move`, `collateral add` and
`collateral set-min` construct M5 intents (`ClaimRootWithHotkey`, `StakeIntoBasket`, `MoveStake`, `SwapBasket`,
`LockStake`, `SetPerpetualLock`, `SetRejectLockedAlpha`, `MoveLock`, `AddCollateral`, `SetMinCollateral`), and
`root register` the M4 `RootRegister`. The reads are off the critical path.
**Deliverables.** Reviewed client source, docs and applicable hosted checks ([§12.1](#121-overview)); the remaining 62 reads, the remaining namespace accessors and
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
      with its `parity/read_hits_extra.toml` rows reviewed; existing read oracle fixtures at head and pinned; fresh interop samples are optional targeted hosted evidence.
- [ ] **M6-03** `rtao::blocking::{SyncClient, SyncSnapshot, Subtensor, set_weights, close_shared_clients}` over
      `rtao_client::blocking::BlockingRuntime` (facade feature `blocking`; `SyncClient` has all 14 namespaces, where
      Python's has 13, D-027); the frozen legacy runtime-API table for archive reads; the port of
      `PYT/unit/test_facades.py` (namespace parity of the sync facade).
- **M6-04 optional source experiment.** Investigate a `[u8; 32]` decode visitor only for an observed client
  issue. Any comparison is bounded hosted work; no benchmark campaign is required.
- [ ] **M6-05** CLI (24): `root` list, claim, allocate, move, swap, register; `subnets` list, show, hyperparameters,
      burn-cost, create-cost, price, conviction; `conviction` list, targeting, show, add, mode, accept, move;
      `collateral` show, list, add, set-min; `query` complete; hidden v9 aliases of the groups this milestone first
      ships. `basket_index.py` is ported into `rtao-cli` as `root_helpers::basket_index`, and `basket_index_data.py` is
      generated into `rtao-cli` from extracted literal tuples, not into `rtao-catalog`, which every neuron build
      compiles.
- [ ] **M6-06** The four `test_*` keys of [inventory §2.5](docs/plan/parity-inventory.md#25-upstream-e2e-names) assigned
      to M6 (`test_subnets_all_batched`, `test_quote_stake_slippage_and_fee`, `test_metagraph_fast_path_matches_runtime`,
      `test_leases_read`) mapped to rtao e2e tests.

**Exit checks (hosted CI):**

1. Hosted read/model/namespace/blocking tests pass against committed fixtures; exact-hits/single-block tests cover
   implemented reads. Existing read oracle vectors replay successfully.
2. Parity/source/generated guards pass and no implemented read remains incorrectly listed as pending.
3. Missing fresh cross-language, live-chain or platform evidence is recorded. Those checks are optional targeted
   hosted work; no decoder benchmark campaign is required. C1-C7 apply.


### 12.9 M7: Signers, EVM, PoW GPU, timelock UX

**Goal.** Every signer and auxiliary surface of v11. **Depends on** M5 [I: EVM intents and CLI groups need it; signers
need only M2].
**Deliverables.** Reviewed client source, docs and applicable hosted checks: Ledger/Vault/extension signers,
injected Keychain tests, EVM and timelock helpers, GPU feature source and CLI commands. Replay existing
fixtures in Rust. Emulator/interop execution is optional hosted evidence; devices, app builds and
hardware/platform rehearsals are deferred.
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
      `PY/vault/server.py`, upgraded like the extension bridge of M7-04; UOS frames rendered as SVG QR codes with one
      automatically selected numeric/alphanumeric/byte segment, no segment splitting, error correction M and a
      4-module quiet zone (`optimize=0`, `PY/vault/qr.py:15-28`; binary UOS frames use byte mode), with a QR
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
- **M7-08 optional hosted integration.** Target scenario 14 (PoW registration) or 19 (timelocked commitment)
  only for a concrete client change, with a timeout. Local PoW mining, node deployment and long timing runs
  are outside development work.
- [ ] **M7-09** Maintain the core-patch source rules and injected/replay Ledger transport tests. Keep
      `with_transport` and `ApduTransport` confined to testkit; verify shipped crates against the unpatched core
      in relevant hosted checks. Building Ledger apps, starting Speculos and recording new transcripts are
      optional targeted hosted work, with pinned inputs and explicit limits.
- **M7-10 optional hosted integration.** A bounded manual emulator check may cover one changed Ledger
  request or signing/receipt path. No nightly device-model matrix, automatic re-recording, localnet transfers,
  scheduled live probe or bot setup is required. Report absent hardware and emulator evidence honestly.
- **M7-11 deferred by Q30.** Device operation, firmware/app verification and hardware rehearsals are future
  hardware acceptance work. Document the untested HID/device coverage; no local transfer or device run is required.

- [ ] **M7-12** The signer and Ledger guides in the book ([§11](#11-documentation-plan)), including a note that the app
      shows stake amounts typed `TaoBalance` as raw integers
      ([research J §4.6](docs/plan/research/J-ledger-ci-shim.md#46-upstream)), kept until a released app version
      formats them (the fix was filed with Zondax on 2026-10-13, Q22; once it ships, the note says instead that
      `swap_stake_limit` and `move_stake_limit` show their `limit_price`, a price ratio scaled by 1e9, with the TAO
      unit, unless the app's maintainers handle those two calls), and CONTRIBUTING's section on running the Ledger
      emulator lane through an optional bounded hosted dispatch (Q30).
- [ ] **M7-13** Security pass by a local cold reader ([§12.1](#121-overview)), in `docs/security/m7.md`; focus: Ledger,
      Vault and the extension bridge's token handling.

**Exit checks (hosted CI):**

1. Hosted signer tests pass using FakeLedger/injected transports and existing proof/request/APDU fixtures.
2. Vault framing/QR, extension bridge protocol, EVM and timelock tests pass against committed vectors and scripted
   loopback peers. GPU-feature source is checked only in relevant hosted profiles; no local mining runs.
3. Core-patch identity guards and relevant unpatched-core build checks pass in hosted CI.
4. `docs/security/m7.md` records findings and their resolution or explicit acceptance.
5. Hardware/HID, app/firmware, native platform and real Keychain evidence is deferred and documented. Emulator,
   live probes and cross-language/localnet checks are optional targeted hosted evidence. C1-C7 apply.


### 12.10 M8: CLI parity

**Goal.** Every command of the dumped Python command tree. **Depends on** M5, M6, M7.
**Deliverables.** Reviewed client source, docs and applicable hosted checks ([§12.1](#121-overview)); the remaining 45 commands, `PyJsonFormatter`, completions,
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

**Exit checks (hosted CI):**

1. Hosted CLI tree/alias/argument/JSON tests cover implemented commands and replay existing fixtures.
2. Strict source/catalog/parity checks pass for implemented client behavior. Record unexecuted fresh oracle,
   interoperability, native platform and device coverage; strict source completeness does not prove those runs.
3. CLI documentation and relevant hosted checks pass. No release, deployment or infrastructure campaign is
   required. C1-C7 apply.


### 12.11 M9: Client hardening

**Goal.** Review and test the implemented client. **Depends on** M8.
**Deliverables.** Reviewed source, current docs, an accurate filing inventory and a security review report.
Q30 defers release, deployment and automatic upkeep; this milestone does not claim 1.0 release readiness.

- [ ] **M9-01** Review existing fuzz targets and failure regressions as client source. Mainnet cloning, full-node
      operation and increased fuzz/benchmark campaigns are removed by Q30. Targeted robustness checks use bounded
      free hosted jobs.

- [ ] **M9-02** Book and `llms.txt` complete; generated reference and migration tables.
- **M9-03 deferred by Q30.** Automatic updates and release-provenance deployment depend on a future distribution
  task. Keep the feature disabled; do not prepare local release or updater rehearsals.

- [ ] **M9-04** Audit existing upstream PR/issue links, states and source evidence. Keep unfinished proposals local;
      opening every planned filing is no longer an exit requirement (Q30, C4).
- [ ] **M9-05** Review source/pin update guards in hosted CI and record a real pin update if one is needed. Waiting
      for an upstream release or rehearsing the oracle/localnet/recording pipeline is no longer required (Q30).

- [ ] **M9-06** The full security review: a cold-reader pass ([§12.1](#121-overview)) over the whole repository, covering
      signers, keyfiles, secrets handling, the nonce store, `auto-update` and every area of the per-milestone passes, in
      `docs/security/m9.md`; an external review in addition only if the user arranges one.
- **M9-07 deferred by Q30.** Bot installation, automatic upkeep, release publishing, branch-protection and merge
  queue setup are outside current client development. Existing upkeep remains disabled.

**Exit checks:**

1. Applicable hosted client tests, Clippy, supply, feature/MSRV and documentation checks pass.
2. Source/pin guards pass, with unexecuted interoperability coverage recorded (M9-05).
3. Existing filing links and states are accurate; unfinished proposals remain local (M9-04).
4. `docs/security/m9.md` records the review and disposition of each finding (M9-06).
5. Deferred release/upkeep/hardware work is identified without claiming it passed; C1-C7 apply.

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

Client source dependencies remain M0 -> M1 -> M2 -> M4 -> M5 -> M8 -> M9, with M3, M6 and M7 following their API
prerequisites. Logical independence does not authorize concurrent local builds (Q30).

The user's `sam0x17` GitHub access and free `ubuntu-latest` runners cover current development. GHCR namespace
`ghcr.io/sam0x17/rtao-oracle` is authorized if a targeted hosted check needs it. No testnet funds, subnet, soak host,
local node, Studio build host, device run, Pages environment, paid runner or bot App is required. Preserve the prepared
wallet and recovery material. Docs can be downloaded as CI artifacts; release assets and hardware/platform
acceptance are deferred. State missing native macOS, Keychain, ARM and hardware coverage. Do not ask again for
already waived infrastructure prerequisites.

### 12.14 First steps

M0's steps, from the current repository state (cargo-new skeleton, edition 2024, MIT, two commits) to the M0 exit, in
order. The headings group them for reading; the IDs set the order. **Parallel work.** M0-09…M0-13 (xtask) can run beside
M0-14…M0-17 (the seam); M0-21 and M0-24 need only M0-14.

**Prerequisites and PR-U17, relicensing, workspace and pins**

- [x] **M0-01** Recorded the user's GitHub/free-runner decisions and filed PR-U17 from the user's monorepo fork
      after source/prototype evidence existed (§15, §5.10). Q29/Q30 remove infrastructure, funding, deployment,
      device and build-host prerequisites. Do not repeat those setup questions or start a new local rehearsal.
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
- [x] **M0-06** `upstream.toml` with `[monorepo]`, `[core]` and `[parity]` at the M0 tag, the newest final monorepo
      release that finney runs and whose `:vS` localnet image is published ([§10.3](#103-a-pin-bump-step-by-step)
      step 1): `v478` / `850f2755…` on 2026-10-13, the commit this plan's evidence was checked at, whose `sdk/` equals
      `v477`'s. Also `toolchain = "1.89"`, `spec_version` = that tag's spec, `python_package_version = "11.3.1.dev0"`
      (the in-tree source version), `[networks.finney]` genesis, `[localnet]` image tags `pinned`, `mainnet`,
      `testnet`, `devnet` and `main` (digests written by `xtask pin --parity <tag> --check` once M0-09 exists; without
      `--parity`, `--check` only verifies), and the `[artifacts]` list.
- [x] **M0-07** `[workspace.dependencies] bittensor-core = { git = "https://github.com/RaoFoundation/subtensor.git",
      tag = "<core_tag>", default-features = false }` at the M0 tag (every monorepo git dependency is declared in this
      table and inherited with `workspace = true`, so a core bump rewrites one manifest), the ring-only `rustls`
      (`default-features = false`, features `ring`, `std`, `tls12`, `logging`), `serde_json` with `arbitrary_precision`
      ([§5.8](#58-known-core-defects-and-how-rtao-handles-them) #10), and the `[patch.crates-io] w3f-bls` line.
- [x] **M0-08** Skeletons of the other 18 crates (19 with the facade), each with one-line crate docs and the layering of
      [§4.2](#42-layers) in its manifest; `cargo build -p rtao-core` succeeds (for reference, the core alone at this tag
      locked 241 packages and built in 12.9 s wall, 84.6 s CPU, clean release:
      [bench/tagpin](docs/plan/bench/tagpin/tagpin.log)). `rtao-testkit` declares its four `[[test]]` targets (`e2e`,
      `interop`, `core_oracle`, `ledger_emu`) now, as empty files with `test = false` and the `required-features` of
      [architecture §11.1](docs/plan/architecture.md#111-layers), and the `ci` profile gains its filter: nextest rejects a
      `binary()` predicate that matches no target in the workspace (exit 96 with nextest 0.9.143, checked while
      planning), while a declared target satisfies it even when `test = false`, `required-features` or `-p` leave it
      unbuilt. Later milestones fill the files.

**xtask**

- [x] **M0-09** `xtask pin --check`: partial clone (`git clone --filter=blob:none`) cached under `target/upstream.git`;
      the core-input fingerprint over the core's path-dependency closure, written into `upstream.toml` as `fingerprint`;
      `--check` reads the `[localnet]` digests from GHCR. Without `--parity`, `--check` only verifies the committed
      pins against the root manifest and `Cargo.lock`, offline apart from git (the `pr-gates` step of M0-27, which
      gains the core-patch rules in M7-09).
- [x] **M0-10** `xtask import-upstream`: copy `[artifacts]` from `parity.commit`, record sha256; also vendor every
      extraction and chain-source input under `vendor/upstream/src/` and the watched-tree listing
      `vendor/upstream/tree.txt`; `--verify-only` for CI.
- [x] **M0-11** `xtask deps-check` over `cargo metadata` and `cargo tree`: rules 1-8 of [§4.2](#42-layers) and
      `--profile no-host` (both profiles, `cargo tree -e normal,build`).
- [x] **M0-12** `xtask extract` v0: the literal-subset extractor on `ruff_python_parser` (git, pinned rev; fall back to
      `rustpython-parser` if needed); its first run extracts the btauth/1 goldens of `PYT/unit/test_http_auth.py:19-36`
      (`FIXED_NONCE`, `BODY`, `METHOD`, `PATH`, `GOLDEN_SR25519_PAYLOAD`, `GOLDEN_ED25519_SIGNATURE`).
- [x] **M0-13** `xtask parity check`, file layer only, over `vendor/upstream/tree.txt`: seed `parity/map.toml` with all
      357 rows of [python-module-map.md](docs/plan/python-module-map.md) as `pending`; presence is checked for every
      row, blob equality for blessed rows; the license rule of M0-02 is checked too.

**The seam and its guards**

- [x] **M0-14** `rtao-core` re-exports (`keys`, `codec`, `runtime`, `extrinsic`, `digest`, `mlkem`, `timelock`),
      `CoreError` mapping, `DynValue`, `offload`, `BITTENSOR_CORE_TAG`/`BITTENSOR_CORE_COMMIT` generated from
      `upstream.toml`, features `host` and `ledger`; `tests/lock.rs`.
- [x] **M0-15** `clippy.toml` with the ten `disallowed-methods` entries, `allow-invalid = true` on the seven host-gated
      ones ([§5.4](#54-not-used-on-runtime-paths)).
- [x] **M0-16** `deny.toml` (license allowlist, advisories, SCALE-stack `multiple-versions = "deny"`, bans on
      `aws-lc-rs`, `aws-lc-sys` and `native-tls`).
- [x] **M0-17** `rtao-core-ext` skeleton (no tokio, monorepo deny lints) with `verify_strict` as its first module.
- [x] **M0-18** `xtask core-surface`: generate `crates/rtao-core/tests/core_surface.rs` for every re-exported core item
      and the ten `disallowed-methods` paths ([§5.4](#54-not-used-on-runtime-paths)).
- [x] **M0-19** `xtask staged --check`: parse the three `UPSTREAM-*` header lines, generate one probe crate per module
      (its own workspace, the patch line, a copy of `Cargo.lock`), and classify failures as in
      [§5.3](#53-staged-in-rtao-core-ext).
- [x] **M0-20** `xtask lint-upstream`: the monorepo's `subtensor-linting` (git dependency at `core_tag`, declared in
      the root `[workspace.dependencies]`, M0-07) over `rtao-core-ext` only.

**Primitives, JSON and goldens**

- [x] **M0-21** `rtao-primitives`: ids with checked `NetUidStorageIndex`, `AccountId` (ss58 through the core),
      `DefaultAccount`, wire amounts, `Balance<U>` (no `Add`/`Sub`, `try_sum`), `Money`, `Amount`, `ResolvedAmount`,
      `Spend`, `DynBalance`, `IntoCoreValue`, scale-decode visitors, `pyfmt`, `json::{from_slice, from_str}`;
      proptests, including the properties of the hypothesis suite `test_balance_properties.py` adapted to the
      chain-native domain (`RAO` 0..=u64::MAX, `NETUID` 0..=4095), with checked underflow/overflow and rejection of
      negative/oversized inputs, and `// pyt:` provenance (Q25 supersedes the full signed strategy bounds; D-035).
      The suite has no literal cases to extract; the `str(Balance)` oracle vectors arrive in M2.
- [x] **M0-22** The `serde_json/arbitrary_precision` decision of [§5.8](#58-known-core-defects-and-how-rtao-handles-them)
      #10 with its M0 tests: direct versus `Value`-first parsing of untagged and flatten floats, a u128/U256
      `to_corpus_json` round trip, and the oracle comparator's integer and float vectors.
- [x] **M0-23** `rtao-testkit::fixtures`: the `golden.json` and shape-corpus loaders; the replay tests `golden_calls`,
      `golden_storage_keys`, `golden_signature_payloads`, `golden_extrinsics`, `golden_multisig` and `golden_ss58`, one
      per `golden.json` section (`calls`, `storage_keys`, `signature_payloads`, `extrinsics`, `multisig`, `ss58`), through
      `IntoCoreValue` and `compose_call` by name (typed builders arrive with M1's regen); the shape-corpus replay test
      `corpus_replay`.

**btauth/1, doctor, CI and docs**

- [x] **M0-24** `rtao-signer`: `Signer`, `MultiSignature`, `KeypairSigner`, in M0 because `btauth::sign` takes a
      `&dyn Signer`.
- [x] **M0-25** `rtao-btauth` core with every vector and negative case.
- [x] **M0-26** `rtao-cli` with `rtao doctor` (lock check that prints the exact patch line); its `[[bin]] name = "rtao"`
      sets `doc = false` ([§11](#11-documentation-plan)).
- [x] **M0-27** `.github/workflows/pr.yml` with bounded free `ubuntu-latest` jobs for `check`, `no-host`, `msrv`,
      relevant `features` and `supply`, with actions pinned by SHA. Keep source/pin/extraction/parity/dependency
      checks, units, goldens, doctests and supply guards in hosted CI. `cargo xtask gates` is a hosted command;
      Q30 removes the full local run before every push. Cancel superseded runs and avoid unnecessary matrices.

- [x] **M0-28** README (approach, patch line, `rtao doctor`), CONTRIBUTING (xtask commands, bless workflow, the
      attribution rule of M0-02).
- [x] **M0-29** Record M0 client checks and hosted results. Version changes, release tagging and binary publication
      are deferred by Q30.

## 13. Risks

From [architecture §15.1](docs/plan/architecture.md#151-risks) (likelihood / impact are [I]), plus two risks found while
writing and reviewing this plan.

| # | Risk | L / I | Mitigation |
|---|---|---|---|
| R1 | `bittensor-core` is internal and unversioned; its API follows the Python binding | high / medium | one seam crate; `core_surface` probe names the broken symbol; optional hosted `core-canary`; `core_tag` moves rarely; upstream PRs |
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
| R17 | fixtures and localnet differ from mainnet | certain / medium | pinned metadata and source review; optional targeted hosted checks; no mainnet clone (Q30) |
| R18 | secrets: KDF threads, Keychain, nonce-store memory | low / high | `Zeroizing`/`secrecy`, no `Debug` on secrets, bounded in-memory store; per-milestone security passes by local cold readers (M2, M3, M4, M7) and the full review in M9 |
| R19 (new) | `serde_json/arbitrary_precision`, forced on by the core, breaks `#[serde(untagged)]`/`#[serde(flatten)]` parsing of non-integer numbers and makes `serde_json::Value` equality spelling-sensitive, in rtao and in subnet authors' own code | certain / medium [I] | decided in [§5.8](#58-known-core-defects-and-how-rtao-handles-them) #10: rtao enables the feature itself; `Value`-first parsing at every text entry point and in rtao's number-or-string types; `rtao::btauth::server::Json<T>` for subnet authors; oracle JSON compared with integers exact and non-integers as f64; an M0 test pins all of it |
| R20 (new) | upstream churn moves the parity target during development | certain / medium | upstream tracking as standing work from M2 ([§12.1](#121-overview)); pin bumps from M2, by the agent during the goal loop and by the bot after it ([§12.4](#124-m2-validator)); the porting report sizes each bump; auto-bless covers cosmetic edits; the M8 strict gate is evaluated at the `parity_tag` current at M8 |
| R21 (new) | the Ledger emulator lane is not the production path: Speculos never runs the core's HID transport or the binary Ledger Live installs; until PR-U17 ships, the rtao workspace, and with it the device-run build and the release binaries, uses the patched core, whose `open()` path differs from upstream only by the seam; and the lane's tooling moves (API levels, image tags, removed flags) | certain until PR-U17 / medium | PR-U17 filed in M0; `[core.patch] paths` bounds the difference, and `xtask pin --check` fails once the patch is obsolete ([§5.10](#510-the-pr-u17-core-patch)); `core-unpatched` proves that shipped crates build without it; hardware/HID coverage remains deferred and unverified under Q30; pins by digest and a source-built ELF checked by sha256 ([§9.5](#95-python-oracle-and-interop)) |
| R22 (new) | a runtime or Ledger app change breaks clear signing for every SDK: finney runtimes 456 and 457 shipped without the compiled metadata hash, and every Ledger transaction failed with `CannotLookup` ([subtensor#3159](https://github.com/RaoFoundation/subtensor/issues/3159)) | low / high for Ledger users | optional targeted hosted emulator/probe checks can supply evidence; no scheduled campaign or hardware acceptance is claimed ([§9.7](#97-ci-lanes-and-what-each-catches)); retain additional issue proposals locally |

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
| Q17 | Ledger in CI: a self-hosted hardware runner, or a manual release checklist? | "sure, though maybe there is some shim we can use, worth researching" | Original decision: manual device checklist plus CI-only emulator research. Q30 now defers device acceptance and makes emulator execution optional hosted evidence | Q30 defers manual hardware acceptance and makes costly emulator checks optional hosted evidence; no local device rehearsal or release gate |
| Q18 | Upstream engagement: file each upstream PR when its evidence exists, or all of them in M9? | "go ahead and file them once evidence exists, I assume you are talking about upstream fixes we found?" | yes, these are the upstream fixes and improvements found while planning: the confirmed core defects (PR-U2's `Sudo.Sudid` receipt, PR-U5's epoch predicate, PR-U7's shield era), the staged-code moves (PR-U1, U6, U9, U10, U15), the public `Policy` inputs (PR-U8), the `w3f-bls` manifest change (PR-U4), the catalog and metadata publishing improvements (PR-U3, U11, U12, U16), the encrypted keyfile vectors (PR-U13), the `SelectiveMetagraphIndex` request (PR-U14), the Ledger transport seam found while researching Q17 (PR-U17) and the Python-side issues. This records the original filing scope | current work audits existing filings and keeps additional proposals local ([Executing this plan](#executing-this-plan), rule 7); the one item found later outside it, for the Ledger app, was filed with Zondax on 2026-10-13 at the user's request (Q22); [§15](#15-upstream-contributions) names each one's evidence step and records its link (common exit check C4); M9-04 audits them |
| Q19 | Security passes: M2, M3, M4 and M7 each end with a security pass by a reviewer other than the author. Who performs them? | "we will use local cold readers" | local cold readers: fresh-context reviewers given only the repository, the milestone diff and a security checklist | the procedure is in [§12.1](#121-overview); M2-26, M3-11, M4-19 and M7-13; M9-06 applies it to the whole repository |
| — | Time estimates (an instruction, not a numbered question) | "don't bake time into the plan, I'm going to have codex execute it as a goal step by step, but I wouldn't commit to specific days being specific things, AI is notoriously bad at estimating its own completion times either way" | no time estimates: no durations, effort figures, milestone dates or staffing calendars; CI cadences and upstream release-cadence facts stay | [Executing this plan](#executing-this-plan); [§1.4](#14-delivery-order); step IDs and numbered exit checks in [§12](#12-milestones); [§12.13](#1213-dependency-order-and-external-prerequisites); [§12.14](#1214-first-steps) |
| Q20 | Ledger emulator lane until PR-U17 ships: plan v2 had "a test-only copy of the core's Ledger message code" in `rtao-testkit` | "ok let's just add to the plan that we will cargo patch bittensor-core with that support until it merges" | until a `core_tag` contains PR-U17, the rtao workspace builds `bittensor-core` through a Cargo `[patch]` that adds exactly PR-U17 on top of `core_tag`; the test-only transcription survives only as a fallback if upstream declines PR-U17 | [§5.10](#510-the-pr-u17-core-patch): M0-01 files PR-U17 from the user's fork; M7-09 adds the patch branch `rtao-core-patch/<core_tag>`, the two root `[patch]` tables, `[core.patch]`, `xtask core-patch`, the core-patch rules of `xtask pin --check`, the clippy entries that keep PR-U17's items in `rtao-testkit::ledger_emu` and the `core-unpatched` job (the hosted M7 core-patch check); the emulator lane and `ledger_replay` run the core's own `ledger.rs` from their first run; subnet builds never see the patch (Cargo ignores `[patch]` in dependencies); a pin bump rebases the patch, or drops it once a tag contains PR-U17; upstream declining PR-U17 is a stop-and-ask point (rule 6) |
| Q21 | Merging: the v2 summary said "Codex may merge its own PRs once the required checks pass" and asked whether every merge should need the user's approval | "we are just going to develop all of this in one goal loop, no PRs, just let codex commit until it is done" | M0 to M9 run as one continuous goal loop: no PRs in the rtao repository; the agent commits directly to `main` and pushes, without asking, until M9 exits | Q30 replaces local full prepush gates with hosted acceptance and defers milestone tags and automatic-upkeep handover. Direct client-development commits remain authorized |
| Q22 | Ledger app display: the core patch cannot change how the Ledger app shows amounts, and the app shows stake amounts typed `TaoBalance` as raw integers ([research J §4.6](docs/plan/research/J-ledger-ci-shim.md#46-upstream)); plan v2 left filing with `Zondax/ledger-polkadot` to the user's approval | "for zondax we should just submit a PR or issue"; later the same day: "yeah go ahead and open it now, ideally with a PR fixing it if it is an easy fix" | filed on 2026-10-13, before the plan runs: issue [Zondax/ledger-polkadot#295](https://github.com/Zondax/ledger-polkadot/issues/295), PR [#296](https://github.com/Zondax/ledger-polkadot/pull/296), which adds `TaoBalance` to the app's balance type names; `AlphaBalance` stays unformatted (a per-subnet token, never TAO) | [Executing this plan](#executing-this-plan): rule 6 no longer names the item (any other filing outside the monorepo stays stop-and-ask), and rule 7 pre-authorizes its follow-up (answering maintainer questions, pushing the changes they ask for to the PR branch); C4 counts it as filed; the [§15](#15-upstream-contributions) row; M7-12's guide note stays until a released app version formats the amounts; M9-04 audits it |
| Q23 | External prerequisites and CI/docs choices, 2026-10-14 | "I can provide testnet tao to whatever wallet address you need; just use my github; we will skip sophisticated github runners, I want to just use ubuntu-latest free ones; I don't see why we need github pages, that seems overkill; I have a ledger we can use" | use `sam0x17` for GitHub/GHCR; hosted CI is free `ubuntu-latest` Linux x64 only; downloadable rustdoc/book release assets replace Pages; testnet funding is offered and a Ledger is available | Q30 removes local Mac builds, hardware/Keychain rehearsals and clone/soak infrastructure prerequisites. Native macOS/ARM/hardware coverage is reported as unverified; release publishing is deferred |
| Q24 | Preserve u64 chain balances and Python's arbitrary signed balances? | "Preserve both with signed DynBalance (Recommended)" | `DynBalance` stores an arbitrary signed integer; typed and wire balances use checked u64 | M0-21 retains the full Python strategy bounds through ±2^80 rao and checks range before chain encoding; architecture §5.2 |
| Q25 | Adopt the chain's native balance behavior instead? | "actually imo it is cleaner if we just adopt the chain's native balance behavior, what do you think?"; "let's do it"; "yes" | Supersedes Q24: all balances use u64 rao, with distinct TAO/alpha units and checked arithmetic | Valid-chain display/JSON parity remains; negative and arbitrary-size Python local balance arithmetic is excluded as D-035. M0-21 properties use native bounds and explicit rejection/overflow/underflow tests; M2 formatting vectors retain 10,000 cases over the native domain |
| Q26 | BLS exit criterion, 2026-10-14 | "Approve the built-dependency check" (both explicit BLS questions) | Require exactly one built BLS implementation at the core pin; allow only the recorded dormant registry 0.1.9 lock entry and fail on its activation | Supersedes the literal raw-lock count in M0 exit 5, M0-09/M0-11/M0-14, §5.6 and architecture §1.3; source, commit, version, unexpected-entry and all-profile/all-target checks remain mandatory |
| Q27 | Dormant supply advisory, 2026-10-14 | "Allow guarded dormant exception (Recommended)" | Permit only RUSTSEC-2026-0253 for the exact inactive lru 0.16.4 registry package; retain the unfiltered audit and reject activation or unexpected affected entries | Guard the all-feature/all-target workspace, fuzz and shipped profiles before applying the exception; all other advisories, tool failures, licenses, sources and bans remain blocking |
| Q28 | Dormant ruzstd advisory, 2026-10-14 | "Allow guarded dormant exception" (explicit asynchronous reply) | Permit only RUSTSEC-2026-0344 for ruzstd 0.8.3 from crates.io with checksum a7c1c839d570d835527c9a5e4db7cb2198683a988cb9d7293fc8674e6bd58fc8, while absent from all checked graphs; retain the original nonzero raw audits and require patched range >=0.9.1 | Exact Q27/Q28 audit and activation guards remain applicable in hosted CI. Q30 removes the full local matrix before commit or push |
| Q29 | M2 testnet soak, 2026-10-14 | "we can probably skip a testnet soak, let's just make sure all tests are passing"; "even OTF/RaoFoundation themselves don't do such things" | Waive the 24-hour testnet soak, M2-24 and its dedicated funding/subnet/host prerequisites | M2-24 records the waiver without a passing soak. Q30 makes costly container/interop checks optional hosted evidence and defers releases. Preserve prepared wallet/recovery state |
| Q30 | Local resource use and scope, 2026-10-14 | "stick to development tasks"; "running a bunch of tests in github CI should be sufficient"; "remove anything from the plan that would be costly locally" | Substantial validation belongs on free hosted CI; prohibit local nodes, localnets, containers, soaks, duplicate builds/toolchains, hardware/Keychain rehearsals, fuzz/benchmark and release campaigns | Current execution scope and C1-C7 supersede conflicting rules. Remove mainnet cloning; defer release/platform/device acceptance and automatic upkeep. Preserve source and durable state, clean disposable data, and report actual coverage |

### 14.2 Resolved by measurement or with upstream

From [architecture §15.2](docs/plan/architecture.md#152-decisions-and-open-questions):

| # | Question | Resolved by | Milestone |
|---|---|---|---|
| Q6 | real public-endpoint quotas, to tune the default token bucket | measurement | M1 [I] |
| Q7 | will RaoFoundation accept PR-U1…U17, especially U4, U5, U6? | upstream; audit existing filings and retain additional proposals locally ([§15](#15-upstream-contributions)) | from M0 (PR-U17) |
| Q8 | does a crates.io `w3f-bls 0.1.3` std build produce ciphertexts pallet-drand decrypts? | `ci/pr-u4-probe/` (scenario 16); the four-file source diff predicts yes | M2 |
| Q9 | should upstream account for `MaxEpochsPerBlock` deferral in `get_next_epoch_start_block` and the reveal predictor? | retained local upstream question with M2-17 evidence ([§15](#15-upstream-contributions)) | M2 |
| Q10 | is there a root setter that forces the queued `register_network` path on localnet (`SubnetLimit`)? | **resolved from source**: root `sudo_set_network_immunity_period(0)` (AdminUtils call 35) then `sudo_set_subnet_limit(<current non-root count>)` (call 37) make the next `register_network` prune the lowest-priced subnet and queue (`UP/pallets/admin-utils/src/lib.rs:1215-1228, 1250-1258`; `UP/pallets/subtensor/src/subnets/subnet.rs:204-213, 233-260`; `coinbase/root.rs:358-361`); scenario 10 runs it in the `global` container ([§9.4](#94-localnet-e2e)) | M4 |
| Q11 | when will the runtime or the SDKs move to extrinsic v5? | drift alert | continuous |
| Q12 | will `SelectiveMetagraphIndex` be exposed through metadata or a runtime API? | upstream (PR-U14, filed at M1-03) | M1 |
| Q13 | will `:v<spec>` localnet images keep appearing for every finalized release? | **resolved**: `docker-localnet.yml` publishes `:v<spec>` on every mainnet mirror move (`UP/.github/workflows/docker-localnet.yml:7-9`); `xtask pin --check` waits for the image, and `wasm-override` is a manual fallback | — |
| Q14 | is `Metagraph.total_stake` intentionally declared `TaoBalance` while carrying stake weight? | upstream question (D-019, filed at M1-16) | M1 |
| Q15 | does `subxt` 0.51 build with `default-features = false` over a custom `RpcClientT` without jsonrpsee? | build | M7 |
| Q16 | is tokio-tungstenite 0.30's handshake error API identical to 0.28's? | `FakeNode` test | M1 |

### 14.3 Gaps found while planning

Historical planning discussion. Infrastructure solutions recorded below were subsequently removed or made optional
by Q30; current milestone steps state the active client requirements.

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

Historical review notes, retained as evidence. The retired workflow specimen and infrastructure schedules below
are not instructions to restore them; Q30 and the revised milestone steps describe current work.

The completeness review that followed the six-lens review found these and decided each; the decision lives in the
section named, and architecture.md carries the same decision.

| # | Gap | Decided in |
|---|---|---|
| 15 | `NeuronCx` and the `Validator` trait (M2) hold a `MetagraphHandle` and a `SignedClient` and take a `MetagraphDiff`, which were scheduled in M3 | M2 ([§12.4](#124-m2-validator), [§12.1](#121-overview)) |
| 16 | localnet scenario 8 (runtime upgrade mid-session) had no milestone | M2, its offline-payload assertion in M5 ([§9.4](#94-localnet-e2e)) |
| 17 | Historical: the `pin-bump`, `oracle-refresh`, `upstream-next`, `core-canary`, `live-probe`, `features-powerset`, `release-plz` and `release` workflows, `xtask compat` and the run-time compatibility guard had no milestone, although upstream tracking starts in M2 | M1 and M2 ([§12.3](#123-m1-read-path-and-the-first-typed-read), [§12.4](#124-m2-validator)); M2 exits with a reproducible re-pin at the pinned tag |
| 18 | `e2e_map.toml` had no state for a key whose rtao test does not exist yet, and 15 of the 39 upstream `test_*` names had no milestone | `rtao`/`na`/`pending` states ([§9.4](#94-localnet-e2e)); [inventory §2.5](docs/plan/parity-inventory.md#25-upstream-e2e-names) |
| 19 | the Vault signer's loopback WebSocket page server fell outside dependency rule 2, and no crate rendered its QR frames | [§4.2](#42-layers) rule 2; M7 ([§12.9](#129-m7-signers-evm-pow-gpu-timelock-ux)) |
| 20 | `cargo doc --workspace` lets the `rtao` binary's docs overwrite the facade's in the docs archive | `doc = false` ([§11](#11-documentation-plan)) |
| 21 | `rtao wallet save-password-env`, the three profile examples, `rtao_metagraph::persist`, `auto-update` and `lint-upstream` had no milestone | M4; M2, M3 and M4; M3; M9; M0 ([§12](#12-milestones)) |
| 22 | no rtao CI job generated the template and tested it against the PR's commit (the template's own CI would build the last tag), and template-only PRs skipped the localnet | ordinary hosted template source checks; optional integration is no longer part of required development acceptance (M3, [§12.5](#125-m3-miner); [§9.7](#97-ci-lanes-and-what-each-catches)) |
| 23 | the completeness layer checked only the root `__all__`, not the 14 other public `__all__` lists (`evm` alone exports 38 names) | [§8.3](#83-how-parity-is-enforced) completeness row and `parity/exports_extra.toml` (M2) |
| 24 | the 24-hour testnet soak had no host, although GitHub-hosted jobs stop after 6 hours | originally a long-lived host prerequisite; superseded by Q29's explicit soak waiver ([§12.4](#124-m2-validator), [§12.13](#1213-dependency-order-and-external-prerequisites)) |
| 25 | nothing compared the `--json` output of the 163 hand-written commands with btcli's, although JSON is the CLI's parity contract (`--json` is global); lanes 5 and 8 cover only reads, `tools` and `tx` plans | interop lane 9 through `PinProxy` (M4, extended per milestone; [§9.5](#95-python-oracle-and-interop)); M8 exit |
| 26 | python-module-map.md marked only 11 Python unit-test files `ported`, while architecture §11.2 ports `test_lock_ownership_gate` and the M7 exit requires ports of `test_ledger_signer`, `test_extension_bridge`, `test_extension_signer` and `test_evm`; five behavior tests the oracle cannot reach (PoW challenge, owner pruning, name ordering, facades, commitments accounting) were `oracle-only` | 21 files `ported`, each in the milestone of its code ([§9.2](#92-upstream-fixtures-reused)) |
| 27 | architecture Appendix E's `nightly.yml` did not parse as YAML (a `: ` inside the plain `run:` scalar of the `core-canary` issue step), so GitHub would reject the whole nightly workflow: `upstream-next`, `core-canary`, `fuzz`, `bench` and `features-powerset` | a block scalar; every workflow block of Appendix E now parses ([architecture Appendix E](docs/plan/architecture.md#appendix-e-ci-workflows)) |

## 15. Upstream contributions

None of these blocks rtao; each staged module is deleted when its PR ships in `core_tag`
([architecture Appendix D](docs/plan/architecture.md#appendix-d-upstream-contribution-plan)). Q18 records the original
filing authorization. Current work audits the existing filings and retains additional proposals locally. "Evidence
at step" identifies the source/test work supporting a proposal, without requiring a new submission. Existing links
and reviewed evidence remain accurate under C4 ([§12.1](#121-overview)).

| PR | What | Why | Evidence | Evidence at step | Filed |
|---|---|---|---|---|---|
| PR-U1 | public sans-IO protocol functions in a new ungated module `sdk/bittensor-core/src/protocol/{metadata,storage,runtime_api,extrinsic_lookup,dispatch_error,fee}.rs`, which the blocking client also calls (metadata fetch, storage decode, runtime-API encoding, extrinsic lookup, outcome and dispatch-error decode), plus the `TransactionPaymentApi_query_info` probe | private today; fee estimation uses deprecated `payment_queryInfo` | `CORE/client.rs:469-499, 551-576, 942-1048, 1234-1365` | M2-06 (with `metadata`, `storage` and `runtime_api` from M1-02) | [subtensor#3246](https://github.com/RaoFoundation/subtensor/pull/3246), open; filed 2026-10-14 |
| PR-U2 | Python-parity receipts in an ungated `protocol/receipt.rs` (`Sudo.SudoAsDone`, `Utility.BatchInterrupted`/`ItemFailed`, MevShield failures, fee fallback, and `Sudo.Sudid`'s `sudo_result` field) | the core misses nested failures, and reports a failed `Sudo.sudo` as success | `CORE/client.rs:1024-1044` vs `PY/_transport/utils/receipt.py:8-16`; the `Sudid` defect ([§5.8](#58-known-core-defects-and-how-rtao-handles-them) #4); a `FakeChain` receipt test with a failed `Sudid` (M2) and scenario 18's Sudo-wrapped ops (M5) | M2-06; scenario 18 (M5-06) adds live evidence to the open PR | [subtensor#3247](https://github.com/RaoFoundation/subtensor/pull/3247), open; filed 2026-10-14 |
| PR-U3 | export custom transaction codes, invalid-transaction needles, dispatch errors, explanations and help overrides in `errors.json` | removes extraction from `error_map.py`/`result.py` | `PY/error_map.py:488, 524, 555`; `PY/result.py:116-287` | M2-03 | — |
| PR-U4 | `w3f-bls = { version = "=0.1.3", features = ["std"] }` in the core manifest | removes every downstream patch line | `UP/sdk/bittensor-core/Cargo.toml:62`; E0277 without the patch; `ci/pr-u4-probe/` (scenario 16): offline cross-decryption and a localnet reveal; the four-file diff between crates.io `w3f-bls` 0.1.3 and `vendor/w3f-bls` | M2-20 (scenario 16) | — |
| **PR-U5** | `should_run_epoch` uses `> tempo` and the simulator caps the counter at `tempo + 1`; `generate_commit_v2_at(now, …)` | **confirmed bug**: commits can be lost silently after an `EpochSkipped` slot or a tempo lowered below the step counter, for the core's and Python's users | [§5.8](#58-known-core-defects-and-how-rtao-handles-them) #2, #3 | M2-11 | [subtensor#3248](https://github.com/RaoFoundation/subtensor/pull/3248), open; filed 2026-10-14 |
| PR-U6 | split `host` into `keyfiles-json`, `keyfiles-crypto`, `drand-http`, `rpc-client`, `parallel`, `pow` | neurons need only the pure JSON codec | `CORE/lib.rs:10-31`; `CORE/keyfiles/mod.rs:179-370` | M2-02 | — |
| **PR-U7** | `submit_shielded` signs carrier and inner with era ≤ 8 and resolves the inner receipt | **confirmed bug**: the runtime rejects the core's era-64 carrier as `Stale`; fixes the core's own users and retires nothing in rtao, which never calls it | [§5.8](#58-known-core-defects-and-how-rtao-handles-them) #1 | M4-15 (scenario 11) | — |
| PR-U8 | public `PolicySubject`/`PolicyInput` | any intent layer can reuse `Policy::check`; retires nothing in rtao, whose equivalence test already builds the core with `host` | `CORE/transaction.rs:55-90, 929-976` | M2-08 | — |
| PR-U9 | `Runtime::metadata_ir()` shared by the Python binding and rtao, with storage hashers, key idents, event fields and runtime-API signatures | the IR exists only in the PyO3 binding | `UP/sdk/bittensor-core-py/src/runtime.rs:554, 579` | M1-02 | [subtensor#3249](https://github.com/RaoFoundation/subtensor/pull/3249), open; filed 2026-10-14 |
| PR-U10 | pure `encode_password_for_environment`; public `keys::verify_strict` | `set_var` is unsound with threads; strict verification is private | `CORE/keyfiles/mod.rs:126-138`; `CORE/keys/mod.rs:78-100` | M2-02 (`keyfile_env`; `verify_strict` exists from M0-17) | [subtensor#3250](https://github.com/RaoFoundation/subtensor/pull/3250), open; filed 2026-10-14 |
| PR-U11 | `default` values and `mev_shield_*`/`all_amount_fields` class variables in `intents.json` | the catalog carries no defaults today | 0 catalog properties carry `default` | M2-03 | — |
| PR-U12 | publish V15 and V16 metadata as release assets | the drift job can skip live fetches | — | M1-21 | [subtensor#3245](https://github.com/RaoFoundation/subtensor/issues/3245), asset-contract request |
| PR-U13 | committed encrypted keyfile vectors (NaCl, Fernet, Ansible) | upstream has none | interop-recorded cross vectors | M3-08 | — |
| PR-U14 | expose `SelectiveMetagraphIndex` through metadata or a runtime API (issue) | removes `syn` extraction from pallet source | `pallets/subtensor/src/rpc_info/metagraph.rs` | M1-03 | [subtensor#3244](https://github.com/RaoFoundation/subtensor/issues/3244) |
| PR-U15 | move `Value` navigation helpers out of host-gated `client.rs` | no-host consumers need them | `CORE/client.rs:1515-1570` | M1-02 | [RaoFoundation/subtensor#3243](https://github.com/RaoFoundation/subtensor/pull/3243), filed 2026-10-14 |
| PR-U16 | `generate.py` `read_targets` (`:405-480`) scans each read's module-local helpers and module-level `Item(...)`/`Method(...)` descriptors | the catalog `hits` under-report 12 reads; `parity/read_hits_extra.toml` would shrink to the out-of-module items | [inventory §5.15](docs/plan/parity-inventory.md#515-hits-the-catalog-omits) | M2-09 | — |
| PR-U17 | an `ApduTransport: Send + Sync` trait in `sdk/bittensor-core/src/signers/mod.rs`, outside the `ledger` gate, implemented by the HID transport, and `LedgerDevice::with_transport(transport: impl ApduTransport + 'static) -> Self`; `open()` unchanged and `LedgerDevice` still `Send + Sync`; optionally a std-only TCP transport for Speculos; prototyped while planning, 108 added lines including three unit tests ([research J §8](docs/plan/research/J-ledger-ci-shim.md#8-core-patch-prototype)); filed from the user's fork, whose `rtao-core-patch/<core_tag>` branches carry it into rtao's workspace until it ships ([§5.10](#510-the-pr-u17-core-patch)) | the core's Ledger transport is a private concrete HID type, so neither rtao nor upstream can run the Ledger path against Speculos, Ledger's emulator (G16) | `CORE/signers/mod.rs:6-9`, `CORE/signers/ledger.rs:51-65, 158-219`; the emulator runs of [research J](docs/plan/research/J-ledger-ci-shim.md), the last through the prototype | M0 (the evidence predates the plan); M7-09's transcripts add to the open PR | [RaoFoundation/subtensor#3242](https://github.com/RaoFoundation/subtensor/pull/3242), filed 2026-10-14 |

PR-U1 and PR-U2 target an ungated `protocol` module because `staged --check` probes without `host`: a symbol that lands
only behind `host` retires no staged module that the no-host profile uses. PR-U17 retires the core patch of
[§5.10](#510-the-pr-u17-core-patch) rather than a staged module; its evidence predates M0, so it is filed during M0
(C4).

Existing issues and retained local proposals (Python package, docs, scripts and runtime):

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
| Ed25519 signing-object erasure, including expanded scalar and nonce prefix | M2-26 | [subtensor#3251](https://github.com/RaoFoundation/subtensor/issues/3251); exact v478 Zebra 4.1.0 provenance and current/MSRV proof corrected 2026-10-14 |
| malformed secret URI diagnostics disclose recovery phrase and password (D-036) | M2-26 | [subtensor#3252](https://github.com/RaoFoundation/subtensor/issues/3252); actual-constructor reproduction and tested-boundary mitigation scope corrected 2026-10-14 |
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
| a uhid virtual-HID bridge that runs the core's unmodified HID path against the Ledger emulator | unverified on GitHub-hosted runners (it needs the `uhid` module and root), impossible on Docker Desktop, Linux-only; it adds only the HID framing that the device run covers on real hardware | deferred; any future transport redesign needs separate scope (rule 6, [§5.10](#510-the-pr-u17-core-patch)) |
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
| `e2e-required` | legacy optional e2e workflow summary; not a required development check under Q30 |
| goal loop | client development from M0 onward with direct commits and applicable hosted checks; full local matrices and milestone release campaigns are removed (Q21/Q30) |
| `RTAO_UPKEEP` | repository variable for deferred automatic-upkeep jobs; remains off during current client development (Q30) |
| btauth/1 | v11's signed-HTTP protocol between neurons (eight-line payload, `X-Bittensor-*` headers) |
| Speculos | Ledger's device emulator (LGPL-3.0; a Python program with QEMU, in a Docker image), which runs the Polkadot generic app headless, takes APDUs over TCP, and exposes buttons, touch and screen events through a REST API |
| `ledger-emu` lane | optional targeted hosted Speculos check; existing transcripts replay in ordinary Rust CI; no scheduled or local emulator campaign |
| `LedgerBackend` | `rtao-signer`'s device seam under `LedgerSigner`: the core's `LedgerDevice` in production, fakes and emulator backends in tests |
| step ID | the stable ID of a milestone step, `M<k>-<nn>` (e.g. `M2-07`); steps run in ID order ([Executing this plan](#executing-this-plan)) |
| exit check | active numbered client/source/hosted check that passes before a development milestone exits; optional infrastructure evidence and deferred distribution/device work are recorded separately under C1-C7 |
| cold reader | a fresh-context source reviewer, with no independent local build/test campaign, that took no part in the implementation and receives only the repository, the milestone diff and the security checklist ([§12.1](#121-overview)) |
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
| 22 | the Vault signer runs a loopback HTTP and WebSocket server; `qrcode` selects one numeric/alphanumeric/byte segment with `optimize=0` (binary UOS frames use byte mode), error correction M and border 4; `bittensor.vault` exports `VaultSessionServer` and `scan_vault_address` | `PY/vault/server.py:26, 69-80, 181-242`; `PY/vault/qr.py:15-28`; `PY/vault/__init__.py`; qrcode 8.0 `main.QRCode.add_data` and `util.optimal_mode` | corrected 2026-10-15 |
| 23 | `cargo doc --workspace` with a library `rtao` and a binary `rtao` in another package warns "output filename collision" and writes both to `target/doc/rtao/`; `doc = false` on the binary removes it | a two-crate workspace, cargo 1.99.0 | reproduced |
| 24 | `frame_system`'s `can_set_code` refuses a `spec_version` that does not increase | polkadot-sdk fork checkout `cacb431`, `substrate/frame/system/src/lib.rs:2320-2349` | confirmed |
| 25 | 15 of the 39 upstream `test_*` e2e names were in no plan document; architecture §9.1's `NeuronCx` holds `metagraph: MetagraphHandle` and `http: SignedClient` | `UP/sdk/bittensor-core/tests/e2e-manifest.json`; `e2e.rs:288, 440, 476, 542, 1159-1330` | confirmed |
| 26 | the Python package has no `__main__` module, so `python -m bittensor` does not run | `find UP/sdk/python -name __main__.py` finds only `codegen/__main__.py` | confirmed |
| 27 | 19 Python modules declare `__all__`; besides the root, 14 are public and outside `_transport/` and `cli/` (`evm` 38 names, `intents` 97, `reads` 21, `extension` 16, `vault` 5, …) | `grep -l __all__` over `PY/`; `ast.literal_eval` of each list | confirmed |
| 28 | the core's Ledger transport cannot be swapped: `mod hid` is private, `LedgerDevice` holds the concrete HID `Transport` and `open()` is its only constructor; APDU building is private | `CORE/signers/mod.rs:6-9`; `CORE/signers/ledger.rs:51-65, 158-219`; `CORE/signers/hid.rs:41-72` | confirmed (G16) |
| 29 | Python's `LedgerSigner` takes `spec_name` from the runtime version and decimals and symbol from its arguments (9, `"TAO"`), and sends the unhashed payload parts | `PY/_transport/contract.py:60`; `PY/ledger.py:37-41, 115-163` | confirmed; architecture §3.9.2 now does the same |
| 30 | a runtime built without the compiled metadata hash rejects a `CheckMetadataHash`-Enabled extrinsic with `UnknownTransaction::CannotLookup` | polkadot-sdk fork checkout `cacb431`, `substrate/frame/metadata-hash-extension/src/lib.rs:150-155` | confirmed by code reading; matches subtensor#3159 |

## 19. Progress log

Historical execution record. Earlier local-build, infrastructure and release instructions in dated entries are
superseded by Q30; they are not current work requirements.

Append-only, newest last ([Executing this plan](#executing-this-plan), rule 4). One line per event: a milestone exit
with its hosted acceptance record, a deviation from a step's text (what and why), a decision taken under rule 5, an upstream filing
with its link, a wait on a prerequisite or on the user.

| Date | Step or milestone | Event |
|---|---|---|
| 2026-10-14 | M0-21 | Resolved §5.2's u64 `DynBalance` contradiction with the full signed Python property suite under execution rule 5: `DynBalance` retains arbitrary signed integers, while typed and chain wire balances remain checked u64. Full `RAO`/`SIGNED_RAO` strategy bounds are preserved; conversion enforces chain limits. This extends the Python-shaped API to satisfy the user's full-parity decision without changing chain bytes. |
| 2026-10-14 | M0-09 / M0-11 / M0-14 | The literal single-`w3f-bls` lockfile requirement fails: `Cargo.lock` holds pinned `0.1.3` and dormant registry `0.1.9` from `sp-core`'s optional `bls-experimental`. Rooted `cargo tree --workspace --all-features --target all -e normal,build -i w3f-bls@0.1.9` has no dependents; every built profile uses pinned `0.1.3`. Requested the user's decision under execution rule 2; existing raw-lock checks remain failing until answered. Continuing independent work. |
| 2026-10-14 | M0-12 | Selected the plan's RustPython parser fallback (`rustpython-parser` 0.4.0, Rust-only). Ruff at `1ae6552a9432d759218db172c22a007eddfcb40b` requires `unicode-normalization ^0.1.23`, while `sp-core`'s `parity-bip39 2.0.1` requires exactly `0.1.22`; Cargo cannot resolve both semver-compatible requirements. No core pin or upstream crypto dependency was changed to accommodate tooling. |
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
| 2026-10-14 | M0-21 / Q24 | User explicitly approved "Preserve both with signed DynBalance (Recommended)": signed arbitrary-size Python arithmetic and checked u64 typed/wire balances. The M0 property suite retains its original full bounds. |
| 2026-10-14 | M0-16 | `cargo deny` identified absent license fields in pinned `procedural-fork` and `subtensor-linting`. Added version-scoped Apache-2.0 clarifications against their actual monorepo `../../LICENSE` (cargo-deny hash `0x1ff0b2a9`, SHA256 `ce46a3c2599448e6989685edf7a58cdc5c22aa6dffbb24d4d2386dd0469a84e6`); procedural-fork's inherited Parity source headers also identify Apache-2.0. The license gate passes without exclusions. |
| 2026-10-14 | M0-27 | `cargo-hack --no-dev-deps` removes dev dependencies and requires pruning dev-only lock entries, conflicting with the prescribed `--locked` invocation. Under execution rule 5, run cargo-hack only in a task-scoped workspace snapshot, prune its lock offline, then reject any retained package with a changed name/version/source/checksum or any new package. The original manifests/lock remain intact and the exact locked feature checks are preserved. |
| 2026-10-14 | M0-22 | Regression tests show plain Value-first deserialization still fails untagged `1e-05` under `serde_json/arbitrary_precision`. The Rust JSON adapter preserves exact ordinary number tokens and adapts Serde's content-buffering hook for nested untagged/flattened floats, requiring serde >=1.0.228. Serde's buffered content represents decimal scalars as f64, so exact money/ratio inputs remain ordinary custom fields; wide integers and chain corpus JSON stay exact. The original required float and integer regressions remain enforced. |
| 2026-10-14 | M1-01 / M1-02 | Preparing independent extraction and portable core-extension work while the M0 literal BLS exit decision is pending, as execution rules 3 and 6 allow. M0 remains incomplete; no later milestone tag or checkbox is inferred from this preparation. |
| 2026-10-14 | M1-01 | The prescribed `extract_check_covers_named_tables` test names tables scheduled for later extraction as well as settings/RPC. Following execution rule 5's minimum-unblocking rule, implement the extraction those named tables require now, preserving later catalog and behavior steps and refusing skipped/pending tables as passing coverage. |
| 2026-10-14 | M0-21 / Q25 | User reconsidered signed balances and explicitly approved the native representation: "let's do it" and "yes" after recommending u64 rao, distinct TAO/alpha units and checked arithmetic. Q25 supersedes Q24. `DynBalance` follows the chain range; signed/arbitrary-size Python local arithmetic is excluded as D-035, and the property suite adapts to native bounds with negative/oversized rejection and underflow/overflow tests. Valid-chain formatting retains the required 10,000 native-domain oracle cases. No upstream defect is alleged by this API choice. |
| 2026-10-14 | M0-25 | Python authentication accepts signed/arbitrary decimal nonce text and checks receiver/freshness before cryptography. The planned u64 Caller/store could not represent genuinely fresh `-1` at Unix zero or `u64::MAX+1` near that boundary. Under rule 5, widen authenticated nonce/replay keys, preserve checked u64 signing clocks, and normalize Unicode decimal nonce text using maintained Unicode data. This HTTP protocol type is separate from Q25's native chain balances; no unapproved auth-parity divergence is introduced. |
| 2026-10-14 | M1-01 | `SetHyperparameter.wraps` is a derived comprehension over extracted `OWNER_HYPERPARAMETERS`, while architecture §4.9 excludes derived tables from extraction. Use a bounded Rust derivation with exact source-AST validation, Python insertion-order deduplication, source/table provenance and mutation tests. This preserves full named class-variable coverage without adding arbitrary comprehension evaluation. |
| 2026-10-14 | M1-05…M1-09 | Preparing the socket transport and fakes against extracted RPC constants while M0's BLS decision remains pending. Work remains subject to all M1 transport, TLS and source-test requirements; no milestone completion or tag is inferred. |
| 2026-10-14 | M1-06…M1-09 | `subxt-rpcs` 0.51.1 requires `native` or `web` even with defaults disabled; enable only `native`, keeping optional jsonrpsee disabled. RPC hash/status types use third-party `primitive-types::H256` and `TransactionStatus`, with explicit client conversions to rtao's H256, preserving RPC's third-party-only layer. Six `test_rpc.py` cases exercise higher client/transaction nonce wrappers; their owning implementation must land before claiming all 40 tests covered. |
| 2026-10-14 | M1-03 / M1-04 | Pinned metadata contains unsigned `FixedU128<Frac32>` (`U96F32`) for RootProp and MinerBurned, absent from the plan's primitive type list. Add its exact u128/32-bit-fraction wrapper and mapping while retaining fail-closed unknown fractions. Shape hashes also include validated Lsb0/Msb0 bit-order tags: identical empty marker structures have different wire meaning. No upstream defect is inferred from these omissions. |
| 2026-10-14 | M0-21 / M0-23 | Q25 native balance implementation passes 50 all-feature tests with 10,000 property cases, no-default tests, strict Clippy and Rust 1.89 in both configurations. Post-change golden replay passes all 11 tests and all 2,899 codec samples. D-035 is recorded with input, arithmetic, precision and valid-chain JSON pinning tests. Completion commits still wait for the full required gates. |
| 2026-10-14 | M1-10…M1-14 / M1-09 | Client read-path preparation includes the minimum public error/nonce-cache wrappers required by six higher-level `test_rpc.py` cases. These source tests exercise the client/transaction boundary, so transport-only ports cannot complete M1-09. Implement the needed earlier wrappers under rule 5 while retaining full submission/nonce-lease work in M2. |
| 2026-10-14 | M1-10 | Public network identities are now generated by pin --networks, using two genesis reads per extracted endpoint and rejecting immutable identity changes. Five network tests, four generator tests and final offline input verification pass; full ClientConfig/connect verification remains in M1 integration. |
| 2026-10-14 | M1-13 / M1-23 | xtask compat v0 generates compatibility records from verified pins and metadata, with explicit raw BLAKE2b-256 versus SHA-256 identities. No validated range is emitted without immutable no-breaking drift, E2E and interop reports. Ten tests pass on current Rust and 1.89; offline output checking joins the generated-artifact gate. |
| 2026-10-14 | M1-17 / M1-18 | Client cannot depend on namespace types in rtao-reads without a layer cycle. Under rule 5, expose command-generated namespace accessors through a blanket Namespaces trait over View, exported from rtao::prelude, preserving Client/Snapshot fluent syntax and the separate curated metagraph trait. |
| 2026-10-14 | M1-15 | The ISO datetime parser retains CPython 3.12.12 parsing semantics, with credited source and the complete PSF license. The primitives package declares PSF-2.0 and the supply allowlist includes it. Python was not executed locally; the fetched source scratch was audited and removed. |
| 2026-10-14 | M1-02 / PR-U15 | Filed RaoFoundation/subtensor#3243 from sam0x17/subtensor, sdk-value-navigation at 1ac10f67ff3dcf8306d18183471158b8a648cdb4. Nine portable navigation tests, 71 library tests, strict core/binding Clippy and wasm checks pass. Upstream workflows failed at startup with zero Rust jobs; no forbidden label or review request was applied. Exact clone/hook/configuration directory removed after dependency audits; retained patch/evidence in docs/upstream/pr-u15.md. |
| 2026-10-14 | M1-17 | Category-generated namespace accessors now cover all fourteen pinned Python namespaces with SHA-verified inputs, exact catalog coverage and Python prefix checks; three mutation/regeneration tests pass. The facade prelude exposes current client/view and primitive vocabulary. Scoped checks and earlier reads/metagraph tests pass; full regeneration remains pending the 31-model and fixed-point shape integration. Evidence: `docs/execution/m1-namespaces.md`. |
| 2026-10-14 | M1-20…M1-24 | Resumed parallel preparation of Criterion/fuzz/nightly checks, metadata drift and immutable baselines, read-only live probes, four-target release assembly and bounded endpoint quota measurement. Literal BLS lock checks remain failing; no milestone exit, hosted green run or tag is inferred. |
| 2026-10-14 | M1-03 / PR-U14 | Filed RaoFoundation/subtensor#3244 requesting metadata/runtime-API discovery of selective metagraph field selectors. The pinned v478 enum has 77 indices and numeric Vec<u16> API arguments; regenerated descriptors and source/mapping mutation checks pass. Prior closed issues #1457/#1810 were checked. No runtime change, forbidden label or review request was made. Evidence and submitted body: `docs/upstream/pr-u14.md`. |
| 2026-10-14 | M0-09 / M1 integration | RPC dependencies enabled serde_json/preserve_order transitively, changing object-key bytes in extraction and the core fingerprint. Canonical v1 object serialization restores the unchanged production fingerprint; Python dictionary insertion order remains in entry arrays. Integrated xtask tests and generated golden calls pass. The restored pin check again fails solely on the unchanged literal BLS lock criterion; no pin rebaseline or exit waiver was applied. |
| 2026-10-14 | M1-24 | All seven extracted public pool endpoints accepted 252/252 paced lightweight reads at about one request per second without bursts, quota headers or policy refusals. PUBLIC retains 1/s and uses burst 1. Provider quota ceilings remain unknown; this is an observed operating rate for the sampled workload/source IP. All 39 RPC socket tests and strict Clippy pass. Isolated fixtures/artifacts audited and removed; evidence: `docs/execution/m1-endpoint-quotas.md`. |
| 2026-10-14 | M1-16…M1-18 / M1-22 | All 31 structs, four exact enums, eleven semantic field contracts and fourteen namespace accessors pass regeneration. Updated Rust 1.89 metagraph/runtime tests and strict product Clippy pass. Full Finney/testnet read-only probes pass: finality, V15/spec/code/digest pins, netuid1/largest three-path JSON equality, last ten event blocks and Enabled ephemeral signed transaction validation, returning Invalid(Payment) without submission. No validated compatibility range is inferred without drift/E2E/interop reports. |
| 2026-10-14 | M1-22 | Disposable core-canary preparation preserves release pins, resolves all monorepo dependencies/BLS patch to one immutable main revision and rejects active alternate/fork sources. Current main resolves to v478's commit. Full canary semantic tests pass after serial RPC reruns, except the preserved literal BLS count and a parent compatibility test-fixture integration issue now corrected in source; focused compatibility tests pass. Actual canary build provenance is separate from the retained release pin. Exact rehearsal removed after audit; no production pin changed. |
| 2026-10-14 | M1-23 | Release/docs automation and both Mac CLI/facade build rehearsals are implemented and verified; verifier tests, strict Clippy, rustdoc, mdBook, actionlint and ShellCheck pass. Complete publication still waits for hosted Linux/ARM/resource evidence, approved local trust anchor and authenticated builds from a clean release tag. Rehearsal suite failures remain recorded, never attributed as passing tagged provenance. Disposable release fixtures and isolated Intel stdlib were audited and removed. |
| 2026-10-14 | M1-20 / M1-22 | Local checks pass for 34 Criterion measurements and the actual 30% comparison, all 306 facade feature configurations, four instrumented ASan fuzz targets, strict Clippy and Rust 1.89. Evidence: `docs/execution/m1-performance.md`; measured no-host core 225, validator/miner 318, owner 394 on the recorded Mac target. Hosted lane runs and later conform/localnet additions remain required. Disposable copies/corpora audited and removed; shared caches retained for ongoing execution. |
| 2026-10-14 | M2 preparation | Preparing signer/wallet, catalog/oracle, portable submission helpers, MetagraphSync and the independent btauth client while the literal M0 BLS decision remains pending (execution rules 3 and 6). M1 exits, hosted checks and milestone tags remain required. No step checkbox is inferred from local preparation. |
| 2026-10-14 | M2-01 | Architecture's attached-key accessors lacked the intent-role context needed for Python's bare-signer fallback. Under rule 5, expose bare signer identity separately and resolve intent defaults through a role-aware public helper. A bare coldkey signer supplies no default hotkey; explicit overrides win and wallet public views never unlock. Fourteen current/MSRV role/signing tests and strict Clippy pass; evidence: `docs/execution/m2-signer.md`. |
| 2026-10-14 | M1-21 / PR-U12 | Filed [subtensor#3245](https://github.com/RaoFoundation/subtensor/issues/3245) requesting V15/V16 metadata assets tied to the exact release WASM. v478 has five assets and no versioned metadata; finalized Finney bytes/checksums demonstrate availability. Started with an asset-contract issue so release maintainers can choose the export's place in their train, rather than presenting an unvalidated release patch. This filing does not complete metadata publication or the pending localnet/hosted drift exits. Submitted body/evidence: `docs/upstream/pr-u12.md`. |
| 2026-10-14 | M2-11 / D-006 | Pure weight math passes six current/MSRV tests; staged schedule/payload helpers pass nine and strict library Clippy. The counter-threshold inclusion case also requires deriving the commit epoch after coinbase, while reveals use pre-coinbase state. Under rule 5, clarified this phase boundary within the existing D-006 correction and pinned reveal 126 versus the core's 110. Both no-host upstream safety probes fail as expected; existing upstream vectors still pass locally. Oracle vectors, receipt integration and PR-U5 filing remain required; evidence: `docs/execution/m2-weights.md`. |
| 2026-10-14 | M2-06 / M2-11 / PR-U1, U2, U5 | Filed [subtensor#3246](https://github.com/RaoFoundation/subtensor/pull/3246), [#3247](https://github.com/RaoFoundation/subtensor/pull/3247) and [#3248](https://github.com/RaoFoundation/subtensor/pull/3248), with retained patches and committed-HEAD Rust 1.89 tests, strict Clippy and wasm validation. U5 review head is `9b4d8c51aa7d9fb11be6f0e4cef500e02e0fad36`; its checked simulation-budget guard is also carried in rtao. Remote workflows failed at startup with zero Rust jobs; this remains unvalidated hosted evidence. The first U1 binding check invoked local Python through PyO3 discovery, violating the CI-only instruction; the incident is retained in `docs/upstream/validation/pr-u1-python-discovery-incident.txt`. Later checks disable discovery with `PYO3_NO_PYTHON=1`, now the workspace default. No corpus, recorder or oracle was run locally. Disposable filing worktree was audited and removed. |
| 2026-10-14 | M1-02 / PR-U9 | Filed [subtensor#3249](https://github.com/RaoFoundation/subtensor/pull/3249) at `0c4ed7585ae9d232577e53d45813f383e0ec2eec`, exposing a portable shared metadata IR and delegating the Python binding to it. Committed-HEAD Rust 1.89 validation passes eight portable regressions, 71 library tests (one pre-existing physical-GPU ignore), strict core/binding Clippy, wasm and formatting. The Python materialization regression is reserved for hosted CI. No local Python, forbidden labels or review requests were used; retained patch/body/gate evidence is in `docs/upstream/`. Hosted status and exact cleanup are being recorded. |
| 2026-10-14 | PR-U9 cleanup | U9 hosted Rust run 38561149347 and the nine other workflows ended at startup with zero Rust jobs/check-runs. No retry or forbidden notification action was taken. Audited and removed the exact 207 MB `tmp/upstream-metadata-u9` tree: no process, open-file, launchd, configuration, symlink, virtualenv or Git dependency remained. Evidence: `docs/upstream/validation/pr-u9-cleanup.json`; shared build/pin/tools and unrelated work remain live. |
| 2026-10-14 | M2-02 / PR-U10 | Filed [subtensor#3250](https://github.com/RaoFoundation/subtensor/pull/3250) at `c0208a82ab2a17a91c87654f3d6ebc0dc887cd20`. Portable `keyfile_env` exports pure password encode/decode and `keys::verify_strict` exposes existing Substrate verification without wrapped-message retry; host keyfiles delegate and retain compatibility. Both committed-HEAD pushes ran six no-host regressions, 71 library tests (one existing physical-GPU ignore), strict core/binding Clippy, wasm and formatting. The first SSH connection closed during the hook; an ordinary retry with SSH keepalives passed the same hook and published without bypass. No local Python or dependency/lock change. Hosted status and exact cleanup are pending their final records. |
| 2026-10-14 | PR-U10 cleanup | U10 Check Rust run 38564404907 and nine other workflows ended at startup with zero Rust jobs/check-runs. No labels, review requests or workflow retries were used. The exact 207 MB `tmp/upstream-key-helpers-u10` tree was removed after process/open-file, launchd, configuration, symlink, virtualenv and Git audits found no live dependency. Bounded evidence is `docs/upstream/validation/pr-u10-cleanup.json`. Shared caches and unrelated work remain live. |
| 2026-10-14 | M2 integration / M3 preparation | SS58 input spelling and descriptor-validated offline runtime loading pass current Rust and Rust 1.89 checks, including all 31 cache tests. The staged retirement probes retain all 15 portable modules against v478; the U10 password marker now targets the filed ungated export. Audited and removed the exact empty metagraph persistence and metadata-pin fixture parents. M3 server admission and miner publication are being prepared while the literal BLS criterion awaits an answer; all milestone exits remain required. |
| 2026-10-14 | M2-26 / D-037 | Independent security review found malformed secret-URI error disclosure, unwiped core Ed25519 objects and codec copies, lost transaction identity after a postbroadcast deadline, and unbounded password-file reads. Remediation is in progress. D-037 bounds password/keyfile reads to 16 MiB and validates opened regular descriptors before buffering; Unix nonblocking opens reject FIFOs without retaining a worker, while regular symlinks remain compatible. All nine wallet library tests pass on current Rust and Rust 1.89. Filed [subtensor#3251](https://github.com/RaoFoundation/subtensor/issues/3251) for actual Ed25519 object erasure; that upstream risk remains open and unaccepted. The security exit remains pending until findings are verified fixed or explicitly accepted. |
| 2026-10-14 | Q26 / Q27 | The user explicitly approved the built-dependency BLS check in both named questions and the guarded dormant lru advisory exception. These are specific criterion corrections; the later statement-only "yes" is not needed as approval evidence. Implementation and activation/unknown-entry regressions are in progress. Full gates must pass before any commit or push. Initial oracle staging remains pending/M2-05 until real authenticated hosted artifacts exist, with strict/history/removal checks retained; no milestone exit is inferred. |
| 2026-10-14 | Upstream quality audit / D-036 / D-038 | User requested the complete filing inventory and independent accuracy/fix proofs for all eight PRs and five issues. Source, exact submitted HEADs and live bodies were reviewed. The four Subtensor issues pass artifact-isolated current/MSRV actual-core tests and fresh historical metadata RPC comparison; #3251's Zebra citation is corrected to upstream 4.1.0, and #3252 now includes a reproduction and limits mitigation to tested signer/wallet/codec boundaries. Direct raw core APIs retain upstream diagnostics. Independent review found and locally corrected U10's late base64 wipe guard and Zondax's price-ratio unit label; follow-up publication awaits final corrected-HEAD checks. A foreign shared-target test-binary rerun is excluded; final proof uses distinct no-op workspace wrappers. Ledger Hunter setup indirectly executed a local Python helper contrary to the CI-only rule; the incident is recorded, further configure is paused, and the exact accidental home cache was audited/removed. D-036 and the strict btauth retention-margin D-038 are registered. Evidence: docs/upstream/validation/quality-*.md. This audit claims no milestone, hosted Python, physical-device or full-preflight completion. |
| 2026-10-14 | Q28 | The user explicitly approved the reviewed dormant ruzstd exception through the asynchronous "Allow guarded dormant exception" reply. Applied supply-ruzstd.patch exactly: only RUSTSEC-2026-0344 on registry ruzstd 0.8.3 with the pinned checksum and patched range >=0.9.1 may pass while absent from every workspace/fuzz/shipped all-target normal/build/dev graph. The 12 candidate regressions, separate-target Clippy and native 53-graph trial passed before approval. Final active supply verification passes against the canonical locks, including Unicode 0.6.0: unchanged cargo-deny passes all checks, both raw audits retain exit 1, all 53 graph checks pass, and the supply job exits 0. Current/MSRV guard and gate regressions, owned formatting and fresh isolated all-target Clippy pass; final raw evidence is ci/supply-24734 under the recorded task root. Both raw audits remain unfiltered/nonzero, cargo-deny stays unchanged, and all other findings/tool failures remain blocking. Full gates remain required before a commit, push or milestone exit; no repeated Q26/Q27/Q28 permission is required. Evidence: docs/execution/supply-ruzstd.md. |
| 2026-10-14 | Integration / upstream audit | Published Zondax #296's committed-head native price-unit correction (8645ebd; 5,604 tests pass) and narrowed receipt/Ledger claims against exact evidence. Independent native Unicode review found 80 decimal and 5,812 printability differences between the prior Unicode 16 data and pinned CPython 3.12.11/Unicode 15; root pins category 0.6.0, stages authenticated printable data, adds input/diagnostic regressions and retains full required Unicode notices. Canonical importer now curates registration/uids pruning provenance as extra authenticated inputs without growing the parity file map; its nine regressions and import/verification pass. Test-only RPC imports move through the testkit instead of relaxing layer rules. Full corrected-source gates, remaining follow-up publication and binary notice distribution still precede any milestone exit or main push. |
| 2026-10-14 | Integration | Final canonical dependency declarations/graphs pass after routing test-only core policy and test/tool RPC imports through rtao-testkit and offload through rtao-client; no third-party package identity changes or layer-gate relaxation. Fresh extraction, regeneration and strengthened Unicode attribution/parity checks pass (1,532 matrix rows; first oracle remains explicitly pending). Eight current-source ASan/coverage fuzz targets pass 1,379,172 executions with bounded evidence and audited corpus cleanup. The complete generated-template native preflight passes Rust 1.89 Clippy, both binaries, integration linking, consumer doctor and cleanup, while explicitly withholding milestone acceptance. Full book rendering passes pinned mdbook 0.5.4; its disposable root was audited/removed and the binary retained as a shared task tool. Both required data notices are included in prepared release/image distribution guards and license-only changes trigger relevant image/e2e checks. Full gates, hosted/live acceptance and the remaining Unicode follow-up publication still precede main pushes or milestone exits. |
| 2026-10-14 | M0 exit 9 / pending decision | The literal package-license exit criterion lists only the original/Apache base expressions, excluding the retained PSF clause for the existing CPython datetime port and Unicode clause for the authenticated printable data. The concrete correction in docs/execution/third-party-license-gate.md preserves Q1 and derives each exact required expression from its identified source; the strengthened gate passes actual canonical parity. Requested the user's decision under execution rule 2 before changing that exit wording or claiming exit/publishing main. Current/MSRV regression and full integration checks continue independently; no repeat of the already approved BLS or dormant advisory decisions is needed. |
| 2026-10-14 | Upstream audit | Published and verified the reviewed Unicode corrections to the existing stacked PRs: subtensor #3246 at 77480991b2b3b39ec828d2da9d7c6ecf31113a81 and #3247 at 849c192e9702fa245a6f16941359290786554d85. Exact committed-head outgoing gates pass current/MSRV portable/library tests, binding compilation, both portable wasm checks and upstream-pinned strict Clippy, using genuine separate functional/wasm/lint targets. Both remote/PR SHAs and revised bodies match; labels/review requests remain empty. The failed idle SSH attempt and corrected evidence-count guard are preserved, with no test/source relaxation. Owned finished worktrees/hooks/targets were audited and removed (7,054,340,580 bytes); common source/build caches and other owners' live inputs remain intact. Inventory stays eight PRs and five issues; no hosted Python, live Sudo or device acceptance is inferred. |
| 2026-10-14 | M1 release / attribution | Independent review of the actual archive/image inventory confirmed that LICENSE-MIT, LICENSE-APACHE, NOTICE and the retained CPython LICENSE-PSF were absent from binary/image packaging despite being retained in source. Apache source/object redistribution and the PSF source/binary clauses require the retained license/copyright and relevant notices; shipping both base licenses preserves the advertised OR choice. Root NOTICE now includes the existing parser modification summary. Preparing the bounded four-file addition alongside the already verified certificate/Unicode notices, with source/tag-bound NOTICE, immutable text checks, exact inventory/checksums/provenance and both image-stage guards. This enforces existing Q1/source obligations without adding a license choice; the pending literal M0 exit 9 wording question is unchanged. Expanded-bundle tests and final full gates remain required; no release/image run or milestone exit is claimed. |
| 2026-10-14 | Upstream packaging audit | The user surfaced #3250's eager Python exports versus the unchanged native dependency floor. Verified published source assigns _backend.verify_strict and _backend.encode_password_for_environment during module import. Live PyPI has stable bittensor-core 0.1.5, no stable 0.1.6, and 0.1.6rc68/rc69; source >=0.1.6 excludes those older versions. Actual bittensor 11.3.1rc69 pins native ==0.1.6rc69, matching the RC release train. Stable preparation retains the committed version range while building native and Python from the immutable release commit. The generic mismatched-wheel failure mechanism is valid, but no presently installable broken stable combination or correct future floor is claimed. Reopened read-only release-coordination review before choosing a follow-up; no Python ran or native package was published locally. |
| 2026-10-14 | Canonical integration | Full gates caught three concrete defects after the notice repair: authored Unicode attribution after the importer-generated block, a private btauth response triggering current Clippy's large-error lint, and two staged method calls rejected by pinned upstream linting. Root preserved and moved the complete authored notice before the unchanged generated block, boxed the private error response, and called the existing checked integer helper directly. Import verification and pinned upstream lint now pass; workspace Clippy passed after the response repair, and all 14 current server tests pass. Initial failures remain recorded; final current/MSRV regressions and complete gates still precede publication. No gate was relaxed. |
| 2026-10-14 | M4-01 preparation | Adopted the independently reviewed 14-path hyperparameter implementation, two-path checked-scale repair and four-path hosted oracle recipe with exact patch/before/after hash checks. The generated table has 48 descriptions/42 storage rows and all ten methods; native u128 hyperparameters remain separate from Q25 balances. Both repair regressions reproduced the original accepted-scale overflow on current/MSRV; final isolated affected-package suites pass 151 tests each, and oracle suites pass 21 each with strict Clippy. The oracle recipe covers 7,224 inputs without local Python outputs. Canonical integration/full gates and actual hosted differential acceptance are pending; M4-01 and milestone boxes remain unchecked. Evidence: docs/execution/m4-hyperparams.md. Independent M4 preparation continues while checks and prerequisites wait, under rules 3 and 6. |
| 2026-10-14 | M2-24 prerequisites | The user selected a new dedicated testnet subnet and `sam@matthews-mac-studio` for the soak, with an explicit low-compute constraint because the host has other uses. Read-only SSH inspection with existing host-key validation passed (Darwin 27/arm64, 32 logical processors, 256 GiB RAM); no host files/settings/services changed. Build locally, use burned registration, run no PoW or compiler on the soak host, and plan two Tokio worker threads, lower scheduling priority, measured CPU/RSS and bounded logs. Wallet funding/address, actual registration/staked permit/CR state, deployment and observed 24-hour results remain pending. Evidence: docs/execution/testnet-prerequisites.md. |
| 2026-10-14 | M4-01 canonical validation | Canonical current/MSRV affected-package suites each pass 151 tests; final boxed-response server and staged receipt/Unicode regressions pass. Extraction, regeneration, parity and dependency checks pass. Full gates correctly rejected a stale build-weight report, so root ran actual target/profile remeasurement and regenerated compatibility outputs (225 core-no-host, 361 validator, 365 miner, 416 owner; host only for owner). The repeated full gates pass those checks and are live at nextest test enumeration; newly launched executable samples show macOS dyld startup delay before Rust main. No complete gate success, push or milestone exit is claimed. Evidence: docs/execution/m4-hyperparams-validation.json. |
| 2026-10-14 | M2-24 wallet preparation | Root audited the native helper, its seven fixture tests/24 complete-state mutations and exact executable/artifact hashes, then created the new durable local testnet wallet and verified identical public-only reinspection. Funding coldkey: 5G3pwJ3ptR4x83zuKzFvTC2TA8WeivVtP1NhgxDxPmi2RAvX. Wallet, password/recovery and lifecycle journal remain private under /Users/sam/.local/share/rtao/testnet-soak and are retained live dependencies; the public manifest excludes secrets. No transaction, remote wallet copy or service has run; live quote/funding/setup/soak remain pending. The user reiterated cleanup of task-created Studio directories: remove finished scratch after dependency checks, preserve wallet/state, and report retained live paths. Local builds are on Sams-MacBook-Pro.local; remote Matthews-Mac-Studio.local use remains read-only inspection. |
| 2026-10-14 | Q29 / M2-24 waiver | The user instructed "we can probably skip a testnet soak, let's just make sure all tests are passing" and waived the 24-hour testnet soak. Updated the owning plan, architecture, validator guide and execution records; M2-24 and exit 9 record the waiver without claiming a soak result. All automated gates, localnet epochs/reveals/restart tests, image/TLS, oracle, interoperability and release checks remain required. Dedicated subnet/funding/host setup and quote preparation are stopped. Preserve the prepared wallet/recovery root; no transaction, remote wallet copy, service or remote task directory was created. Dependency-audited cleanup of finished local scratch remains part of completion. |
| 2026-10-14 | Upstream tracking / testnet 479 | Read-only finalized RPC, release/ref and anonymous GHCR checks confirm Finney spec 478 at the pinned code hash and final v478 release/image. Testnet now reports spec 479; authenticated V15/V16 production drift plus independent subxt diff classify seven additions (one call, two errors, two events, two storage entries). No final v479 tag/release/image or source association was verified. Keep core/parity v478; record testnet metadata through xtask drift at a source-stable point with prescribed checks. No runtime behavior/E2E compatibility, transaction, publication or new filing is claimed. Bounded evidence: target/execute-plan.6Ro0PC/runtime-479-drift/. |
| 2026-10-14 | Automated gate repairs | Full canonical nextest completed 1,184 tests: 1,179 passed, five failed, with four separate skipped tests. Failures exposed native certificate enumeration before plaintext RPC deadlines and intermittent native-root availability in the miner HTTP fixture. Adopted two independently tested scoped patches: RPC skips native enumeration only for all-ws primary/fallback/archive pools (WSS and public tls_config trust behavior retained); miner fixture supplies owned explicit ring TLS after actual registration/hotkey checks, with production SignedClient::new unchanged. Current/MSRV affected RPC 80 tests and miner regression pass, strict Clippy passes both; original deadlines/assertions retained. Full gates rerun remains active, no complete success/publication or soak result is claimed. Evidence: docs/execution/tls-roots-repair.md. |
| 2026-10-14 | Publisher startup repair | The complete TLS-repaired rerun passed 1,190/1,191 workspace tests; all four RPC failures were resolved, but the miner routing deadline exposed a second eager native-root lookup in AxonPublisher even with an explicit IP. Adopted the independently reviewed two-path lazy publisher patch `1099b99dce3f009992d70a6e18b9ff24ea90914766b692f0fcdeb6b6981058b3` with exact before/after hashes. Counterfactual counted-loader tests fail under eager behavior on current/MSRV; all ten publisher tests and the unchanged miner regression pass on each compiler, with the miner test completing in 0.66 seconds, with strict Clippy on both. Native roots/ring/ALPN/redirect/5s discovery policy remain; client construction errors now surface at first automatic lookup. No-host passed before adoption; final all-job gates rerun is active and remains required. No test deadline/assertion was weakened and no host setting changed. Evidence: docs/execution/tls-roots-repair.md. |
| 2026-10-14 | Canonical native validation | All 1,192 workspace tests pass in 43.321 seconds after the scoped TLS/publisher repairs; doctests, no-host and MSRV pass in the same full gate run. Features correctly fails when the disposable no-dev copy retains rtao-rpc feature forwarding after removing its dev declaration; preparing an exact harness-only repair without changing original manifests, feature names, build edges, command/profile coverage or retained-lock provenance. Separate supply passes with the approved exact dormant findings, raw reports retained and all activation guards; semver exits without comparison before the first tag. Full all-job gates and hosted oracle/localnet/interop acceptance remain required; no push, formal milestone exit or release is claimed. New upstream findings remain local drafts and the filed inventory remains eight PRs and five issues. |
| 2026-10-14 | M4-02…M4-07 integration | Root rehearsed eight exact reviewed patches against current source and adopted 66 paths with baseline/adoption hashes and reversible backups. The composition retains both multisig decoder leaves and adds three owner-read leaves once; preserves D-039/Q29/license notices; adds the final runtime-cap supplement, D-040's mutable start delay and saturating u64 addition, and nested empty-composite decoding. Canonical regeneration succeeds and reports 20 implemented intents/38 reads, with 63/64 pending; these are source counts, not oracle acceptance. Combined extraction/regeneration/parity/dependency measurement and current/MSRV tests run next, followed by complete gates after feature-harness repair. M4-08 owner orchestration and M4-09 CLI machinery proceed in isolated snapshots while checks wait (rules 3/6); no milestone exit, completing commit or release is inferred. Evidence: docs/execution/m4-integration.md and root-m4-integration/ under the recorded execution root. |
| 2026-10-14 | M4 combined checks / feature harness | Actual canonical matrix generation (1,533 rows), extraction/regeneration, dependency measurement, dependency/parity and formatting checks pass; current-Rust affected tests pass 810 native and three doctests, with one hosted oracle replay ignored. Rust 1.89 remains live. Independently reviewed feature harness preparation now validates the original locked graph before removing only dev-only forwarding, preserving real invalid-feature errors, aliases/normal/build/target features, feature-name collisions, weak behavior and exact retained-lock provenance. Final isolated 14-test/current/MSRV regressions, strict Clippy and full real feature matrix pass for patch 7762d151298a6a922066a5db6225e914d6802df7e3147c8e21e220afe4fd735e. Root's dependent driver requires successful terminal M4 validation and unchanged 393 input hashes before exact adoption and complete canonical gates. Removed the finished 3.7 MiB composition rehearsal after process/open-file/launchd/configuration/symlink/venv/Git audits; source backup/build cache remain live. No full canonical gate, formal milestone exit, push or release is claimed. Evidence: docs/execution/m4-integration.md. |
| 2026-10-15 | M4 canonical validation / harness adoption | Combined source checks now complete: 810 native tests and three doctests pass on both current Rust and Rust 1.89, with the same existing hosted replay ignored; actual matrix/extraction/regeneration/dependency measurement/dependency/parity/formatting checks all exit 0. Root verified 393 source hashes and the final isolated feature repair's 14-test/current/MSRV, strict Clippy and full 95-check matrix proofs, then adopted only xtask/src/gates.rs from patch 7762d151298a6a922066a5db6225e914d6802df7e3147c8e21e220afe4fd735e. Exact before/after hashes, reversible backup and actual apply checks are retained in root-feature-harness-adoption/. Complete canonical gates now run in root-m4-feature-repaired-gates.log; no complete gate success, push, milestone exit or release is claimed. M4 owner proxy routing and CLI source/header/attribution checks and M5-01 offline SDK preparation proceed in isolated snapshots under rule 3. Pending license-criterion decision remains unchanged. |
| 2026-10-15 | M4 source review / D-041 preparation | Independent owner proxy review found no remaining source defect; final native validation remains pending after a provisional fixture subscribed to the wrong head stream. CLI review established an unsafe automatic multisig reopening schedule from authenticated Python and runtime sources: call bytes alone do not preserve the original opening. Under rule 5, reserve D-041 for exact opening-timepoint continuation, with same-view composition and absent/replaced-state refusal; isolated source is ready, but final native pinning tests, canonical registration and any filing remain pending. A source/pin guard for the System timepoint assumption is being prepared. Config/tools/explain source and 16 fixtures are ready for compilation; M5 offline import review caught and is repairing the core decoded-call/encoder representation mismatch. The Keychain harness remains injected-only. Preserve confirmed-live canonical/worker handles through macOS startup delays and continue independent source work under rule 3. No new public filing, completed check or milestone is inferred. Evidence: docs/execution/m4-cli-preparation.md. |
| 2026-10-14 | Q30 resource/scope correction | User rejects the local build campaign after hundreds of gigabytes of disk use. Substantial checks move to bounded free hosted CI; full local gates, node/testnet/container operation, parallel/duplicate builds, extra toolchains, Keychain/device/release rehearsals and local fuzz/bench campaigns are removed. Mainnet cloning is removed; release/upkeep and platform/hardware acceptance are deferred. C1-C7 now describe development acceptance without claims for removed checks. Cleanup results are tracked separately in execution state. |
| 2026-10-15 | Q30 client checkpoint / license wording | Resume source development with hosted validation. The first checkpoint preserves the current M0-M4 implementation (20 intents, 38 reads) without claiming milestone acceptance. Correct M0 exit 9 under rule 5 to enumerate Q1's already required PSF/Unicode obligations, preserving the original dual license and unchanged license gate. Private recovery is excluded from Git and Docker contexts. All substantial tests remain hosted. |
| 2026-10-15 | M4-08 / M4-17 source restoration | Restore the authenticated owner orchestration base, executor retry supplement, final proxy routing and owner example to the canonical worktree from the private recovery archive. Exact patch hashes and apply/format checks pass. Keep source acceptance pending hosted CI; no local build, node or device work. Baseline source checkpoint 450c914 is pushed and workflow 38680556857 is live. No milestone completion or new filing is claimed. |
| 2026-10-15 | Hosted source generation | Add an explicit manual compatibility-report mode to the existing Ubuntu workflow. It runs only cargo-tree/report generation, skips the six test jobs, records the real Linux host for the existing Apple-target graph and uploads three small source reports for review. It cannot push code, run nodes or claim native Mac coverage. Normal checks still verify committed generated reports; local compilation is not needed to refresh manifest inputs. |
| 2026-10-15 | Hosted correction / M4 CLI source | Adopt verified compatibility reports from run 38682108313; preserve empty runtime validation. Correct Q30 oracle staging without removing strict/history guards, and prevent completed multisig approvals from inheriting inner owner retries. Restore hash-verified CLI machinery, replay-secret and attribution support, then config/tools/explain handlers and binary routing. Register D-041 opening preservation and D-042 input budgets with regression sources. Resolve only required lock additions offline, with no compilation. Refresh compatibility and declared parity reports in hosted CI; no new filing, milestone completion or local build is claimed. |
| 2026-10-15 | M0 acceptance audit | Source audit found M0 exit 8 covered only by doctor helper tests. Add real-binary canonical-success and exact missing-patch diagnostics fixtures for hosted CI; force plain Cargo tree output under caller color, preserving locked/offline BLS checks. Correct CONTRIBUTING for CLI PSF and core-ext Unicode obligations. M0 acceptance remains pending all applicable hosted checks; no local build/test or new filing. |
| 2026-10-15 | M4-10 / D-043 generated query inputs | Register additive per-entry account-name resolution for plural generated-query `_ss58s` parameters. Pinned `cli/query.py:161-178` splits array CSV inputs but resolves only scalar `_ss58` names. Prepared Rust source preserves account-list order and duplicates and refuses unknown entries before connecting; two existing regressions cover parsing and fake-backed batched read dispatch. Source and regression references are inspected; hosted validation remains pending and no successful CLI build is claimed. No upstream defect is alleged or filing made. No local builds, tests or scratch were created. |
| 2026-10-15 | M4-10 / D-044 PoW preparation | Review found that initial expired proofs failed strict SDK planning before late refresh could run. Add public-only preparation for actual unshielded requests, preserving the reviewed proxy owner and explicit hotkey, then refresh again after unlock. Dry runs never mine or unlock and still refuse stale challenges. Register those timing/preview differences as D-044 without alleging an upstream defect. Three-case fake submission and dry-run sentinel fixtures await hosted execution; no local test or mining ran. |
| 2026-10-15 | Hosted CLI integration | The complete 0fbf9bc check ran 1,678 tests: 1,659 passed, 19 failed and four other tests were skipped; doctests were not reached. Repair the shared test-temporary-directory environment and duplicated-NOTICE fixture without weakening gates. Push generator prerequisites and obtain successful bounded generation run 38686769925 at f841f63. Verify artifact digests, exact paths, 83/102 metadata names, 23 dependency hashes and unchanged package sets before byte-for-byte adoption. Integrate generated tx/query, review facts, PoW preparation, weights/axon and implemented hparams commands. New source tests and all applicable normal hosted checks remain pending; no milestone acceptance or local build is claimed. |
| 2026-10-15 | M4-11/M4-12 source preparation | Prepare wallet and subnet command routing with typed SDK reads/intents, the rtao-only pure password export, encrypted-import and Unicode-order fixtures. An independent reader found blocking FIFO opens; reuse the SDK bounded nonblocking reader and add real-binary Unix refusal coverage. Resolve one new pure casefold dependency without building, authenticate its source and verify Unicode 15 behavior against official data. Keep sequential trim-null unadvertised until shared durable SDK/CLI state integration exists. Clarify that the node-managing localnet command is deferred under Q30. No local builds, tests, wallet operations or mining; compatibility refresh and hosted acceptance remain pending. |
| 2026-10-15 | M5-01 / D-045 shared persistence | Prepare Python-schema offline transaction import/preparation/submission, keeping runtime/call/payload authentication and current policy checks. Independent review moves nonce reservation after signature attachment so stale or foreign preparations cannot clear the cache. Adopt the two owner-read interfaces from verified hosted run 38690259736; coverage is 20 intents and 40 reads. Add shared multisig book/cache writers under architecture section 10.2, registering atomic publication, corruption retention and bounded authenticated lookup as D-045. Retain fuzz target source without executing a campaign. Source, fixtures and docs remain pending combined hosted acceptance; no new filing or local build. |
| 2026-10-15 | M4-12 shared Null-pruning source | Extract a shared guarded SDK session and drive it through CLI review and durable submission hooks. Bind rebuilt calls to the reviewed bytes before private acquisition, record raw receipts before release/follow-up failures, and verify canonical pending approvals between saved multisig rounds. Add real two-member, preview, proxy, uncertain-delivery and finalized-progress regression source. Review replaces the fixed 32-approval bound with group size and reuses bounded nonblocking journal reads. Only formatting/source checks ran locally; the command remains pending hosted validation and no milestone exit is claimed. |
| 2026-10-15 | M5-02 crowdloan/root source | Port ten crowdloan and two root intents with native TAO validation, effective-origin all-funds arithmetic, nested semantic construction and fixture source. Bound nested deserialization and retained subtree copying under D-046. Correct D-045's evidence fixture without changing the parity checker. Authenticate pinned Python sources; verify and adopt hosted run 38694967521's exact generated/report archives, 23 dependency hashes, twelve pending removals, nine call maps and matching status changes. Recorded coverage is 32/83 intents and 40/102 reads. The prior normal run passed compilation, strict Clippy and five jobs but stopped at parity before nextest/doctests. Combined hosted acceptance remains pending; no milestone completion, new filing, local build or scratch root. |
| 2026-10-15 | M5-02 coldkey source / CLI exposure | Port four coldkey operations with pinned summaries/warnings, unchanged no-spend policy and decoded-account BLAKE2b-256 hashing. Add independent-vector, raw SCALE, prefix, JSON and preflight fixture source. Enable all CLI intent interfaces while retaining implemented-only command advertisement. Verify and adopt generation run 38696143491 at 18679e3 with exact artifact identities, 23 dependency hashes, four pending removals, three call maps and unchanged role package sets. Recorded coverage is 36/83 intents and 40/102 reads. Combined hosted tests remain pending; no milestone exit, new filing, local build or scratch root. |
| 2026-10-15 | Hosted acceptance correction | Normal run 38695489489 at fa77262 passes compilation, strict Clippy and the five other jobs, then rejects D-046 against the default-feature test index before nextest/doctests. Align divergence qualification with the existing ordinary all-feature harness/body index used by the required test command, retaining cfg, ignored-test and target/harness exclusions. Add actual source-index and missing-body regressions. Adopt verified generation run 38696690648, changing only the two unused fieldless-call parameters plus source provenance reports. Combined normal checks and milestone acceptance remain pending. |
| 2026-10-15 | Hosted test evidence | Normal run 38697413448 at 60ccce5 passes compilation, strict Clippy, parity and the five other normal jobs. Nextest runs 1,850 tests: 1,832 pass, 18 fail and four are skipped; doctests are not reached. Correct source-backed CLI schema/output and wallet fixtures, stale pending-operation assertions and the invalid custom-prefix assumption before another normal run. The exact failure inventory and log digest are retained privately. No milestone acceptance, new filing or local build is claimed. |
| 2026-10-15 | M5-02 proxy/children/association source | Prepare thirteen explicit behavior ports with authenticated pinned sources, independent SCALE fixtures, Python child-share/take normalization and nested proxy wallet/default semantics. Share the existing bounded JSON scanner, extend D-046's constructor coverage and qualify kept D-020 behavior with real fixture references. Record JSON carrier and malformed-child diagnostic limits. Remove only these authored pending entries; generated integration and hosted execution remain pending. Correct all eighteen failures from the preceding hosted run using source-backed fixture changes, preserving assertions and production behavior. No milestone exit, upstream filing, local build or scratch root. |
| 2026-10-15 | M5-02 proxy/children/association generation | Verify and adopt hosted run 38700421705 at c39fab3 unchanged: fourteen exact archive members and digests, 23 dependency hashes, unchanged role inventories, thirteen pending/availability changes, ten call maps, six constructor changes, five JSON normalization bodies and only the corresponding matrix/D-020/D-046 updates. Declared coverage is 49/83 intents and 40/102 reads; ordinary compilation/tests on this combined checkpoint remain pending. All archive handling used memory; no local build or scratch root. |
| 2026-10-15 | M5-02 lock/basket/root-claim source | Prepare seventeen operations across lock, collateral, leasing, EVM, basket/configuration and root claims, with authenticated pinned sources and fixture tests. Root claims include mandatory admission/reserve preflight and quote failure behavior. Wire existing generated maps and only the required registration/futures and collateral/tokio features; correct candidate API references, typed EVM JSON, integer-key fixtures and unknown-prefix assumptions. Preserve native u64 balances while correcting intent amount strings to Python decimal formatting. Generated integration and hosted compilation/execution remain pending; integrated coverage stays 49/83 intents and 40/102 reads. No new filing, local build or scratch root. |
| 2026-10-15 | M5-02 lock/basket/root-claim generation | Verify and adopt hosted run 38702003174 unchanged: fourteen exact archive members/digests, 23 dependency hashes, unchanged role inventories, seventeen pending/availability changes, eleven call maps, two constructors, two JSON bodies and only matching intent matrix changes. Declared coverage reaches 66/83 intents and 40/102 reads. Normal run 38701008514 at b0e0aa1 passes compilation/Clippy/parity and five other jobs; 1,871 of 1,872 tests pass. Correct the one metadata-default fixture with an explicit map miss; doctests and combined acceptance remain pending. No milestone exit, new filing or local build. |
| 2026-10-15 | Hosted collateral-read correction | Run 38702708753 at 359b09e passes supply/no-host/semver but fails compilation in check/features/MSRV. Register the existing MinerCollateralState typed decoder as ReadValue, fix the root-claim fixture helper path and remove its unused import. Nextest/doctests were not reached; another normal hosted run is required. Instance-role/batch development remains separate source work. No local build or new filing. |
| 2026-10-15 | M5-02 batch/trades/identity source | Prepare seven ports with guarded child normalization, semantic Batch composition, full preflight aggregation, native-u64 staking trades, identity UTF-8 and legacy hotkey swap behavior. Route instance signer roles through dynamic SDK/CLI paths; typed entry points refuse mismatches before key access. Add lazy-acquisition, locked-preview, isolated-hook and CLI hotkey Batch regression source. Correct generated leasing Copy handling after normal run 38703332560 passes five jobs but fails strict Clippy before tests. Close the remaining Q30 plan-audit references and add bounded hosted strict rustdoc/book/index checks using the existing target. Generated integration, ordinary tests and milestone acceptance remain pending; integrated coverage stays 66/83 intents and 40/102 reads. No local build, scratch root or upstream filing. |
| 2026-10-15 | M5-02 batch/trades/identity generation | Verify and adopt hosted run 38704675475 unchanged: fourteen exact regular archive members/digests, 23 dependency hashes, unchanged role inventories, seven pending/availability changes, one identity helper, two leasing Copy corrections, one guarded/canonical Batch constructor, one normalized JSON body, seven matching intent statuses and D-046's Batch fixture reference. Declared coverage reaches 73/83 intents and 40/102 reads. Ordinary compilation/tests and milestone acceptance remain pending. All archive handling used memory; no local build, scratch root or new filing. |
| 2026-10-15 | Hosted staking-quote decoder correction | Normal run 38705303263 at 1c807e1 fails check/features compilation because the existing SimSwapResult model lacks client ReadValue registration. Register its already-derived typed decoder. Supply/no-host/semver pass; MSRV is still running. Tests, doctests and new hosted docs checks were not reached. Retain bounded failure evidence privately; no local build or generated edit. |
| 2026-10-15 | Hosted lock-fixture evidence correction | Normal run 38705671132 at 93f562c passes compilation, strict Clippy and five other normal jobs, then parity rejects the dynamic lock settings fixture for missing explicit Rust class references. Add typed variant/field assertions for MoveLock, SetPerpetualLock and SetRejectLockedAlpha alongside existing byte/policy/preview assertions. Keep the evidence checker unchanged. Nextest, doctests and docs builds remain unexecuted on this source; bounded failure evidence retained privately. No local build. |
| 2026-10-15 | M5 final intent / CLI source | Prepare ten remaining ports with full claim-aware staking, governance-only StakeBurn and raw multisig funding/weight/nested-input behavior. Add LockState decoding, bounded multisig JSON hooks and Python result-extra decoding. Author the 20 CLI routes, shared proxy/multisig lifecycle, pinned pending-call details and bounded advisory proposal discovery. Register D-047/D-048 for approval-only book retention and honest unknown proposal status, without new filings. Add actual 21-constructor policy fixtures, all-83 ordinary planning with all12 supplementary samples, deterministic public PoW and generated transaction docs. Map only the 72 plan keys to the exact qualified ordinary target; record fixture identity adaptations. Hosted run 38707066144 passes compilation/Clippy and five jobs but parity requires explicit AddStake test typing; name concrete types in existing fixture bindings. Generated integration, tests and acceptance remain pending; integrated counts stay 73/83 intents and 40/102 reads. One offline lock resolution adds no versions, only the existing reqwest edge. No local build or scratch root. |
| 2026-10-15 | M5 final intent generation | Verify and adopt hosted run 38709002368 unchanged: fourteen exact regular archive members/digests, 23 dependency hashes, unchanged role inventories, ten pending/availability removals, four guarded constructors, four bounded JSON bodies and only matching intent/72 plan mappings/D-046/D-047/D-048 matrix changes. No pass-through or other interface changes. Declared coverage reaches all83 intents and 40/102 reads. Combined compilation/tests/doctests/rendered docs and milestone acceptance remain pending. Archives inspected in memory; no local build, temporary root or new filing. |
| 2026-10-15 | Hosted final-intent compilation corrections | Normal run 38709461377 at 5cba852 fails check/MSRV compilation: multisig argument iterator lifetimes, NetUid/u16 policy assertion and unsupported H256 FromStr in the public PoW fixture. Apply explicit shared lifetime, order-preserving native ID projection and strict fixed-width hash decoding. Preserve all assertions and gates. No-host/supply/semver pass; features remains running at inspection. Tests/doctests/docs were not reached; bounded failure evidence is retained privately and the next ordinary hosted run must verify the fixes. No local build, scratch root or new filing. |
| 2026-10-15 | Hosted multisig fixture lock correction | Normal run 38710157011 at c6bd593 passes MSRV/no-host/supply/semver. Strict Clippy next rejects a test lock whose borrowed request remains in scope across client.close().await. Use a lexical block to end both borrows before awaiting, preserving RPC hash assertions and all gates. Features is running at inspection; tests/doctests/docs were not reached. Another hosted run must validate the correction. Independent M6 source preparation proceeds without local builds. |
| 2026-10-15 | Hosted M5 test evidence and source corrections | Normal run 38710720744 at 06a3598 passes compilation/Clippy/parity and five other jobs. Nextest runs 2,021 tests: 2,005 pass, 16 fail and four are skipped. The all-83 plan fixture passes. Correct source-backed fixture assumptions for multisig timepoints/probe boundaries, storage batching/observations, UTF-8 rendering, command availability, distinct input addresses, last-allocated nonce state and valid native subnet4095; add explicit invalid4096/65535 cases. Preserve all gates and strengthen exact observations. Doctests/docs and combined acceptance remain pending. |
| 2026-10-15 | M6-01/02/03 source and acceptance audit | Author all remaining62 reads, exact-hit/head/history/native-boundary fixtures, curated aliases, public aggregate block lookup and a cold blocking runtime/facade with14 namespaces and generated catalog adapters. Independent source reviews correct commitment text rendering, CPython3.12 summation and bounded TLS teardown; execution remains unverified. Source pending register is empty, while generated availability stays40/102 until verified hosted adoption. Add missing M4 owner template/book guides and bounded hosted downstream checks; default template workflow starts no live service. Record final-range security/mapping/docs backlog without milestone acceptance. One offline lock resolution adds only existing dependency edges; no local build, temporary root or new filing. |
| 2026-10-15 | Hosted M5 tests and M6 generated integration | Run 38714093697 at 745f3f3 passes all 2,022 executed nextest tests, nine doctests and five other jobs, then fails strict rustdoc on six literal angle-bracket placeholders. Generator and exact hosted output are corrected without weakening the gate. Verify and adopt generation 38715060741 at 7953bea: fifteen exact regular members, 23 dependency hashes, unchanged role inventories, all62 read bodies/output mappings and14 blocking namespaces/101 methods. Integrated coverage reaches83/83 intents and102/102 reads. This generation establishes no new test success. |
| 2026-10-15 | M6 CLI, docs and security source checkpoint | Commit all24 M6 command routes and complete102-query tree with parser/entrypoint/FakeChain fixtures and independent source reviews. Adopt45 basket samples/171 fund baselines from hosted38716924750 at c281803; exclude frozen display data from the shared catalog in generator source. Add per-operation/query/error/hyperparameter reference generation and source-derived catalogs, hosted raw/rendered docs and LLM links; correct five independent review findings. Commit bounded keyfile parser repair and four M4 security repairs with meaningful regression source. M7 starts with pure UOS framing and14 hand-derived fixtures. No new milestone, fresh oracle/device/platform result or filing is claimed. Current source needs hosted generation and combined tests. Local target remains534 MB/recovery146 MB, with no build, service, tool installation or temporary root. |
| 2026-10-15 | M6 combined parity corrections | Run38720004914 at4058413 passes features/MSRV/no-host/supply/semver and main compilation/Clippy, then rejects four parity consistency checks before tests. Correct canonical source/header order, compare the generator's exact two escaped placeholders, exclude private/synchronous helpers from catalog entry-point discovery, and register only the four reviewed ordinary read-test bindings with assertion/source checks. Add negative fixtures without disabling checks. Hosted basket regeneration/adoption and ordinary execution remain pending. No local build, scratch root or filing. |
| 2026-10-15 | M7 source integration | Integrate Ledger/Vault/extension signers and account-bound CLI acquisition; all34 M7 routes; EVM keys,21 precompiles, HTTP transactions and shared primitive address helpers; timelocks, Subxt legacy transport and optional GPU PoW. Author fixtures and independent source reviews without local execution. Repair guarded plaintext ownership, cancellation/session/file-publication paths and QR offload. Add signer/Ledger/EVM guides,36 exact public export aliases, D033/D042 source mappings and D051 proof consent/account binding. Resolve metadata-only dependencies at MSRV-compatible versions; no build. Latest M6 run38723619536 passes five jobs/compilation/Clippy and all parity layers except the facade license; correct its manifest. Fresh hosted graph/matrix generation and ordinary execution are required before acceptance. Core-patch/replay, remaining transport fixtures and M8/M9 still remain. No new filing or scratch root. |
| 2026-10-15 | M7 hosted report adoption | Verify and adopt run38725474706 at01b1f7d: exact four regular report members, archive digest/size, clean Linux source provenance and all23 committed dependency hashes. All four role package inventories/counts/host features remain unchanged (225/361/365/416); optional M7 dependencies stay outside neuron profiles. Matrix changes are limited to36 reviewed export aliases, D033 activation, D042 timelock bounds and D051 consent/effective identity. No generated interface or runtime/oracle validation claim changes. Ordinary M7 compilation/tests and milestone acceptance remain pending. Archives handled in memory; no local build, scratch root or filing. |
| 2026-10-15 | M7 hosted compilation corrections | Run38726489833 at7956175 stops main Clippy on constant chunks_exact and MSRV on two ambiguous Zeroizing array AsRef calls. Use stable array chunks and explicit byte slices without changing behavior. No-host rejects Vault's newly required digest/proof/QR offload; permit only signer ledger/vault features and add negative declaration fixtures. Supply detects active ruint1.17.2 advisory RUSTSEC-2026-0220; compatible remediation remains under investigation and is not waived. Semver passes; tests and milestone acceptance remain pending. No local build or scratch root. |
| 2026-10-15 | M7 transport and hosted-source corrections | Run38727084747 at73b4fde passes MSRV and semver; main Clippy stops on16 signer nested-condition lints, no-host on timelock's offload declaration and features on Subxt's private-module import. Apply let chains, narrowly permit tx offload with positive/negative declaration fixtures and use the public backend re-export. Add independently reviewed automatic watcher installation, distinct retraction A/B, single nonce consumption and unusable-default TLS fixtures; actual dual-provider coverage remains absent. Preserve known receipt identity on direct/saved output errors with authored regressions. No local build or acceptance claim. |
| 2026-10-15 | Active integer advisory source remediation | Retain fixed upstream ruint1.20.1 with exact archive/license/file provenance and only the Rust1.89 declaration/previous const reversal loop restored. Independent review verifies unchanged security fixes and all other source files. Add source/identity guards, five no-allocation probe fixtures under an external hosted timeout and four integrated Alloy regressions. Metadata-only resolution adds the path patch and no existing baseline package upgrades; fresh hosted MSRV, supply and execution remain required. No advisory suppression, local build or new filing. |
| 2026-10-15 | M8 CLI source integration | Author remaining45 routes, shared bounded contact/proxy writers, live stake pickers/atomic exits, wallet message APIs, bounded history, upgrade verification/approval and exact native balances. Integrate one installed command-tree builder, historical dispatch, five-shell completions and finite Python JSON formatting. Independent reviews repair schema/equality, bounds, prompt/account, sorting/precision, artifact and proposal trust defects; authored handler/entrypoint/regression fixtures await hosted execution. Register D052 threshold-one/post-review upgrade protection and extend D035/D042/D045. Add CLI chapter and a source-only165-registration/28-alias inventory; no fresh Click-resolved tree, oracle, interop or platform claim. Strict remains pending357 file mappings,186 export decisions,16 environment decisions,29 divergences and32 e2e mappings; D034 warning and M10/Q30 gate conflicts need reconciliation. No milestone accepted. |
| 2026-10-15 | M7 core-patch source preparation | Integrate reviewed hosted-only patch maintenance, pin/cache/lock guards, test-only API bans and separate effective patch provenance. Publish the already reviewed PR3242 head eafe84e8 unchanged as fork branch rtao-core-patch/v478 after validating its exact v478 parent and three modified signer files; no new commit or filing. A bounded free hosted dispatch prepares three candidate pin files for review, without fork credentials or pushes. Replay source is retained but not wired against the unpatched baseline; candidate adoption, testkit activation, API probes and shipped unpatched checks remain required. No local clone, worktree, build, temporary root or hardware run. |
| 2026-10-15 | Hosted maintenance-tool compilation repair | Report run38729491140 at a27e73e stops on two toml_edit Value method errors in the new core-patch Clippy guard reader. Resolve inline tables before reading fields. Keep the separately verified test-only constructor ban outside the unpatched release surface generator; its positive/negative probes remain owned by core-patch checks. No reports or tests succeeded in this run; fresh hosted verification remains required. No local build. |
| 2026-10-15 | M8 hosted report adoption | Run38729887190 at a8d2d1a passes hosted xtask/report generation. Verify/adopt exactly four regular report members and archive digests/size, clean source provenance and23 matching committed dependency hashes. All four role inventories/counts/host features remain unchanged. Matrix changes only register D052 and append D035/D042/D045 fixture names; no fresh runtime/oracle claims or generated-interface edits. Full integrated compilation/tests remain pending. D034 warning work is uncommitted and excluded from this run. No local build or temporary extraction. |
| 2026-10-15 | Hosted failures / reviewed core adoption | Run38730653363 passes semver/no-host but fails check/MSRV on three source defects and supply on active informational proc-macro-error2 maintenance notice; features remains live at inspection. Correct the two lints and nested picker path. Preserve the exact upstream notice and active Subxt parent chain with classifier regressions after confirming no compatible upstream fix or feature switch; retain raw audit findings. Run38730694470 passes preparation and patched/unpatched API probes. Verify/adopt its exact three-file artifact: only the reviewed core source identity changes in the lock; base/BLS/macros stay pinned. Wire source-only Ledger replay independently of Docker, strengthen its deadline fixture, and add a bounded hosted unpatched shipped-crate check. Full current workspace tests and acceptance remain pending; no local build, emulator, new filing or temporary extraction. |
| 2026-10-15 | Core graph report adoption | Run38732821845 at a035907 passes hosted report generation. Verify all four exact artifact members, archive digest, clean source and23 matching dependency inputs. Adopt only the reviewed core identity replacement across the unchanged225/361/365/416 role inventories; the declared matrix and runtime/oracle claims are unchanged. Older features job38730653363 is now terminal: retained template evidence confirms the same already corrected nested picker paths, without a new defect. Current workspace tests, the supply treatment and the unpatched shipped-crate check still require a full hosted run. Uncommitted parity-audit and D034 source are excluded. No local build or temporary extraction. |
| 2026-10-15 | Hosted core checkpoint corrections | Run38733328981 passes no-host/semver. MSRV identifies a crowd async ownership error; check reaches EVM/replay tests and finds a stable-only atomic API deprecation, resolver parameter inference and array/vector comparison. Correct the exact expressions, preserving the Rust1.89 atomic operation with a local compatibility annotation. Raw supply classification accepts the exact maintenance notice; cargo-deny then rejects the newly reviewed fork repository. Add that repository to the source allowlist; existing core-patch checks still require its sole package and immutable revision. Other live jobs remain under observation. No current test success, local build, new dependency version or milestone acceptance is claimed. |
| 2026-10-15 | M8 existing dependency edge preparation | Add direct Unix rustix filesystem access for the generic wallet Keyfile and nix terminal/poll APIs for injected masked input, using versions already locked; multisig activates its existing Tokio dependency to bound its advisory state read. Add a five-minute free hosted lock-only job and exact package/edge guards instead of fetching a second core repository onto the laptop. Candidate locks must retain all package identities and be reviewed before clean compatibility generation. Main run38734177643 passes MSRV/no-host/semver but main Clippy stops on eight source warnings before tests; their local corrections are not yet submitted. No local resolution, build, terminal rehearsal or milestone acceptance. |
| 2026-10-15 | M8 source audit and native API completion | Reconcile source inventories with exact fixtures:180 export decisions reviewed and one masked-input contract pending;147 of357 file rows reconciled,210 pending; all29 environment names have consumers or an explicit presentation decision. Correct the inherited cosmetic masked-input omission and retain codec/signal limitations as pending. Add generic public Keyfile/password APIs with explicitly owned environment mutations, native POSIX star editing and13 injected fixtures, ordered ABI enumeration, direct EVM map/key creation fixtures and Python argument previews. Independent reviews repair wallet fsync/diagnostic edges and identify D034 recipe-rebuild, stalled-read and null-opening defects; inspect reviewed bytes through known wrappers, bound the read and add exact regression sources. Correct observed Clippy warnings without weakening gates. Q30 source parity explicitly reports an absent first oracle as unverified while preserving history/corruption checks; only named optional M10 D031 is deferred during through-M9 strict checks. No current tests, oracle, terminal, platform or milestone acceptance claimed. |
| 2026-10-15 | Hosted dependency and validation evidence | Run38735786356 at f1b9b0d passes lock-only resolution. Verify/adopt its sole Cargo.lock artifact:850 unchanged package identities and only rtao-cli->nix plus rtao-wallet->rustix edges. Retain exact archive/file digests; no local fetch/build/extraction. Normal run38734177643 now finishes: features, including unpatched shipped crates, MSRV/no-host/semver pass, and all five Rust1.89 no-allocation integer-security probes pass. Main check stops on the corrected eight Clippy findings before tests; raw supply now reaches unmaintained smallstr RUSTSEC-2026-0215 through Subxt. Record upstream facts without suppression or a new filing. Fresh hosted generation and combined validation are still required for this source batch. |
| 2026-10-15 | M8 source-review corrections | Independent source reviews verify frozen-model producers and correct the freeze-hash wording. Reconcile ten more read/intent/CLI rows, with exact attribution headers, then masked input:158 of357 file rows reconciled,199 pending; all181 export decisions now have explicit native homes/decisions and fixture references. Under rule5 record D053 UTF-8/fixed-key terminal adaptation, fix escape-state cancellation and replace the Unix rpassword fallback with an owned hidden editor and restoration guard;18 injected fixtures remain unexecuted. Fix short bytes32 preview annotations, ambiguous remembered extension accounts, eager ignored bridge options, named-account rendering/classification and owner-parameter diagnostics. Expose StakeValuation::spot_value in the library and complete Python byte-item coercion. Activate existing D017 against exact certificate-size/error-order fixtures. Independently review the exact active smallstr maintenance policy: no compatible upstream release, full raw warning retained, changed advisory/identity/parent graphs still fail. Formatting passes for45 changed Rust sources; fresh hosted generation and combined tests remain required. No local build, temporary root, new filing or milestone acceptance. |
| 2026-10-15 | M8 hosted report adoption | Run38739615609 at e4322e6 passes report generation. Verify and adopt exactly18 regular source-report/generated members with both archive digests, clean source provenance and23 matching dependency hashes. The sole new role package is the reviewed rustix1.1.5 filesystem dependency: core/validator/miner/owner counts225/362/366/417; host flags remain false/false/false/true. All13 generated Rust files are byte-identical; extracted environment data changes only its reviewed policy counts. Matrix changes match reviewed file/export/environment decisions and D017/D034/D053; oracle changes are limited to three explicit source classifications and the new unverified divergence. No runtime/oracle success is added. Full current tests and security dispositions remain pending; cold review has identified additional client hardening work. No local build or extraction directory. |
| 2026-10-15 | Hosted checkpoint follow-up | Normal run38740705929 at94dfbf1 passes MSRV, no-host and semver; main Clippy stops on one needless borrow in EVM array-string parsing before tests. Remove that borrow without changing coercion. Supply accepts the exact smallstr maintenance notice and then exposes RUSTSEC-2024-0449 on unsafe-libyaml0.2.11 for investigation; no new policy is applied in this checkpoint. Features result is being recorded separately. Additional M9 review/fixes are uncommitted and excluded from this run. No local build or test success claimed. |
| 2026-10-15 | Hosted CLI and YAML-policy corrections | Normal run38742652282 at d7143f7 finishes: features, MSRV, no-host and semver pass; main check stops before tests on three incorrectly qualified EVM fixture helper calls, four lock-scope lints and two book-fixture lints. Correct those sources without weakening gates. Supply stops on unsafe-libyaml RUSTSEC-2024-0449. Inspect every raw finding and published parent features: Subxt unconditionally retains frame-decode legacy YAML support, with no compatible released parent migration. Add independently reviewed exact informational-maintenance handling with five package identities, three guarded parent relations, unchanged raw reports/statuses, actual-warning fixture and rejection regressions. Vulnerabilities remain subject to existing fail-closed/dormant rules. Combined hosted validation remains pending; the separate M9 source batch is still uncommitted. No local build, test, temporary directory, new filing or milestone acceptance. |
| 2026-10-15 | M8 parity evidence and M9 source hardening | Normal run38746145922 at5125fb9 passes all five supporting jobs, including supply; main compilation and strict Clippy pass, then parity stops before tests on a missing basket-producer attribution and renamed D044 fixture reference. Correct both. Source reconciliation reaches171/357 files,186 pending; all read product rows are reconciled. Reconcile the existing divergence register against actual fixture bodies and add the five missing evidence groups; only optional M10 D031 remains pending after registering D054-D060 and qualifying D021's leaf semantics. Author reviewed auth/body/notification/persisted-input budgets, cancellation/secret-reader repairs, recursive and deferred callback safety, exact RawU16 validation, public typed multisig adapters and bound continuation proofs. Independent review catches and corrects overflow retry reclassification, proof minting through a public wrap hook, dynamic-role continuation origin and a pinned-only shielding requirement; final corrected-source rereads and hosted execution remain pending. Add registration burn/lock disclosure, natural-unit hparam hints, three M9 source/audit reports and current guide limits. Fresh hosted generation and combined checks must precede any acceptance. No local build/test, temporary root, new filing or milestone acceptance. |
| 2026-10-15 | M9 hosted source-report adoption | Run38749149983 succeeds at clean4c9473c. Authenticate both ZIP identities/digests and exactly18 regular allowlisted members, all23 committed/current dependency hashes, unchanged role inventories225/362/366/417 and host selectionsfalse/false/false/true. All13 generated Rust files and extracted environment data are byte-identical. Compatibility changes only measurement provenance; runtime/oracle validation claims remain absent. Matrix changes are exactly15 reviewed file rows and35 divergence rows including seven new unverified D054-D060 entries; existing oracle classifications do not change. The file map now has172 reconciled rows and185 pending, including the newly completed public multisig adapter source. Retain bounded adoption proof and no archive/extraction directories. Full combined hosted tests and final security dispositions remain required; no milestone accepted. |
| 2026-10-15 | M9 hosted fixture compilation corrections | Normal run38750380241 at eefbfa2 passes semver, MSRV and supply at inspection; features/no-host remain running. Main check stops before tests on two Crowdloan descriptor-seeding type mismatches and V15 pallet metadata's singular error field. Correct the fixtures with typed state seeding and the actual metadata field, removing the unused descriptor import. Keep bounded compiler evidence in check-38750380241.json. Formatting/diff checks pass; corrected compilation, regression execution and final security rereviews remain pending. No local build/test, temporary directory, new upstream filing or milestone acceptance. |
| 2026-10-15 | M9 hosted signing-fixture lint correction | Run38751673871 at4f28dbf passes semver/MSRV/no-host/supply at inspection; features remains active. Fixture type errors are resolved; main Clippy stops before tests on signing-recorder type complexity. Introduce a fixture alias without suppressing the lint. Retain check-38751673871.json with full raw-log digest. Final independent source rereads clear the corrected dynamic-role continuation and pinned-only shielding guard; execution remains pending. Previous run38750380241 is terminal, with no-host passing and features superseded/cancelled. No local build/test, temporary directory, new filing or milestone acceptance. |
| 2026-10-15 | M9 source-parity follow-up | Run38753200118 at164247c passes semver/MSRV/no-host/supply at inspection; check/features remain live. Reconcile eight more source rows, reaching180/357 with177 pending; add nonlexical JSON-order fixtures after disproving an apparent dependency-feature gap. Strengthen complete Plan approval JSON and nine-field tool manifests, restore Python unknown-op quoting/sorted callable registry, and add exact registry attribution. Implement pinned subnet creation cost/queue disclosure and register D061 for accepted Yuma/Null setter examples after proving Python's numeric examples fail its own setter. Reconcile two ordinary read e2e bindings and the obsolete core tempo-rejection case, leaving29 pending. Add narrow proxy-codec/reveal payload+epoch extraction; independent review finds unchecked parameter/impl bindings, then source guards and five mutants correct them. Full chain rows and corrected-parser rereview remain unverified. Retain chain_rules.json through the exact hosted artifact allowlist; generated outputs need hosted regeneration/adoption. No local build/test, temporary directory, new upstream filing, current suite success or milestone acceptance. |
| 2026-10-15 | M9 report adoption and parity corrections | Run38754665115 succeeds at clean d65e5a8. Verify/adopt19 exact regular artifact members, both archive identities/digests and23 dependency inputs; role inventories and host selection are unchanged. Generated Rust changes only unknown-op delegation; chain rules add only reviewed proxy/reveal projections. Matrix changes match11 file rows,3 e2e dispositions and new unverified D061; no runtime/oracle claim changes. Run38753200118 completes with five supporting jobs passing and main compilation/strict Clippy passing; parity stops before tests on xtask license and platform-only D057 witnesses. Declare the combined Apache obligation and select existing unconditional regular-file/rotation pinning witnesses while retaining Unix FIFO/symlink fixtures. These preserve the gate rules. Fresh hosted compatibility/matrix generation is required after the manifest/register changes; no local build/test, archive/extraction directory, filing or milestone acceptance. |
| 2026-10-15 | M9 corrected parity-report adoption | Refresh38756143897 succeeds at clean877a39c. Verify/adopt its four exact regular members, archive identity/digest and23 committed/current dependency inputs. Only xtask's reviewed license input changes; role inventories and core host flags stay fixed. Matrix changes only D057's two unconditional pinning references; source statuses and oracle classifications remain unchanged. Preserve empty runtime-validation claims, bounded proof and no archives/extraction directories. Combined normal hosted validation is next; no local build/test or milestone acceptance. |
| 2026-10-15 | M9 ordering-fixture lint correction | Normal run38756773737 atd179b14 passes semver/MSRV at inspection; supply/no-host/features remain live. Main Clippy stops before tests on two unnecessary one-element slice clones in the Children ordering fixture. Borrow with std::slice::from_ref while preserving both assertions and later owned tool arguments. Retain exact failure/digest in check-38756773737.json; formatting/diff checks pass and no lint is suppressed. No local build/test, new filing or milestone acceptance. |
| 2026-10-15 | M9 optional-oracle source reconciliation | Review all 1,956 lines of the twelve core-py package/binding files and the exact tag/commit, frozen uv path dependency, locked maturin build and image-identity consumer chain. Verify every watched blob and retained artifact SHA256. Preserve oracle-only classification, reconcile these source inputs and retain unverified image/Python/differential/device/wheel coverage. Q30 already separates missing first optional outputs from client source acceptance; correct the prior map's conflation without changing any native product obligation or gate. Source rows reach 192/357, 165 pending; 29 e2e mappings remain pending. Bounded evidence is core-py-source-review.json. No Python, Docker, build/test, new filing or milestone acceptance. |
| 2026-10-15 | M9 optional-input report adoption and parser correction | Refresh38757888373 succeeds at clean303e631. Verify/adopt four exact regular report members, archive identity/digest and23 unchanged dependency inputs. Only measurement provenance and twelve reviewed oracle-only file dispositions change; classifications/runtime claims and role inventories remain unchanged. Previous normal run38756773737 completes with five supporting jobs passing; its two main Clippy warnings are corrected. Independent parser rereview closes the parameter/impl findings and identifies an omitted where-clause check; require absent added constraints and add one rejection mutant without changing current pinned output. Combined hosted execution remains required; no local build/test, extraction directory, filing or milestone acceptance. |
| 2026-10-15 | M9 intent/auth/lock source reconciliation and client-contract fixtures | Independently review complete sources/fixture bodies and reconcile ten exact blob/SHA256-pinned file rows, reaching202/357 with155 pending. Record existing native owner/window preflight as D062 and account-normalized btauth replay identity as D063. Correct the contradictory owner-list assertion and exact attribution. Repair lock contact-label JSON types and natural hierarchical alias selection, with independent rereview of equal-key ordering. Author five independently reviewed production-intent/Executor policy/fee fixtures retaining exact upstream amounts, call/fee probes and no private acquisition/submission. Bind only those client contracts with explicit unverified runtime fee limits; classify existing source mappings separately and preserve strict pending/oracle rules, leaving24 e2e rows pending. Gate integration independent rereview and hosted execution remain required; fresh matrix generation is next. Normal run38759055202 at4000bd0 passes five supporting jobs at inspection; main is still live. No local build/test, service, temporary directory, filing or milestone acceptance. |
| 2026-10-15 | M9 response-budget witness correction | Normal run38759055202 at4000bd0 finishes with all five supporting jobs, main compilation and strict Clippy passing. Parity stops before tests because D055 registers client::tests instead of the actual client::attempt_budget_tests module. Correct only the exact witness reference; preserve its oversized-response/timing assertions and unchanged gate. Retain check-38759055202.json with full raw-log digest. Refresh38761443333 at a9a7ea5 predates this register correction, so its matrix is superseded before adoption; fresh hosted generation and normal validation are required. No local build/test or milestone acceptance. |
| 2026-10-15 | M9 scoped-client report adoption | Refresh38761784282 succeeds at clean8cd8de5. Verify/adopt four exact regular report members, immutable archive digest/size and23 unchanged dependency inputs. Preserve role inventories/host selection and all compatibility claims. Matrix changes match ten reviewed source rows, new unverified D062/D063, D055's actual module path and explicit e2e source classes:72 intent plans,6 reads,5 client contracts,6 hosted scenario sources,2 inapplicable,24 pending. Preserve all115 keys and unverified runtime status. A bounded spelling inspection resolves all199 divergence functions including two manual path aliases; feature/harness checks and execution remain CI work. Retain bounded proof with no archive/extraction directory. Combined normal hosted validation and independent gate-integration rereview remain required; no local build/test or milestone acceptance. |
| 2026-10-15 | M9 workspace-witness and attribution corrections | Normal run38762528504 at60ff9a4 passes five supporting jobs, main compilation and strict Clippy; parity validates all115 e2e keys including five client contracts, then stops before tests on a false CLI-derived home and omitted xtask test discovery. Follow declared explicit workspace members/immediate-child globs while preserving exclusions, feature/harness/ignore qualification, source bounds and all witness assertions; add focused positive/refusal fixtures. Correct five mappings that counted consumers/producers as derived-code homes, retaining actual ports/literals and unchanged license rules. Move the single mandatory parity check before longer compiler/probe work without dropping gates. Complete native-balance/property source review under Q25/D035, reaching204/357 with153 pending; signed/arbitrary-size behavior and fresh oracle execution remain excluded/unverified as documented. State-only filing refresh still finds8 PRs+5 issues open at unchanged reviewed PR heads. Fresh hosted reports and combined execution remain required; no local build/test, temporary directory, filing or milestone acceptance. |
| 2026-10-15 | M9 workspace-witness report adoption | Refresh38766936693 succeeds at clean41f3838. Authenticate and adopt four exact regular reports, archive digest/size and23 unchanged dependency-input hashes. Matrix changes only two reviewed balance/property source dispositions and five corrected derived-code homes; source counts204/357 reconciled,153 pending. All115 e2e rows, oracle classifications, runtime qualifications, role inventories and compatibility claims remain unchanged. Retain bounded proof without an archive/extraction directory. Normal combined hosted validation and independent balance/gate rereviews are next; no local build/test, service, filing or milestone acceptance. |
| 2026-10-15 | M9 mapping-gate and Q30 audit follow-up | Independent review clears balance/property mappings, workspace discovery and gate ordering, then finds a combined hosted-target/evidence-removal escape for known client/read keys. Enforce their exact targets before either validator's hosted-prefix shortcut and add the precise indexed-target mutant; corrected source rereview passes, execution pending. Full resource-policy audit finds no remaining active costly local requirement; align stale filing/Ledger-recording wording and clarify the deleted task-owned cache, with independent rereview. Current normal run38767926902 atbbfa09a has supply/MSRV/no-host/semver passing at16:24 UTC, check/features active. Keep _money.py pending for generic unit-selector/generator coverage; all eleven current call sites trace correctly. No local build/test, service, temporary root, filing or milestone acceptance. |
| 2026-10-15 | M9 optional-divergence validation and transfer transcripts | Normal run38767926902 atbbfa09a passes four supporting jobs at16:28 UTC; main stops before Clippy/tests on an empty-witness check preceding optional M10 D031 deferral, with features still active. Restrict the empty-list allowance to exact pending/M10/addition D031; exercise the full strict validator and reject active/M9/class mutations. Independently reviewed transfer/batch fixtures retain upstream3TAO,1+2TAO and10^19rao, exact signed submissions, receipt decoding and pinned before/receipt reads over explicit state transcripts. Wrong-call/exhaustion negatives forbid fake default success. Keep all runtime economics/atomicity/rollback and mapping integration unverified. Formatting/diff checks pass; fresh combined hosted execution is next. No local build/test, service, temporary root, filing or milestone acceptance. |
| 2026-10-15 | M9 divergence-fixture compiler correction | Normal run38769405520 at6480626 passes every ordinary parity layer, including all63 divergence declarations and optional D031; main Clippy then catches the new fixture's undeclared tempfile reference. Reuse the production document validator directly from the full-register fixture, preserving all mutations without temporary files or a new dependency. Supply/MSRV/no-host/semver pass, features remains active at inspection; ordinary execution/docs remain unverified. Push this compiler correction separately while later source work is reviewed. No local build/test, service, filing or milestone acceptance. |
| 2026-10-15 | M9 money/CLI/test source batch | Authenticated source review and authored case completion reconcile five more rows, reaching209/357 with148 pending. Generic money selection consumes metadata-first identity/fallback tables, validates sibling NetUid sources and preserves native/root currency; six fee-filter counterparts use production plans and independent SCALE with explicit serving test support. Root-claim tests preserve all31 cases in18 bodies, but both rows remain pending authenticated table extraction. Repair collateral required/help details and retain complete ordered JSON/human fixtures; disclose human-list repair as D064 and native30-second diagnostic phase bounds as D065, with actual timeout/cancellation/closure fixtures. Bind three exact transfer/batch client transcripts with explicit unverified runtime limits, leaving21 e2e keys pending. Three separate client/config/facade source findings remain open. Formatting/diff and source reviews pass; reports need hosted refresh/adoption and combined compilation/execution. Normal237d189 run38770765426 remains separate and active. No local build/test, service, temporary root, filing or milestone acceptance. |
| 2026-10-15 | M9 client-source report adoption | Refresh38771543517 succeeds at cleanddd73e5. Verify/adopt four exact regular reports, immutable archive digest/size and all23 committed/current dependency inputs; only the reviewed serving test-feature manifest hash changes. Role inventories/host flags stay fixed. Matrix differences match five reconciled rows, two corrected pending root-claim homes, three qualified lifecycle bindings and new unverified D064/D065. Counts209reconciled/148pending files and21pending e2e keys; existing oracle classifications and compatibility claims are unchanged. Retain bounded proof without archive/extraction directories. Normal237d189 run38770765426 main/features remain active at16:49 UTC with four supporting jobs passing; retain it for the ordinary outcome before dispatching normal CI on the report-adoption head. No local build/test, service, filing or milestone acceptance. |
| 2026-10-15 | M9 client-boundary repair | Enforce Python's4,300-digit cache/custom-payload string limit, normalize empty weights-helper wallet names and preserve connected nonroot balance symbols in both blocking factories without cold work/RPC. Independent rereview catches and verifies ROOT's absent-symbol metadata rule with a seeded root record. Full file-map rows remain pending; direct formatting/whitespace pass, hosted execution unverified. At17:02 UTC normal237d189 run38770765426 has all five supporting jobs passing and main active. Preserve that outcome before the next normal dispatch. New source review identifies checkpoint temporary-name ownership, closed-client RPC and per-extrinsic block-decode parity gaps for focused follow-up. No local build/test, new scratch directory, filing or milestone acceptance. |
| 2026-10-15 | M9 ordinary failure correction and source inputs | Normal `237d189` run38770765426 passes all five supporting jobs, main compilation/strict Clippy/ordinary parity, then nextest reports 2,831 run: 2,784 pass/47 fail/4 skip; doctests/docs not reached. Retain authenticated bounded failure evidence and fix every observed cause without weakening safety assertions. Correct digest display contexts, result hash JSON, modern preimage length, Vault replacement close, upgrade repo validation and decrypt title stream; repair fake duplicate request recording and stale fixture assumptions. Continue reviewed closed-client/block-decode/EVM-text/help corrections. Three source rows reach 212/357 with 145 pending; 21 e2e keys pending. Add authenticated all 102 read sample fixtures and exact ordinary intent summary/fee checks. Add narrow root-claim AST/test-input projection plus allowlisted hosted output, keeping both full rows pending disclosed remaining adaptations. Hosted generation/adoption must precede ordinary execution. Source/formatting checks pass; no current green test, milestone exit, local build/test, new scratch root or upstream filing. |
| 2026-10-15 | M9 hosted generation compiler correction | Generation38776446102 at8112797 stops before reports on unconstrained generic request inference in hash_for_number. Add explicit serde_json::Value type before is_null, preserving cancellation and RPC semantics. Retain bounded exact-source log proof; no artifact/test/milestone acceptance. Direct formatting/whitespace checks only; retry hosted generation before ordinary execution. |
| 2026-10-15 | M9 ABI decimal boundary follow-up | Root review finds significant-width normalization alone accepts decimal leading-zero strings beyond Python's default4,300-digit limit. Count all decimal digits while excluding sign/whitespace/separators; keep hexadecimal exempt and all ABI/native bounds. Extend the public coercion fixture across4,300/4,301 ASCII/Unicode digits and exempt long hex. Direct formatting/whitespace pass; no executed claim. Current hosted generation atd7b4a27 is independent of this source-only change. |
| 2026-10-15 | M9 constructor unit generation correction | Generation38776671979 atd7b4a27 compiles/extracts but regeneration exposes generic helper-table validation preceding a custom Crowdloan resolver. Select custom resolvers first. For ordinary constructors preserve authenticated identity/fallback precedence, then recognize an explicit metadata-mapped native balance unit through compact wrappers; unknown untyped u64 still refuses. Add real pinned-metadata Crowdloan argument regression without copying a fallback table. Retain bounded log proof; no adopted artifact, passing test or milestone acceptance. Direct formatting/whitespace pass; retry hosted generation. |
| 2026-10-15 | M9 fixture/report adoption | Generation 38777439872 succeeds at clean 5464ccf. Authenticate two immutable archives, 20 exact regular members, 23 unchanged dependency inputs and the new 22,297-byte fixture's source provenance/case inventory. Other generated interfaces stay byte-identical; only three source matrix rows change, reaching 212 reconciled/145 pending with 21 e2e keys pending. Existing e2e/oracle/compatibility claims and role inventories/host flags remain unchanged; record the hosted CPU as provenance. Replace test unwraps with checked errors without relaxing lints/assertions. Retain bounded adoption proof without archive/extraction directories, then push normal combined CI. No current green tests/docs, local build/test, filing or milestone acceptance. |
| 2026-10-15 | M9 named-batch and CLI boundary follow-up | Normal64fef73 run38778912578 main stops before tests on three fixture/compiler/Clippy errors; four supporting jobs pass, features is superseded after main failure. Push262be70 correcting the literal type, test-item order and needless clone without lint exemptions. Independently reviewed cold symbol properties now avoid worker/connection/RPC, and named empty batches skip metadata after blocking connection/lifecycle checks. Native immutable snapshots avoid Python numeric-height re-resolution. Strengthen duplicate/default batching and cache-copy/closure fixtures. Repair timelock file whitespace and empty output paths; help and policy findings remain pending. Four complete source rows reach216/357 reconciled with141 pending; result/_substrate rows and21 e2e keys stay pending. Generated reports and hosted execution of this follow-up remain unverified; current normal262be70 run38780323094 stays separate. No local build/test, service, temporary root, upstream filing or milestone acceptance. |
| 2026-10-15 | M9 finite input and serving evidence | Extend the existing authenticated root-claim projection with seven arithmetic groups, eligibility overrides and explicit helper threshold; native consumers preserve independent fee expectations and both uncapped comparisons. Root independent source review finds no surviving issue; keep both whole source rows pending hosted generation/adoption. Bind complete extracted EVM presets to both native consumers and register the four-pair WebSocket alias table for hosted extraction. Add exact two-step TLS/Prometheus client submission transcript with hotkey-only signing, independent calls, nonces, explicit receipts and changed-certificate/exhaustion refusals. Independent review clears the fixture after correcting a stale request-count comment and setup cleanup. Bind its exact ordinary test and finite coverage limits in both e2e validators, leaving20 pending keys; runtime admission, endpoint/certificate persistence and TLS/reachability remain unverified. Source216/357 reconciliation remains unchanged. Formatting/whitespace pass; no executed tests, local build, service, extraction directory, filing or milestone acceptance. |
| 2026-10-15 | M9 diagnostic normalization and security linkage | Repair both oversized custom-code regex branches with typed InvalidCustomCode diagnostics and stop subsequent fallback, preserving structured conversion precedence. Unknown numeric RPC codes still retain exact attempted-hash unknown fate; known pool codes remain definite refusals. Independent source review and root reread clear ASCII/Unicode boundary, precedence and real-submission fixture sources; tests remain unexecuted. Complete authenticated WebSocket alias consumers against independent extracted presets for absent/empty/custom environment values; their new table awaits generation38781345962 atb2f9483. Record exact ten security-fix commit links and regression locations, preserving open/execution-pending dispositions and unaccepted M2-S02. Normal262be70 run38780323094 remains separate with four supporting jobs passing at observation. No source-row promotion, local build/test, service, scratch directory, filing or milestone acceptance. |
| 2026-10-15 | M9 named-table output retention | Source review catches that the new EVM alias projection changes named_tables.json, but the generation workflow permits/uploads only the previously changed extraction outputs. Add this exact regular-file path to both codegen allowlist and artifact upload. Supersede generation38781345962 with a bounded run on the corrected workflow; ordinary262be70 CI remains separate. Archive adoption must now verify17 generated members plus four reports, including the authenticated alias table. No local generation/build/test or passing-run claim. |
| 2026-10-15 | M9 named-batch trait correction | Generation38781345962 atb2f9483 fails during rtao-client compilation before extraction: moving the helper into view.rs brings both View and DynView storage_raw_many methods into scope. Select View explicitly, retaining the identical pinned request/decoder path. The dependent never-type diagnostics share that single ambiguity. Retain bounded immutable log proof; supersede8488341 generation38781939785 with the corrected source. The named_tables output-retention correction remains necessary but was not the observed first failure. No artifact adoption, passing-test or milestone claim; formatting/whitespace only locally. |
| 2026-10-15 | M9 finite fixture/report adoption | Generation38782055157 succeeds at clean30f6369. Verify two immutable archive digests/sizes,17 generated plus four report members, and23 unchanged committed/current dependency inputs; adopt only seven changed files. Named tables and generated Rust add exactly the four-pair WebSocket alias record. Root fixture preserves every prior record and adds seven model groups/nine calls, two eligibility overrides, allowance inputs and explicit helper threshold. All other generated interfaces stay byte-identical. Matrix changes exactly four reviewed file rows and the qualified serving client transcript, reaching216/357 reconciled with141 pending and20 pending e2e keys. Oracle/runtime/compatibility claims and role inventories/host flags stay unchanged; report only the hosted CPU/source/time provenance change. Retain bounded proof without archive/extraction directories. Normal262be70 run38780323094 remains active; combined tests for this adopted source are not yet claimed. No local build/test, service, filing or milestone acceptance. |
| 2026-10-15 | M9 finite-input source reconciliation | With authenticated30f generation outputs adopted, reconcile complete root-claim production/test rows and EVM networks against the prior full source reviews, finite-input follow-up, actual consumers and preserved independent expectations. Reconcile result.py after independently reviewed typed regex-failure and fate-boundary repair. Authenticate all four source blobs/hashes; record native helper/fixture/exception/environment representations and pending substrate failure-event construction separately. Source counts220/357 reconciled,137 pending;20 e2e keys remain pending. These are source dispositions, not current green tests or fresh oracle execution. Refresh declared compatibility/parity reports in hosted CI while preserving ordinary262be70 outcome. No local build/test, service, filing or milestone acceptance. |
| 2026-10-15 | M9 reconciled-source report adoption | Refresh38783800344 succeeds at cleana276a83. Authenticate the54,663-byte immutable archive and four exact regular reports; verify23 unchanged dependency-input hashes, unchanged225/362/366/417 role inventories/host flags and all compatibility/oracle claims. Matrix changes only the four newly reviewed root-claim production/test, network and result rows; source counts220/357 reconciled with137 pending,20 e2e keys pending. Adopt exact bytes and retain bounded proof without archives/extraction directories. All five supporting jobs of normal262be70 run38780323094 pass; main remains active. Its final ordinary outcome and combined validation on this later source remain pending. No local build/test, service, filing or milestone acceptance. |
| 2026-10-15 | M9 failed facade event construction | Match pinned _substrate._result_from_report by omitting triggered events from failed ExtrinsicResult construction, while low-level reports and pinned event APIs retain receipt records. Strengthen existing real-executor success/failure fixtures; failed-batch transcript still checks the actual decoded failure event through its exact pinned receipt block and now separately asserts empty facade events. Preserve error, identity, fee/data, nonce/attempt and state assertions. Root source review and direct formatting/whitespace pass; independent rereview and hosted execution remain pending. Do not promote the full substrate row. Normal262be70 run38780323094 remains active with all five supporting jobs passed; no local build/test, service, filing or milestone acceptance. |
| 2026-10-15 | M9 three remaining hosted failures and receipt evidence | Normal262be70 run38780323094 passes all five supporting jobs, main compilation/strict Clippy/parity, then nextest runs2,878:2,875 pass/3 fail/4 skip; doctests/docs not reached. Preserve authenticated bounded log and exact failure evidence. Correct collateral fixture to seed genuine optional UID absence, retain complete JSON/order/registered UID assertions; correct conviction hint stream/singular/source option spelling with exact stderr and --all suppression assertions. Fix recovery block-hash parsing for canonical result hex plus legacy32-byte arrays, preserving strict schema/width/finality/group/timepoint checks and adding malformed cases. Following actual native receipt consumers disproves284ce40 event suppression as a safe implementation: CLI multisig and pruning need failed execution evidence. Restore prior diagnostic retention, explicitly extend D-023 and compare success/failure event vectors with actual pinned records; Python JSON remains unchanged. Historical284ce40 omission proposal is superseded. Root source review/formatting/whitespace pass; final independent rereview and current-source execution pending. Refresh reports then run normal CI on the combined source. No local build/test, service, filing or milestone acceptance. |
| 2026-10-15 | M9 receipt-evidence report adoption and combined run | Refresh38786488779 succeeds at cleanfc91b36. Authenticate the54,731-byte archive/four regular reports and23 unchanged dependency inputs; role inventories/host flags and all compatibility/oracle claims stay unchanged. Matrix changes only D-023 by adding the two reviewed failed-receipt regression targets;65 divergence identities,220 reconciled/137 pending source rows and20 pending e2e keys remain fixed. Adopt exact reports with bounded proof and no retained archive/extraction directory. Prior normal262be70 outcome is captured:2,875 pass/3 fail/4 skip, all five supporting jobs pass. The three causes are corrected in source; start normal combined validation on the current complete adopted source, including the query-batch/cold-symbol, root-input/network, serving, regex, recovery and diagnostic-retention follow-ups. No current green test/milestone, local build/test, service or new filing is claimed. |
| 2026-10-15 | M9 client boundary and source reconciliation | Normal97090f2 run38787018268 passes all five supporting jobs but stops before tests on a combined attribution header lacking the full _subtensor.py path. Split canonical headers without weakening the gate; retain exact log/source proof. Restore timelock command/prompt guidance; add async snapshot sibling/symbol/balance helpers and explicit eager-sibling lifecycle notes; extend existing D-007 documentation to derived slot timing. Preserve exact classifier policy payloads in shared storage while sanitized diagnostics remain unchanged. Repair Python edge whitespace for EVM artifacts/secrets and bridge tokens, CRLF/bare-CR artifact and successful macOS process text, scalar JSON array coercion and pre-allocation outer-array item checks. Independently reviewed source/fixtures retain u64 chain amounts, secret erasure, no-RPC cached helpers and existing tuple/refusal semantics. Complete eight source rows reach228/357 reconciled with129 pending and20 pending e2e keys; strict M8 remains off. Authenticated prior selected execution proves70 S9 regression PASS records with29 unchanged boundary/test files, without treating the failed prior run as C1 or closing findings. Refresh reports and run combined hosted checks for this batch; no local build/test, service, temporary root, upstream filing or milestone acceptance. |
| 2026-10-15 | M9 client-boundary report adoption | Refresh38789498154 succeeds at clean56469a0. Authenticate55,264-byte immutable archive/four regular reports and23 unchanged dependency inputs; matrix changes exactly eight reviewed file rows, reaching228 reconciled/129 pending with20 pending e2e keys. Role inventories/host flags, all non-build-weight compatibility fields and oracle/runtime claims remain unchanged. Graph differences are only measured CPU/time and source provenance. Adopt exact reports with bounded proof and no archive/extraction directory, then run combined hosted validation. Latest ordinary97090f2 result remains the attribution refusal before tests with all five supporting jobs passing. No current test/milestone acceptance, local build/test, service or new filing. |
| 2026-10-15 | M9 complete attribution preflight and signer corrections | Normalda99c3d run38790070632 passes all five supporting jobs but again refuses attribution before tests. Audit every reconciled direct Rust home and exact mapped path across the workspace; remove reused dependencies from derived-home lists while retaining their documented behavior dependencies, add actual canonical config/result/root-claim origins, and emit authenticated named-table origins outside the generated array so formatting preserves them. Keep the license checker intact and add source-parity verification before refresh artifact upload. Share the existing browser selector/launcher between Vault and extension, removing Vault's ten-second wait/kill behavior; a bounded pipe-child fixture checks lifetime/process-group separation without launching a browser. Enable only the already-declared Unix nix dependency for Vault discovery. Correct the factual interpretation of qrcode optimize=0 under rule5's Python-behavior principle: one numeric/alphanumeric/byte segment without splitting, preserving ECC M/quiet zone4 and binary UOS byte mode. Independent SVG decoder fixtures cover all mode/capacity boundaries. Eight named natural-name/wallet-compat fixtures retain exact substantive cases plus explicit native API dispositions. Eight more source rows reach236 reconciled/121 pending with20 pending e2e keys; strict M8 stays off. Root/source reviews and direct formatting pass; generated catalog/reports and combined hosted tests remain pending. No local build/test, service, temporary root, new filing or milestone acceptance. |
| 2026-10-15 | M9 manual CI parity baseline | Refresh38793856089 at5774702 passes attribution, extraction, symbols, catalog, chain, coverage and matrix checks, then refuses pending/oracle validation because workflow_dispatch has no push before SHA. Preserve the exact failed run/log and adopt no artifacts. Add explicit manual-event handling bound to GITHUB_SHA and the exact checked-out HEAD, compare its immutable first parent and retain complete hardened oracle history; malformed, mismatched and unavailable-parent identities still fail closed. A source fixture commits/removes historical evidence and checks that an explicit older base and replacement refs cannot hide it. Existing push/PR/merge-group validation is unchanged. Independent source review and direct formatting/whitespace pass; hosted execution remains pending. No local build/test, service, temporary root, filing or milestone acceptance. |
| 2026-10-15 | M9 timelock contracts and recorded codec fixtures | Restore Python callback source-conflict errors, input-acquisition priority, pure duration-prompt grammar and public ciphertext replay while plaintext stays omitted under D-042. Preserve reveal-option parser qualifications and register the existing total nonnegative native decryption deadline as D-066; zero starts no attempt and running blocking crypto can finish independently. Independent source reviews clear the repairs. Add complete recorded immortal payload-parts and block-extrinsic dictionary fixtures under V14/V15; add full ordered shape-corpus decode_batch, below-threshold and boundary checks without replacing scalar tests. Reconcile complete reads-table, exact v446 accounting/failed-commitment and shape-corpus source rows after authentication and actual-origin repair, reaching239/357 reconciled with118 pending;20 e2e keys stay pending and strict M8 remains off. Hyperparameter and other codec/property assertion gaps stay pending. Direct formatting/whitespace and attribution preflight pass apart from the expected generated catalog origin awaiting hosted refresh. New fixtures remain unexecuted. No local build/test, service, temporary root, filing or milestone acceptance. |
| 2026-10-15 | M9 facade/prompt source contracts | Preserve both exact three-field stake prompt fixtures and add compiler-checked native blocking witnesses for all34 Client/11 Snapshot operations. Authenticated Python AST completeness and generated/curated namespace correspondence guards reject missing operations and parameter-order drift while documenting typed owned arguments, native pure helpers and dynamic-proxy dispositions. Cold finney construction/close checks identity without starting a worker. Independent source review clears the four-file checkpoint; reconcile the two complete test modules to241/357 source rows with116 pending. New fixtures/guards remain unexecuted pending hosted CI; no milestone acceptance, local build/test, service, temporary root or new filing. |
| 2026-10-15 | M9 authenticated generation adoption and combined checks | Refresh38796559286 succeeds at cleane8c0fc7, including the repaired manual baseline and full ordinary source-parity check. Authenticate and re-fetch two immutable archives (269978/55847 bytes), all17 generated plus four report members and23 committed/current dependency inputs; adopt exactly five changed files. Generated catalog changes only canonical named-table origins, with36 column-zero headers and all non-origin data unchanged;16 other generated members are byte-identical. Matrix changes exactly17 reviewed file rows plus the unverified D-066 entry, reaching241 reconciled/116 pending with20 pending e2e keys. Role inventories225/362/366/417 and host flags stay unchanged; the only dependency-input change since the old graph is the reviewed existing Unix nix edge for Vault. All non-build-weight compatibility/oracle/runtime claims remain unchanged. Retain bounded exact-byte proof without archives/extraction directories. Full attribution preflight now has zero missing/invalid origins. Push the adopted source to start combined hosted tests; no current passing-test/milestone, local build/test, service or new filing is claimed. |
| 2026-10-15 | M9 exact fixtures and Vault display context | Combined run38797979277 at e1dca323 stops at strict Clippy before tests: one fixture guard crosses an await, nine facade witness types trigger complexity warnings and the cold-construction test returns a large error. End the guard lexically, name reusable type aliases while retaining all45 typed function pointers, and box only the test error; independent review clears these unexecuted corrections. Complete all15 hyperparameter source-test mappings and28 slippage cases with exact registry/build bytes; only the hyperparameter rejection diagnostic changes production behavior. Complete authenticated Vault UOS/server/signer review and repair the missing CLI summary caller: reviewed semantic/raw/per-round text stays scoped to each lazy source and reaches Vault before warm-up. Source-isolation, raw Sudo, page-text and five explicit/manual-stage requests pin display behavior without changing authorization or signed bytes. Record existing native session admission/lifecycle policy as D-067, with no exploit claim or new filing. Five source rows reach246/357 reconciled,111 pending,20 pending e2e mappings and67 divergence identities; strict M8 remains off. Independent source reviews, direct formatting/whitespace and attribution preflight pass; new fixtures remain unexecuted and reports await hosted refresh. No milestone acceptance, local build/test, service, device work or temporary root. |
| 2026-10-15 | M9 exact report adoption and hosted validation | Report-only refresh38801055077 succeeds at clean0f69e6d, including complete source parity and generated-output checks. Authenticate and re-fetch immutable artifact12005075143 (56192 bytes; SHA2561f34f5149fef81ca52a2874556286105dfbc6bfdd66bba3dae4a6bf2bc1cdbf9), verify exactly four report members and all23 unchanged dependency inputs, then adopt only those four reports. Matrix changes exactly five reviewed file rows and adds only unverified D-067; all357 generated file rows match the committed map. Role counts225/362/366/417, host flags and all non-build-weight compatibility/runtime/oracle claims remain unchanged. Retain bounded hashes/proof without archives or extraction directories. Prior combined run38797979277 finishes with features, semver, no-host, supply and MSRV passing; its main Clippy failure remains recorded before tests. Push the reviewed source/report checkpoint to start ordinary combined hosted checks, without a duplicate dispatch.246 reconciled/111 pending,20 pending e2e keys, strict M8 off and zero milestone acceptance remain unchanged. No local build/test, service, temporary root, device work or new filing. |
| 2026-10-15 | M9 extension, timelock and package source closure | Preserve normal run38802337753 at 8fcab84: all five supporting jobs pass while the main check remains live. Repair extension remembered-duplicate blank selection, Python integer grammar, empty stage omission, crypto edge whitespace, available-account hints and replacement close handling. A deterministic closed-queue fixture forces the missing select branch; the original end-to-end cancellation/new-tab fixture stays single-run. Restore injected readiness notices with manual session URL, five-second throttling and JSON/quiet suppression; native callbacks remain inline without their own timeout. Record existing extension local I/O/process/signature-admission policy as D-068 without an exploit claim or new filing. Timelock unit parsing uses Python 3.12 compensated sums, sealing admits native rounds before datetime materialization, missing-input errors retain exact help, and docs name the actual async API. Optional replay now calls public drand helpers. Verify the exact singleton Python CLI script entry without changing compatibility output; correct root export homes and document language/feature/logging/version dispositions. Complete source reviews reconcile 14 rows to 260/357 with 97 pending, 20 pending e2e keys and 68 divergences; codec oracle-only rows retain their missing native property/assertion coverage explicitly. Independent review corrected two test descriptions and clarified that decrypt may fetch public beacon signatures. Source reviews, direct formatting/whitespace and attribution preflight pass; all new fixtures remain unexecuted and generated reports await hosted refresh. Strict M8 stays off; no milestone acceptance, local build/test, node, service, device work, temporary root or Mac Studio activity. |
| 2026-10-15 | M9 portable divergence fixture identities | Refresh38805471164 at5d3edcf passes every source-parity layer except divergence eligibility: D-068 incorrectly lists a cfg(unix) publication fixture as a required pinning identity. Retain its exact 58736-byte failed log hash78b0d6ca13a0a6d8cecefadcd62a30c6a5bd2bfb7cff9bdd5d52dcab84f8f86e and adopt no outputs. Correct the register to list existing unconditional transport, permission, deadline, no-spawn and signature cases; retain both Unix publication/foreground fixtures unchanged as supplementary platform coverage. The parity checker, test bodies and cross-platform coverage limits stay intact. Retry only the bounded hosted report refresh; preserve normal38802337753 and its live main check. No test pass, milestone acceptance, local build, service or new filing is claimed. |
| 2026-10-15 | M9 source-report checkpoint and ordinary test results | Report refresh38806339150 succeeds at clean67a7a7b, including source parity and generated-output paths. Authenticate and independently re-fetch artifact12006811683 (56707 bytes; SHA25646c9a097aa698dfe059cc4875e33fcce62809a5e4fbf997fa9e54ed85ab42f11), adopt exactly four report members, and verify all23 unchanged dependency inputs. Matrix changes exactly14 reviewed file rows plus unverifiedD-068; all357 file rows match the committed map. Role inventories225/362/366/417, host flags and all non-build-weight compatibility/runtime/oracle claims remain unchanged; no archives or extraction trees retained. Ordinary run38802337753 at8fcab84 passes all five supporting jobs and reaches2934 tests:2932 passed,2 failed,4 skipped. Both failures are serving lifecycle fixture receipt checks; preserve exact797701-byte log hashc1c7670581a0c2510c7e36213694a33f8e45270c37c9c6175c1e52bc9f217ac0 and repair before the next combined run. Later extension/timelock/package fixtures still require combined execution.260 reconciled/97 pending,20 pending e2e keys, strictM8 off and no milestone acceptance remain explicit. No local build/test, service, device, temporary root, Mac Studio activity or new upstream filing. |
| 2026-10-15 | M9 hosted serving-fixture repair | Trace both receipt failures in38802337753 to guarded_author re-encoding a five-field decoded call display record with an encoder requiring a single-key runtime enum. The valid call reaches that error before submission/receipt advancement; changed-certificate rejection short-circuits it. Independent review verifies the active pinned codec and strict complete extrinsic framing. Retain decoded equality and compare the actual trailing call bytes with independently composed source literals; preserve every account, nonce, nonzero signature, receipt/event/status, changed-certificate and exhaustion assertion. Improve missing-receipt diagnostics with bounded transcript violations. Direct rustfmt, patch/source hashes and whitespace pass; no tests run locally and the correction remains unexecuted until the next hosted combined run. Reports/source counts and security findings stay unchanged; no milestone acceptance or new filing. |
| 2026-10-15 | M9 wallet source API dispositions | Under execution rule 5 and sections 8.1/8.3, record the complete wallet.py/wallets.py source reviews and independent eligibility decision in owning architecture, book, rustdoc, exact map homes and reasoned root crypto-helper export records. Native role writes reuse configured PasswordSources with caller-owned callback confirmation; automatic Wallet save-to-environment flags are omitted in favor of separate owned Keyfile saving and CLI export. Per-role typed, nonprinting creation/recovery and CLI-only inventory replace Python convenience placement. Correct the adjacent once-per-process/unconditional pair-zeroization wording: caches are per shared source, and M2-S02 remains open and unaccepted. Reconcile only the two authenticated source relationships; current hosted/Python/oracle execution and milestone acceptance are not claimed. No local build/test, service, temporary root, hardware work or new filing. |
| 2026-10-15 | M9 client waits, receipt weights and multisig diagnostics | Under rule 5, expose the promised async block/timestamp/epoch waits and share their algorithms with the blocking facade; preserve blocking error variants and EpochEvent re-export, document native None versus zero budgets, and correct the stale BlockInfo recovery claim. Four FakeNode fixture bodies cover real observed headers/epoch changes/chain time and finite/zero/indefinite budgets. Preserve decoded receipt weight as public DynValue (pool null, exact included Outcome value); two actual-client fixtures cover success/direct/nested failure, large integers and retained D-023 events. Fix typed raw/semantic CLI funding-remediation transport, append D-041 recovery help and preserve Python empty-label truthiness; four substantive funding/CLI fixtures retain pre-signing refusal and opaque S9 protections. All three patches received independent source review; direct formatting/whitespace and attribution preflight pass, with no execution claim. Restore extension group guide help and reconcile nine complete authenticated storage/client/multisig/wallet/extension production/test source relationships:269/357 reconciled,88 pending. Extension/storage pytest rows stay oracle-only with exact assertion/API gaps recorded; root crypto exports have explicit typed/CLI dispositions. Full contract.py remains pending beyond the narrow weight repair. Preserve ordinary38807792907 at7877bc6 while refreshing this later source in the separate hosted report lane. Generated reports and combined tests for these later changes remain pending; strict M8 stays off, M2-S02 open/unaccepted and zero milestones accepted. No local build/test, temporary root, node/service/device work, Mac Studio activity or new filing. |
| 2026-10-15 | M9 hosted oracle caller compilation repair | Ordinary38807792907 at7877bc6 passes source parity and four supporting jobs, then its main all-target Clippy stops at E0277 before tests: hosted_oracle.rs still awaits the timelock replay helper made synchronous in5d3edcf. Remove the one stale await and confirm both actual replay callers match the synchronous signature; retain the immutable-oracle ignore requirement. Exact68815-byte failed log SHA256a507510b08fb3581eb0c5d5db29e189128a19fe851b547b364d550322daa3303 is retained. Feature validation remains live and is preserved. Direct formatting and whitespace pass; no new ordinary test or serving-repair execution result is claimed. |
| 2026-10-15 | M9 client/report integration checkpoint | Refresh38808984276 succeeds at clean d872032, including refreshed source parity and generated-output paths. Authenticate and independently re-fetch artifact12007288779 (57101 bytes; SHA25605fe692f3a1c8ad31afd7b5ff712d514979e91d1c2685f47a6e2d818631e8464), verify exactly four regular allowlisted reports and all23 unchanged dependency inputs, and adopt only those reports. Matrix changes exactly the nine reviewed source rows and root parse/format crypto dispositions from module_map to decision; all357 file rows match the committed map. Role inventories225/362/366/417, host flags and non-build-weight compatibility/runtime/oracle claims remain unchanged; no oracle status changes or archives/extraction directories retained. Prior normal38807792907 ends with all five supporting jobs successful and the main stale-await compilation failure already fixed in this source; no newer tests ran there. Push one ordinary combined CI checkpoint for all integrated changes.269 source rows reconciled/88 pending,20 e2e keys pending,68 divergence identities, strict M8 off and zero milestone acceptance remain explicit. Last actual ordinary result remains2932 passed/2 failed/4 skipped at8fcab84; serving fixture repair and later additions await hosted execution. Local target remains547192 KiB with no new build output or temporary root; no Mac Studio/service/device work or new filing. |
| 2026-10-15 | M9 transport, signing and Ledger source dispositions | Adopt complete authenticated receipt-utils, signing, transport-contract, extrinsics, runtime-manager and Ledger reviews with concrete source homes and canonical attribution. Preserve native async capabilities, immutable signing views, exact 17-field unsigned JSON, checked u64 balances, strict import, richer internal IR, parent-selected metadata and D-008/D-023/D-024/D-026 protections. Record the two public unsigned/context namespaced exports, optional unverified interop and specific cache/device/fixture limits; correct stale fallback and cache-identity wording. Reconcile six source rows to275/357,82 pending,20 pending e2e mappings and68 divergences; generated reports await hosted refresh. Add a real authenticated AST producer for all13 frozen legacy runtime API declarations, guarded callback policies and two actual V15 byte-output fixtures. Move the old lookup verbatim into an explicit bootstrap include because xtask depends on the client; hosted extract must replace that seed before exact output checks or new fixture results can pass. Extend only the existing free-runner generated-artifact allowlist/upload. The runtime_api.py source row stays pending until that handoff is verified. Independent source reviews and direct formatting/whitespace pass; no local builds/tests or new oracle/device claims. Preserve useful ordinary run38809901640 at55f7ec0, whose five supporting jobs pass while its main gate remains live; use a skipped-CI source push and the separate bounded generation lane. Strict M8 remains off, M2-S02 open/unaccepted and no milestone accepted. Local target remains547192 KiB with no new build output, temporary root, Mac Studio work or new upstream filing. |
| 2026-10-15 | M9 authenticated runtime-API output adoption | Refresh38812061254 succeeds at clean1b9ecd5. Independently fetch and authenticate the generated artifact12010100050 (270802 bytes; SHA256b588021240dae0f98d8398c8fb826c0a5b4a9f01d0f433f2c53541f80a1da607) and report artifact12009092627 (57238 bytes; SHA256bb5eb8cbaf1e46b9e3d13b04244647d1c24f6c3035151f9cf594a7da17960e1e) twice. Verify all18 generated members:17 are byte-identical to committed consumers, while the real3438-byte runtime lookup replaces its explicit bootstrap seed and all13 contracts match the complete authenticated source inventory. Adopt only that include and four reports, with all23 dependency inputs unchanged. Matrix changes exactly six reviewed source rows and two concrete export dispositions; role counts225/362/366/417, host flags and other compatibility/runtime/oracle claims remain unchanged. Retain bounded proof without archives or extraction directories.275/357 source rows are reconciled,82 pending,20 e2e mappings pending and68 divergences; strict M8 remains off and M2-S02 open/unaccepted. Preserve ordinary38809901640 and its live main gate; all five supporting jobs have passed. Exact-source extractor/new-fixture execution and the serving repair still await the ordinary hosted gate. No local builds/tests, new temporary root, Mac Studio work, device work, filing or milestone acceptance. |
| 2026-10-15 | M9 ordinary test success and private-link repair | Ordinary38809901640 at55f7ec0 completes all five supporting jobs successfully. Its main gate passes2963 tests with zero failures and four skips, plus workspace doctests; both prior serving-fixture failures are confirmed repaired. Strict rustdoc then rejects two StateStore save/load links to MAX_CHECKPOINT_BYTES through a private module. Replace exactly those two doc links with the actual64 MiB limit, retaining implementation, visibility and strict lint policy. The exact781765-byte terminal log SHA256ba00375b057c261cbb8ec7efb71fb77897bfe305e384b6b2231a6aebeb35c502 is retained as bounded proof; book generation was not completed. Refresh the current execution-state header and retain earlier records as history. Push this documentation-only correction separately for ordinary hosted validation of the adopted G1 output and fixtures; later CLI/batch/source-row work remains uncommitted and unexecuted. Published source inventory stays275/357 with82 pending,20 pending e2e mappings and68 divergences; strict M8 stays off and M2-S02 open/unaccepted. No local builds/tests, services, temporary roots, hardware work, new filing or milestone acceptance. |
| 2026-10-15 | M9 precompile source binding and positive reconstruction coverage | Adopt complete authenticated codec, runtime-API, registration, crowdloan and Ledger-test dispositions:280/357 source rows reconciled,77 pending. Preserve native typed/strict inputs, checked u64, immutable views, D-039/D-044/D-046/D-059 and S9; the Ledger pytest input remains oracle-only with its mandatory-context adaptation and no device claim. Correct canonical-signature CLI deprecation/caller-role/owner notices using the resolved ABI name while preserving exact overload encoding; three substantive fixtures pin actual ABI choices. Add a real Utility.batch JSON/import/sign/attach/decode fixture. Bind all five Python precompile tables to production through authenticated AST extraction and ABI identities; an explicit copied-data bootstrap must be replaced by real hosted output before acceptance. Independent review found constructor annotations/modifiers could change semantics; the revised guard refuses pseudo-fields, aliases, shadowed builtins and field modifiers while preserving valid boolean-default changes. Source/data/format/application checks pass; extractor/consumer fixtures remain unexecuted. Review all nine fuzz targets, complete driver,21 prior committed seeds and actual production/regression paths. Add a hand-authored public17-field immortal remark candidate, initialization reconstruction guard and ordinary sign/attach/tamper/cache/no-author witness; no positive execution or sanitizer coverage is claimed. Correct historical no-KDF prose to disclose mnemonic PBKDF2 without wallet-envelope decryption. All repairs received independent source review.20 e2e mappings and68 divergences stay unchanged; strict M8 remains off and M2-S02 open/unaccepted. Preserve live ordinary38814558931 at4e9447b; use a skipped-CI source push and separate bounded hosted generation. No local builds/tests/fuzzing, new temporary root, node/service/device/Mac Studio work, upstream filing or milestone acceptance. |
| 2026-10-15 | M9 generated precompile output adoption | Generation 38816297566 passes at clean 2ad5d1b. Authenticate and independently refetch both archives; all 23 members match the reviewed inspection. Adopt exactly seven files: the generated precompile consumer, five Python table records, named tables and four reports. The other 16 generated files and all 23 dependency inputs are unchanged. Matrix reaches 280/357 reconciled, 77 pending; role counts, 20 pending e2e mappings, 68 divergences and runtime/oracle claims are unchanged. Retain bounded inspection/adoption proofs without archives or extraction directories. Preserve live ordinary 38814558931 at 4e9447b, whose five supporting jobs pass; newer fixtures remain unexecuted. Strict M8 stays off, M2-S02 remains open/unaccepted and no milestone is accepted. No local builds/tests, services, Mac Studio work or upstream filing. |
| 2026-10-15 | M9 ordinary test result and CLI rustdoc repair | Ordinary 38814558931 at 4e9447b completes: all five supporting jobs pass; the main gate passes all 2968 tests (four skipped) and workspace doctests. Strict rustdoc then rejects the output module's unresolved PythonBalance link. Qualify its existing public path, preserving behavior, visibility and strict lint policy. Retain bounded terminal proof check-38814558931.json (782118-byte log, SHA256 8343a0f3afc901d20745f0f6b5d1c12f053e0f33d7fa7feac4c18141ca115394). Book generation was not reached. Push adopted generated outputs and this correction for normal hosted validation; newer fixtures have no passing result yet. Source/e2e/security status is unchanged; no local build/test or milestone acceptance. |
| 2026-10-15 | M9 transport policy and public diagnostic repairs | Apply independently reviewed patches: policy refusals received during paced reconnect return terminally before failure accounting/rotation, with four real-session fixtures; public RPC display applies Python strip/fallback rules while preserving typed causes, source, policy metadata and unknown-fate hashes, with eleven fake-startup vectors. Complete substrate/interface reviews support two qualified source dispositions (282/357 reconciled, 75 pending); generated matrix remains at 280 pending hosted refresh. Under rule 5, document native read/write ownership, authenticated watched external signing versus pool-only raw submission, and map-presence primitives without claiming a dedicated wrapper or complete probe fixture. Preserve null-pruning rows, S9, M2-S02 and strict M8 status. Fragmented HTTP refusal-body diagnostics remain a separate confirmed pending repair. Direct formatting/whitespace and source review pass; new fixtures remain unexecuted. Preserve ordinary 38818850318 at f21c091 with a skipped-CI source checkpoint and separate bounded report refresh. No local build/test, services, new temporary root or upstream filing. |
| 2026-10-15 | M9 transport report adoption | Report refresh 38819346876 succeeds at clean d8db07d, including full source parity. Authenticate and independently refetch artifact 12012143940 (57455 bytes, SHA256 c05ad8c4c5bd014594b13dcd15744f6b99f53306d50daa31f82326bb199bf0bb). Adopt only four reports; matrix changes exactly the two reviewed substrate/interface rows to 282/357 reconciled and 75 pending. All 23 dependency inputs, role counts, host flags, 20 pending e2e mappings, 68 divergences and other compatibility claims remain unchanged. Retain bounded proofs without archives or extraction trees. Preserve live ordinary 38818850318 at f21c091, with all five supporting jobs passing; the newer policy/diagnostic fixtures still require hosted execution. No local build/test, upstream filing or milestone acceptance. |
| 2026-10-15 | M9 bounded HTTP refusal capture and source dispositions | Adopt the independently reviewed retained-stream repair, preserving the actual verified WebSocket/101 tail, existing TLS config and upgrade/header checks. Capture at most 4096 entity bytes under 64 KiB wire processing, 1024-byte chunk lines and the original deadline; known policy survives optional detail failure. Add eight private units and one ws/wss fixture, including immediate malformed-byte refusal versus incomplete CR. Complete precompile/RPC source dispositions reach 284/357 reconciled, 73 pending; reports remain at 282 pending hosted refresh. Preserve native balances, author fate/replay, S9, M2-S02 and inherited pre-ack cancellation limits. Source/format checks pass; compilation and execution remain unverified. No local build/test, services or filing. |
| 2026-10-15 | M9 hosted lock resolution and earlier documentation checks | Exclude the handwritten lock candidate. The new direct tokio-rustls edge reuses an existing package but requires both workspace and fuzz locks; extend the existing hosted resolver/upload to both, sharing 170 seconds and retaining exact package-identity/edge guards. Source commit intentionally precedes hosted lock generation/adoption and ordinary validation. Move the unchanged mandatory documentation command before nextest/doctests so link failures stop earlier. Preserve live ordinary 38818850318 at f21c091 through a skipped-CI push; all five supporting jobs pass, main remains active. No new runner/job or local validation is added; no milestone accepted. |
| 2026-10-15 | M9 hosted lock adoption and extractor refusal repair | Ordinary 38818850318 at f21c091 passes all five supporting jobs and 2976/2977 tests (four skipped); the constructor-semantic refusal fixture fails before doctests/docs. Retain exact 773213-byte log hash 572428c4f9513974a72b200c53e5fc947ab20fe882f258bccf469a74b5818754 and seven authenticated named passing fixtures, including positive unsigned-seed reconstruction/signing/attachment. Source trace proves RustPython 0.4 discards singleton parentheses before deriving AnnAssign.simple. Require the original statement/name span to equal the canonical identifier, retain refusal coverage and add nested/index variants and exact failure diagnostics; no local tests run. Lock refresh 38822187126 succeeds at clean cc32b5d. Authenticate/refetch artifact 12013537957 twice (81335 bytes, SHA256 aa96d11c605c6921828634f484beda268e6a41eda543422e47ab9b21f538fb99), then adopt both generated locks: exactly one existing TLS dependency edge each, no identity/other changes. Source remains 284 reconciled/73 pending, reports at 282 pending refresh; no newer fixture, sanitizer/oracle or milestone acceptance is inferred. |
| 2026-10-15 | M9 current report adoption and combined CI checkpoint | Report refresh 38823630645 succeeds at clean 7cc7501, including full source parity. Authenticate and independently refetch artifact 12013334654 (57584 bytes, SHA256 01be5d4f44f225a41617f32957d1dfb61287737a909f75233fb8a2b47ddf2412); adopt only four generated reports. Matrix changes exactly the two reviewed precompile/RPC rows, reaching 284/357 reconciled and 73 pending. All 23 dependency inputs match source, with only the reviewed workspace lock and RPC manifest hashes changed; role inventories, host flags, 20 pending e2e mappings, 68 divergences and other compatibility claims remain unchanged. Retain bounded inspection/adoption proofs without archives or extraction trees. Push one ordinary checkpoint for the integrated transport, extractor and documentation fixes. Newer fixture execution and final security dispositions remain pending; strict M8 stays off and M2-S02 open/unaccepted. No local build/test, new temporary root, services, Mac Studio work, filing or milestone acceptance. |
| 2026-10-15 | M9 hosted no-host lint correction | Ordinary 38824416747 at d9ae668 passes supply, MSRV and semver; no-host fails strict Clippy on a stale Url import after the handshake refactor. Its actual remaining uses are test/fuzzing-only, so apply that exact existing condition to the import without relaxing lint policy. Retain terminal 184651-byte log hash 9254c2e86e6d7a6fc83d9ad914780d14822329ec028150f3987a91e8f68a2355 and bounded authenticated excerpts. Preserve the still-live main/feature jobs with a skipped-CI correction push; later source-review proposals remain separate working changes. No local build/test, service, upstream filing or milestone acceptance. |
| 2026-10-15 | M9 EVM source completion and journal opening hardening | Complete authenticated reviews and independent repair reviews reconcile all ten EVM CLI modules, basket display pricing and both optional recorder relationships: source map 297/357 reconciled, 60 pending; reports remain at 284 pending refresh. Repair seven exact input gaps with fourteen injected handler/intent/ABI/preview/confirmation regressions, preserving native checked u64, typed/wire bounds, late unlock, consent and unknown-fate policy. Strengthen the basket dust fixture with distinct prices/exact cutoff. Repair first-pruning-approval opening authentication and restart against retained receipt position/event and explicit reviewed timepoint; four modeled same/replaced-opening cases retain raw evidence, correct artificial inclusion positions and refuse another signer/broadcast. Extend D-041 evidence without a new filing. Correct already-Yuma no-op ordering and native wait-budget help; both whole null-pruning rows remain pending, including exact wait semantics and full pytest mapping. Complete codegen/check.py review leads to authored metadata-name/classifier/nonblank prose guards and mutation fixtures; retain its source row pending follow-up review/execution. Ordinary 38824416747 main fails strict Clippy fixture compilation on RPC-versus-native H256 (77984-byte log SHA256 351ce4a11a4b49f367c5bbe81e8f28722e76af135fb38b564df3a1214fd1cb3d); qualify only the RPC constructor. No tests/docs ran there. Its feature job subsequently passes; the terminal run has no test/docs execution. Push the reviewed source batch with skipped CI and refresh reports before the next combined gate. Current fixtures/security acceptance remain unverified; strict M8 off and M2-S02 open/unaccepted. No local builds/tests, node/service/device/Mac Studio work, new temporary root or upstream filing. |
| 2026-10-15 | M9 report adoption and wait-budget correction | Hosted refresh 38826175917 passes at clean d2d5add, including full source parity. Authenticate and independently refetch artifact 12015413800 (57978 bytes, SHA256 f3af367e6632bef6c2200ca8fee15bb86387e81f5598e348337ca4db52ffa8da); adopt exactly four reports. Source map/matrix now agree at 297/357 reconciled, 60 pending; only thirteen source promotions and two D-041 tests change. All 23 dependency inputs, role inventories, host flags, 20 pending e2e mappings, 68 divergence identities and other compatibility claims stay unchanged. Restore Python's null-pruning timer: start on first wait, clear after each accepted batch and permit a final partial-sleep state recheck. Independent source review approves three paused-time FakeChain regressions while journal, finality, three-stall/max-batch, submission deadlines and attempted-delivery draining remain intact. Whole null-pruning rows remain pending. Retain bounded proofs without archives/extraction directories, then push one ordinary hosted checkpoint. Current test/docs success, strict M8 and milestone acceptance remain unclaimed; M2-S02 stays open/unaccepted. No local builds/tests, services, new temporary root, Mac Studio work or upstream filing. |
| 2026-10-15 | M9 timelock documentation links | Ordinary 38827263561 at dc5c09a passes all five supporting jobs and main Clippy/source/pin/extraction checks, then strict rustdoc fails on two unqualified module links to timelock seal/open. Qualify their public crate paths; no behavior or lint-policy change. Preserve the exact 106372-byte terminal log SHA256 e30baf51a75ef2456d0d676950754e4be50e48c07bd0e9350cbc8dcc331bdafd in bounded proof check-38827263561.json. Documentation precedes nextest; behavior tests and workspace doctests did not run. Compiler corrections are verified, newer fixture execution and docs completion remain pending. Push the focused correction for ordinary hosted CI; leave the separately reviewed weights/staking/source dispositions unapplied. Source/matrix remain 297 reconciled, 60 pending, with 20 e2e mappings pending; strict M8 off, M2-S02 open/unaccepted and no milestone accepted. No local builds/tests, services, new temporary root, Mac Studio work or upstream filing. |
| 2026-10-16 | M9 integrated hosted checkpoint passes | Ordinary 38832708259 passes all six jobs at exact 7bb8c0b: main, no-host, features, MSRV, semver and supply. Main passes strict Clippy, source/pin/extraction checks, documentation generation, all 3013 executed tests (four skipped) and workspace doctests; documentation artifact uploads successfully. Authenticate the 927796-byte main log, SHA256 88bc33d05df3a5c14789d615940791ce65c1e6486726926f1b25de2dc15933d3; retain bounded results without raw logs, archives or extraction trees. Exact-name evidence confirms all 70 selected S9 regressions and 12 new M2-S03 regressions pass on this source. Source/matrix remain 297/357 reconciled, 60 pending, with 20 pending e2e mappings and 68 divergences. Final security dispositions require source review and retained coverage limits; passing CI alone accepts no source row or milestone. Strict M8 remains off and M2-S02 open/unaccepted. Local target is absent; no local build/test, service, Mac Studio work, new temporary root or upstream filing. |
| 2026-10-16 | M9 security dispositions and weight arithmetic repair | Independent repair-boundary review and 95 exact current PASS records close the original S9-01 through S9-10 defects, the pruning-opening substitution defect and M2-S01/S03/S04/S05 with their documented limits. Bind all 40 source blobs and recompute current test anchors; retain the authenticated log identity and bounded evidence, without another test run for those unchanged sources. M2-S02 remains open/unaccepted. Separately repair a demonstrated public weight-clipping success/error mismatch: finite minimum-subnormal scores can underflow the final denominator to zero, where Python raises and Rust returned successful NaNs. Add an exact-zero refusal using native WeightsError and a helper/conform regression preserving valid subnormal branches. Existing conform already refused before call composition; no invalid submission established. Independent source review, direct formatting and whitespace checks pass; new regression execution awaits hosted CI. Keep weights pending, source/matrix at 297/357 reconciled and 60 pending, strict M8 off and every milestone unaccepted. No local build/test, services, Mac Studio work, new temporary root or upstream filing. |
| 2026-10-16 | M9 codegen check source disposition | Complete independent authenticated review reconciles all six functions and declarations of codegen/check.py, with native production tooling/gate callers and substantive fixtures. Add the actual parity_catalog.rs home alongside the extracted tables; bind source blob a859b70612e8b92d0ae14e51c657e996ab9052e2. Independently authenticated exact current PASS records verify the metadata-name/Unknown and Python-strip-nonblank guards. Record native pinned regeneration, Result/CLI, namespace and authoritative Rust custom-code adaptations without claiming Python/live drift or exhaustive pytest execution. Source map reaches 298/357 reconciled, 59 pending; matrix remains at 297/60 pending hosted refresh. Preserve ordinary 38858957306 at 50b7239 for the narrow weight repair; later report/source changes use a skipped-CI checkpoint before bounded generation. Strict M8 stays off, M2-S02 open/unaccepted and no milestone accepted. No local build/test, new temporary root, services or upstream filing. |
| 2026-10-16 | M9 codegen report adoption | Hosted refresh 38859189354 passes full source parity at clean 6c4f82c. Authenticate and independently refetch artifact 12029210729 (58018 bytes, SHA256 da3e79b71704bf997cc96d26aa544949de9330f4a1f47dc0d8d5e736c8802159), then adopt exactly four reports. Matrix changes only the reviewed codegen/check.py row: source/matrix agree at 298/357 reconciled and 59 pending. All 23 actual dependency inputs, role counts, host flags, 20 pending e2e mappings, 68 divergences and other compatibility claims remain unchanged. Retain bounded inspection/adoption proofs without archives or extraction trees. Preserve ordinary 38858957306 for the weight repair through a skipped-CI report checkpoint. No local build/test, service, Mac Studio work, new temporary root, upstream filing or milestone acceptance. |
| 2026-10-16 | M9 weight repair passes hosted CI | Ordinary 38858957306 succeeds at exact 50b7239 in all six jobs. Main passes all 3014 executed tests (four skipped), doctests, docs and selected blocking checks. Authenticate the 710339-byte main log, SHA256 15e295426302f686b8cef1918bf91bc2050f97588eca73028870037c2cd82357, and retain 41 exact named PASS records: the new subnormal clipping regression, all 33 SDK pruning fixtures and seven CLI pruning fixtures. Correct stale pruning execution claims while retaining absent exact boundary/pytest/live-runtime coverage. No local execution, new root, raw archive or acceptance. |
| 2026-10-16 | M9 staking, weights and pruning source dispositions; prompt repairs | Complete authenticated whole-file reviews reconcile 14 staking intents, all 19 weight functions/methods and the pruning product workflow. Add precise native homes/secondary origins and correct compensated sum versus left-fold documentation. Explicit typed/native policies and separate pytest/oracle obligations remain. Source map reaches 301/357 reconciled and 56 pending; matrix remains at 298/59 pending hosted refresh. Repair the actual AppContext account-name tuple drain and connect successful prompt completion to original argv and the actual native price API, preserving machine/quiet/dry-run suppression and hidden-input omission. Two selection fixtures and five actual parser/handler/finish/fake-client fixtures are authored and independently source-reviewed; formatting passes and execution awaits hosted CI. Prompt/output and separate pruning/weight pytest rows remain pending. No local build/test, service, Mac Studio work, extra temporary root, new filing or milestone acceptance. |
| 2026-10-16 | M9 attribution correction and CLI content/order repairs | Report-only 38862240133 fails at exact b59fd13 because the newly reconciled rtao-neuron pruning home uses a combined attribution comment. Preserve the guard and split the two sources into canonical headers; no generated report adopted. Retain bounded 59499-byte log identity, SHA256 0971f8c9149cd5390fb2b53e9df618110d4ae8278cf6538bd9ab7d73ad60a681. Integrate independently reviewed O1/O2 v2 and P3: actual transfer-history content and checked successful root-TAO totals, catalog-aware explain guidance, and public wallet confirmation before missing sign/decrypt input. Raw JSON, quiet/machine rules, strict grammar, configuration precedence and prompt replay remain explicit contracts. Four output/command and six wallet-order fixtures join the preceding seven unexecuted fixtures; source review, direct rustfmt and whitespace checks pass, hosted execution pending. Source map stays 301/357 reconciled and 56 pending; adopted matrix stays 298/59. Output/prompt, samples and pytest rows remain pending; strict M8 off, M2-S02 open/unaccepted, no milestone acceptance. No local build/test, service, Mac Studio work, new temporary root or filing. |
| 2026-10-16 | M9 corrected report adoption and producer/pruning regressions | Corrected report-only 38862997033 passes full source parity at clean f48535d. Authenticate and independently refetch artifact 12029777947 (58166 bytes, SHA256 ebff9b28aa3c2329462ba83031580e438135c7e97e69e598a434fefe83d8a95d); adopt exactly four reports, promoting only the three reviewed staking/weights/CLI-pruning rows to reach source/matrix 301/357 reconciled and 56 pending. All 23 dependency inputs, role counts, host flags, 20 e2e gaps, 68 divergences and other claims unchanged. Integrate independently reviewed sample-only producer binding/effects/declaration guards and five actual-resolver fixtures preserving six original outputs; no incorrect pinned value established, whole samples row stays pending. Add four real pruning-session/client fixtures for implicit mechanism2 target128, pending-target priority and complete key-only legacy queue probes including empty TWC values. Exact Python origins/native adaptations remain explicit; full pytest row stays pending. Nine additions join seventeen CLI fixtures awaiting hosted execution; source review, rustfmt and whitespace pass. Read-only 06:50 UTC refresh confirms thirteen recorded upstream filings remain open and eight PRs unmerged at unchanged heads with empty labels/review requests; no new census/source-test claim. No local build/test, service, Mac Studio work, extra root, archive/extraction tree, new filing or milestone acceptance. |
| 2026-10-16 | M9 prompt argv ownership correction | Ordinary 38863766724 at 9bd2d02 exposes MSRV E0382: a move handler consumes the original argv vector before success guidance borrows it. Bind a shared argv slice inside dispatch and pass it to all seven async handlers, retaining the owned original for final hints. Record exact bounded MSRV log evidence (58981 bytes, SHA256 c776d6f6862cc8b4af9ba8737660b973258c7269eed3f15c3e51ab6088b5d935). Direct rustfmt and whitespace pass; all 26 newly authored fixture results remain pending the corrected hosted checkpoint. Source/matrix counts and strict/milestone state unchanged; no local build/test or new filing. |
| 2026-10-16 | M9 wallet fixture lint correction | Ordinary 38864132855 at 8211420 passes MSRV/no-host/semver; main stops before tests on two needless_borrows_for_generic_args findings in the new wallet fixture. Pass the Copy environment closure directly at both existing calls, preserving the strict lint policy. Retain bounded main log identity (68704 bytes, SHA256 0876f601d04385eb3305af03cfd868c69bd710d0869e004849f0f032617cdb95). Direct formatting/whitespace pass; all 26 new fixture executions remain pending the corrected hosted run. Complete PoW module/solver/worker/GPU and fixture source review found no supported-domain defect; its proposed disposition stays ignored pending exact hosted evidence. Source/map and milestone states unchanged, no local builds/tests or filings. |
| 2026-10-16 | M0 source and fixture completion record | Ordinary 38865058430 passes all six jobs at exact 3f1aa67: 3040 tests pass, zero fail, four skipped, with strict Clippy, docs, doctests and blocking source/dependency checks. Independent current audit finds no missing M0 implementation and binds 60 exact inputs. Authenticate 123 named M0 PASS records, including all six goldens, corpus, 44 btauth tests, native money/JSON, signers, strict verification, BLS and both real-binary doctor cases. Retain bounded m0-3f1aa67-hosted-results.json (60592 bytes, SHA256 2f9256ef73b49321c0b74ea1ed7c3d4c8b19bbeba783f8ea73c497412e0094ff); main log 715611 bytes/SHA256 55de87df70e9afc863da7175a9dc7ed809ce239c310f8b2e2eef0b84ef46e975. Check applicable M0 source/fixture steps and record M0-29; update current M0 docs. Final milestone exit still requires applicable hosted checks on the combined follow-up checkpoint. Q30 deferred/optional evidence, M1 gaps and M2-S02 remain separate; no local execution, release or milestone claim from this record alone. |
| 2026-10-16 | M9 samples/PoW source dispositions and root target-selection repair | Exact 3f1aa67 proof confirms all 26 new fixtures, 26 existing native PoW/fake-GPU cases and four all83/all102 consumers pass. Whole-source independent review reconciles samples.py and pow_registration.py, with precise native homes, canonical headers and explicit typed fixture/proof/backend/cleanup/valid-challenge adaptations. Source map reaches 303/357 reconciled, 54 pending; adopted matrix stays 301/56 until hosted refresh. Find S9-11 in actual root picker: an earlier display label equal to a later validator's full SS58 could override that exact address. Restore address-before-label precedence after numeric choices; add one actual allocation/typed-intent/replay fake-chain regression with collision and controls. Independent review, formatting and whitespace pass; the new regression awaits hosted execution. Whole root-helper and pytest rows stay pending, strict M8 off, M2-S02 open/unaccepted. No new filings, local builds/tests, service, Studio work or new temporary root. |
| 2026-10-16 | M1 RPC fixture provenance and maintained TLS validation | Independent M1 audit identifies two copied pytest parameter tables, seven missing exact origins and no maintained current two-provider RPC fixture. Add a guarded Rust AST producer and three actual-resolver mutation/provenance fixtures; existing policy tests consume the generated corpus. Add a nonshipped independent TLS workspace and bounded lock-preparation mode; existing features job will exercise one real RPC/FakeNode exchange with both ring/AWS-LC compiled for shipped Rustls 0.23.46 and no process default. Shipped dependency inputs, locks and AWS-LC bans stay unchanged. Source review/formatting pass; hosted corpus generation, independent lock adoption and combined execution remain pending. Five M1 execution documents distinguish historical evidence from Q30 hosted/deferred scope and retire stale cache requirements. Source map stays 303/357, 54 pending; matrix stays 301/56 until refresh. Whole pytest/root-helper reconciliation, HTTPS coverage and M2-S02 remain separate; no milestone acceptance, local build/test, service, Studio work, new root or filing. |
| 2026-10-16 | M1 authenticated generation and combined client follow-up | Codegen/report 38871417189 and isolated TLS-lock 38871434589 pass at exact d5132a0. Authenticate 19 generated members, four reports and two TLS members; independently refetch and adopt exactly six outputs plus the isolated lock. Old corpus/catalog bytes and 17 other generated members remain identical; matrix now agrees at 303/357 reconciled, 54 pending. All 23 shipped dependency inputs/counts/host flags, 20 e2e gaps and 68 divergences stay unchanged. Integrate reviewed root-helper F1-F5 repair with five actual command fixtures, refused-connect socket observer, batch cardinality assertion and metagraph direct-hex adapter repair using the existing shared parser plus one public fixture. Whole RPC audit traces all40 functions/91 assertions; whole metagraph/fake-node audits retain typed API and execution limits. Complete M1 resource audit of23 documents corrects17 further stale local-cache/reproduction/platform requirements. Formatting/whitespace pass; expected3051 workspace tests plus one isolated TLS test await combined hosted execution. Source rows remain pending until final disposition/results; M2-S02 open/unaccepted, strict M8 off, no milestone accepted. No local build/test, service, Studio work, new root/archive/extraction tree or filing. |
| 2026-10-16 | M0 exit and M1 read-path exit | All six jobs pass at d01830c in hosted run 38873429391: 3,051 tests pass, zero fail, four skipped, plus nine doctests across 19 suites, strict Clippy, docs and source/dependency guards. Authenticated bounded proof binds 175 unique named PASS records and 129 final source inputs; overlapping groups retain 123 M0, 45 M1 and 11 new-fixture results. The foundation audit retains 55 unchanged inputs and five reviewed deltas. Separate exact TLS evidence verifies one real RPC test with both compiled providers and no process default. Record the M0/M1 Q30 client-development exits and check M1-01 through M1-21; M1-22/24 remain optional and M1-23 deferred. S9-11 and root-helper/metagraph repairs pass. Source/matrix remain 303/357 reconciled, 54 pending, with 20 e2e gaps and 68 divergences. Strict M8 stays off and M2-S02 open/unaccepted. No fresh oracle, live-network, platform, device or release claim; no local build/test, Studio work, service, new temporary root or filing. |
| 2026-10-16 | M6 root command and M8 human-output follow-up | Adopt independently reviewed signing-wallet-before-relative-hotkey and allocation-picker context repairs, plus curated wallet list fields/defaults, pending multisig member/replay content and mnemonic-clear stdout guidance under quiet. Five new fixture functions and extended actual-command regressions await hosted execution; direct formatting and whitespace checks pass. Complete authenticated source reviews with exact d01830c evidence reconcile only metagraph.py, harness/fake_node.py and test_rpc.py. Source map reaches 306/357 reconciled, 51 pending; generated matrix remains 303/54 until hosted report refresh and reviewed adoption. Whole root/output/prompt rows stay pending for remaining context/picker work. Strict M8 off, M2-S02 open/unaccepted, no later milestone acceptance. No local builds/tests, new temporary root, Studio work, service or filing. |
| 2026-10-16 | M1 three-source report adoption | Hosted refresh 38876950939 passes full source parity at exact 82d82d7. Authenticate and independently refetch artifact 12035678693 (58,349 bytes, SHA256 2770c94c6e5507a9f829272eb3f7ababa080746637c1db55e4add34ad5340a08), then adopt exactly four reports. Matrix changes only the reviewed metagraph, FakeNode and RPC pytest rows; source/matrix agree at 306/357 reconciled, 51 pending. All 23 shipped dependency inputs, role inventories/host flags, 20 e2e gaps, 68 divergences and other compatibility claims remain unchanged. Retain bounded proofs without archives or extraction directories. Push the ordinary combined checkpoint for the five new CLI fixtures and extended regressions. Current fixture execution remains pending; strict M8 off, M2-S02 open/unaccepted, no later milestone acceptance or new filing. No local build/test, service, Studio work or new temporary root. |
| 2026-10-16 | M6 command review, M8 required addresses and association contract | Adopt independently reviewed display context through actual root previews, final confirmation, external signer labels and saved approval cards, with semantic mismatch refusal before RPC/private acquisition. Integrate existing public pickers into ten non-EVM required-address routes with signer-first wallet selection, real clap validation/reservations and exact positional replay. Correct three fixture-only setup errors before adoption. Add one real AssociateHotkey executor/receipt/pinned-read fixture and exact client-contract e2e binding with five explicit runtime limits. Thirteen new functions are authored and directly formatted; hosted execution remains pending. Source-file map stays 306/357 reconciled, 51 pending; e2e source pending falls to 19 while generated matrix remains at 20 until hosted refresh. Strict M8 off and M2-S02 open/unaccepted. Current earlier checkpoint 38877875846 still runs main after five supporting jobs pass. No local build/test, new temporary root, services, Studio work, device or upstream filing. |
| 2026-10-16 | Association report adoption and keyfile fixture coverage | Hosted report refresh 38880542426 passes at exact af2565d. Authenticate and independently refetch artifact 12037044413 (58,405 bytes, SHA256 2c8bba283bf36bf31a169098ff7e210799811a49489643d1c90030f846e6e5b1); adopt exactly four reports. Only the association row and derived counts change: 19 pending e2e keys and ten client contracts. All other source families, 23 dependency inputs, role inventories/host flags, 68 divergences and compatibility claims remain unchanged. Complete eight-test/23-assertion keyfiles-golden review finds no product defect; independently review and add one Unix overwrite/permission fixture plus stronger NaCl/signature assertions without extra KDF calls. Retain oracle-only, Fernet, exact encrypted-create and isolated-umask execution limits; whole source row stays pending. Expected combined main total is 3,070 tests, all 19 new functions since d01830c awaiting hosted execution. Source files remain 306/357 reconciled, 51 pending; strict M8 off, M2-S02 open/unaccepted. No local build/test, new temporary root, service, Studio work or new filing. |
| 2026-10-16 | Association fixture import correction | Combined hosted 38882046195 at a042d98 stops in strict Clippy before tests on E0432: associated_evm_key is public under reads::m6_accounts, not the reads root. Correct only the fixture import; retain production API and lint policy. Bounded diagnostic check-38882046195.json authenticates its 67,265-byte log, SHA256 6c522c799df32b98f6e0626057441d7110669acd0f6c1026c1af322a9955b05b. All 19 newly authored fixture executions remain pending a corrected combined checkpoint. Previous run 38877875846 was cancelled when superseded, with no complete main-test summary. No local builds/tests, services, new temporary roots or filings. |
| 2026-10-16 | Reviewed root decimal/picker and public policy batch | Integrate independently reviewed root-only exact Decimal normalization, native checked u64 and strict-constructor preservation, explicit D-042 4096-byte/exponent budgets, signed-zero support and attached prompted replay. Retain Move picked destination names while preserving required source-identity failures. Correct public PolicySubject diagnostics to use the semantic operation, with real BurnedRegister direct/plan/execute refusal coverage. Reuse finite auto-stake submission/read and offline transfer/Proxy/Sudo signing harnesses. Seven new fixture functions are authored, directly formatted and source-reviewed; expected next main total is 3,077, all seven unexecuted. Source e2e pending becomes 18/client-contract 11; generated reports remain 19/10 until refresh. Source files remain 306/357 reconciled, 51 pending; keyfiles oracle-only disposition unapplied pending current fixture evidence. Audit/correct 25 M2 execution records and the validator metric, retiring local build/cache and mandatory infrastructure instructions while preserving historical results and exact source/security/coverage limits. M3-M9 record audit continues. Current 38883630624 still runs main after five supporting jobs pass; no current success or later milestone exit inferred. Strict M8 off, M2-S02 open/unaccepted. No local builds/tests, nodes/services, Studio work, new root or upstream filing. |
| 2026-10-16 | M3-M9 execution-record resource audit | Read all 31 authored execution Markdown records; correct 13, retaining 18 without changes. Independently review every changed hunk and reconstruct all 13 exact after-images in memory against their declared hashes. Patch bacf2123affc25d82008e77b0b1a73cef6f38f7967189a380a67136bf1149a35 retires stale local cache/tool/restore and mandatory infrastructure/release wording, preserves historical test counts and the durable M4 hyperparameter proof, and keeps applicable hosted source/fixture/security/docs checks required. Template workflow source confirms native checks by default and explicit optional hosted localnet opt-in; no workflow is activated. No new execution result, source-row promotion or milestone acceptance follows. Current 38883630624 still runs main with five supporting jobs passed. No local build/test, service, Studio work, new temporary root or filing. |
| 2026-10-16 | Partial hosted result and picker fixture correction | Exact 0b0bb79 run 38883630624 completes with five supporting jobs passed; main runs 3,070 tests with 3,069 passed, one picker fixture failed and four skipped. Doctests/docs were not reached. Bounded partial proof authenticates 22 selected PASS records, including both keyfile fixtures, without claiming overall success. Correct the failing fixture: assert Clap waives the missing required address when a conflicting message is supplied, then omit both fields to test prompting followed by final conflict rejection. No production behavior changes. Adopt reviewed keyfiles source-input disposition with exact two-fixture evidence, retaining oracle-only and explicit Fernet/encrypted-create/umask gaps. Source inputs reach 307/357 reconciled, 50 pending; generated reports remain 306/51. Source e2e remains 18 pending/11 client contracts versus generated 19/10. Seven newer functions remain unexecuted; next combined total is expected to be 3,077 after hosted report refresh and adoption. Strict M8 stays off, M2-S02 open/unaccepted. No local builds/tests, services, Studio work, new temporary root or filing. |
| 2026-10-16 | Auto-stake/keyfiles report adoption | Hosted report-only 38888110550 passes at exact 3143d72. Freeze all 36 source inputs, authenticate artifact 12040193494 (58,602 bytes, SHA256 33a643d7137ee0ae94a183764483926ec702056e69088bcc2aad788363bd1203), independently refetch and adopt exactly four reports. Source/matrix now agree at 307/357 reconciled, 50 pending; e2e source/matrix agree at 18 pending and 11 client contracts. Only the reviewed auto-stake row, oracle-only keyfiles source input, one first D-042 test, derived counts and normal measurement metadata change. All 23 dependency inputs, role inventories/host flags and other compatibility claims stay unchanged. Correct historical security resource framing without extending reviewed-source identities. M3 source-readiness audit finds implemented client/template witnesses; shared compiled-example documentation wiring remains unfinished and also limits M2 guide acceptance. Owning-boundary review confirms no safe compatible wrapper for M2-S02; upstream issue #3251 remains open. Push ordinary combined CI for 3,077 expected tests; current execution remains pending. Strict M8 off, no later milestone acceptance, no local build/test, services, Studio work, new temporary root or filing. |
| 2026-10-16 | Multisig registration completion fixture | Add one independently reviewed actual MultisigThreshold1Adapter<RegisterSubnet> fixture with ordinary immediate and shielded later-block completion. Distinct member/group/explicit-hotkey values catch forwarding mistakes; the shielded vector requires deferred completion and retains the original receipt. Reuse FakeChain and established deterministic inner-receipt overlay without new dependencies, production harness changes or a broader matrix. Source review, applicability, direct formatting and whitespace pass; execution remains pending outside current 60a20ee run 38889135192. Next combined total is expected to be 3,078. The exact multisig-safety source row stays pending. No local tests/builds, services, Studio work, new temporary root or filing. |
| 2026-10-16 | Compiled operator examples and current authentication source | Integrate reviewed Docify 0.4.1 source wiring for real validator/miner/owner examples, guide excerpts and unchanged README output. Explicit include_str dependencies track proc-macro input changes. Keep Docify dev-only in the facade and normal only in unpublished xtask. Reuse free hosted lock-refresh with a bounded first-Docify candidate guard; preserve all old package identities/metadata and resolved dependency edges, allow only two Docify edges and a bounded registry closure, retain fuzz lock unchanged. Correct the independently found raw-edge qualification false rejection by resolving exact package identities. Verify all 13 source after-images and direct formatting/script syntax; lock resolution, candidate review, graph/report refresh and hosted compile/tests remain pending. A fresh reader completes two full cold reads of 3,280 authentication-source lines at 6fba8dc with no surviving concrete finding; preserve historical M3 hashes and append current identities/limits. All five auth hashes match pending 60a20ee source. Current run 38889135192 still has main active after five supporting jobs pass; no overall success or later milestone exit is inferred. Expected next main total remains 3,078. M2-S02 open/unaccepted; source/matrix inventory stays 307/357 and 50 pending; strict M8 off. No local builds/tests, installs, services, Studio work or new filing. |
| 2026-10-16 | Green client checkpoint and bounded Docify lock correction | Ordinary run 38889135192 passes all six jobs at exact 60a20ee: 3,077 tests, zero failed, four skipped, nine doctests across 19 suites, strict Clippy/docs and blocking guards. All seven root/policy/offline/auto-stake functions and corrected picker fixture pass. Apply the three reviewed root-command source dispositions with exact fixture evidence: source map 310/357 reconciled, 47 pending; generated matrix remains 307/50 until hosted refresh. New registration fixture and Docify source are outside that green checkpoint. First Docify lock run 38891932293 at 0f68970 resolves ten additions, then correctly rejects unlisted build helpers; no lock adopted. Narrow the guard to exact observed versions/checksums and the existing cc parallel-feature edges, preserving old identities/metadata/resolved edges. Retain failed candidates for diagnosis. Next combined main expects 3,078 tests. M2-S02 remains open/unaccepted, strict M8 off; no later milestone acceptance or new upstream filing. No local build/test, services or Studio work. |
| 2026-10-16 | Reviewed documentation dependency candidate | Free hosted lock run 38893353758 passes at exact 80ad78b. Authenticate the bounded artifact, inspect all ten exact package additions and three old-package edge changes, obtain independent source review and separately refetch before adopting only Cargo.lock. Preserve every old identity/metadata/edge and unchanged fuzz lock. No runtime dependency is declared for Docify; profile graphs, supply and compilation remain subject to hosted validation. Next generate compatibility/parity reports for the reviewed root dispositions and dependency inputs. No local Cargo, raw archive retention, new upstream filing or milestone acceptance. |
| 2026-10-16 | Local-snippet documentation compatibility correction | Report run 38893835867 at 7ac3c94 fails compiling Docify 0.4.1: its open git2/sha2 constraints select API-incompatible new releases. Select published Docify 0.2.9, whose used local snippet/Markdown APIs remain supported without the unused Git/SHA/native build closure; authenticate source/checksums and prepare exact hosted resolution against the pre-tool graph. Add two independently reviewed shared offline balance/formatting examples; embedding produces Markdown, actual execution remains pending. Expected combined main total becomes 3,080. Reconcile the fully reviewed historical basket producer as oracle-only under section 8.3 with exact frozen-data identities and explicit unverified producer/archive behavior. Source map becomes 311/357 reconciled, 46 pending; generated remains 307/50. No local builds/tests, service, upstream filing or milestone acceptance. |
| 2026-10-16 | Local documentation lock adoption | Hosted lock run 38895014224 passes at 6a2e191. Authenticate immutable artifact, inspect all nine exact additions against the original pre-Docify graph and independently refetch before adopting Cargo.lock only. Remove the failed Git/native build closure and restore old cc edges; every original package identity/metadata/edge and fuzz lock remain intact, except the two reviewed Docify consumer edges. No local Cargo or validation; hosted reports and 3,080-test combined CI remain next. |
| 2026-10-16 | Root port attribution correction | Report run 38895327494 compiles corrected Docify and generates dependency/parity outputs, then the license guard rejects root_move.py because its source appears only in an additional-sources comment. Replace that shorthand with complete Derived-from headers for both root_move.py and root_swap.py at the authenticated Apache-2.0 commit; preserve the actual root.py/root_helpers.py headers and all production bodies. Retain failure evidence, adopt no failed-run reports and rerun the bounded hosted report lane. No local Cargo/test or milestone acceptance. |
| 2026-10-16 | Root and documentation report adoption | Hosted report-only run 38896301076 passes at exact 4c1e5af, including attribution. Authenticate artifact 12044595804 (58,707 bytes, SHA256 9eefd53289a02dc5b948266ee11a7229aafde024dc03c2b2fa98325be58baca2), inspect three root rows plus oracle-only basket producer and all 23 dependency inputs, independently refetch, then adopt exactly four reports. Source/matrix agree at 311/357 reconciled and 46 pending. Only four reviewed Docify dependency-input hashes and normal measurement metadata change; profile package counts 225/362/366/417, host flags false/false/false/true and other compatibility claims stay unchanged. Push ordinary combined CI for 3,080 expected tests; no new runtime/oracle/hardware or milestone acceptance claim. No local Cargo, raw archive retention or new filing. |
| 2026-10-16 | Green 3,080-test client checkpoint and individual M2 steps | Ordinary run 38897437100 passes all six jobs at exact 3772699: 3,080 tests, zero failed, four skipped and nine doctests across 19 workspace suites, plus strict Clippy/docs/source/feature/MSRV/no-host/semver/supply guards. Bounded proof authenticates 34 selected PASS records, including wrapped registration completion, both shared documentation examples and positive unsigned-seed reconstruction/signing/attachment. Nextest's nine-slow suffix and stdout/stderr-interleaved doctest headings are represented without changing counts; independent collector review finds no defect. Check the 20 independently audited M2 implementation/validator-guide steps; M2-26 and M2 remain unaccepted with M2-S02 open. Adopt the fully reviewed multisig-safety source row after adding five genuine adapted pytest origins and preserving all executable witness bodies. Source reaches 312/357 reconciled and 45 pending; generated matrix remains 311/46 until refresh. No independent Python, live-chain, hardware, standalone fuzz or later milestone result follows. |
| 2026-10-16 | M9 documentation and offline-signing source batch | Add the missing SDK, agent, migration, concepts, Null-consensus, testing, parity, upgrades and pin-log chapters. Generate complete LLM chapter inclusion, CLI-derived agent card, item aliases, divergence register and per-transaction Executor guidance; keep source reconciliation status visible. Include READMEs in all 19 library roots and add docsrs-only feature labels, with three generated intent outputs awaiting hosted regeneration. Add three shared offline example tests, one signature-form attachment regression and three generator regressions; strengthen imported metadata-mode attachment and bidirectional cross-signature refusal without another function. Independent source reviews, direct formatting and whitespace checks pass. These seven new functions and current docs are outside green 3772699; the next main run expects 3,087 tests after reviewed hosted codegen/report adoption. The prepare/attach source row remains pending until its new and strengthened fixtures pass. M2-S02 remains open/unaccepted, strict M8 off; no new milestone acceptance, local build/test/codegen, upstream filing or resource campaign. |
| 2026-10-16 | Multisig and documentation generated-output adoption | Hosted codegen/report run 38901469526 passes at exact 1ba30ab. Authenticate compat artifact 12047032074 (58,742 bytes, SHA256 da141cd73b787c348d64e630e53f6e3ad9ac6005b69e84459be400f38a271d77) and generated artifact 12046786797 (278,109 bytes, SHA256 eb5a14c499aff2133f427992cf7882db2acbd084bb04464591ed88ede5f90eee). Independently inspect and refetch before adopting exactly three generated intent files plus four reports. The three contain only 187 docsrs feature badges matching adjacent existing cfg gates; removing them reproduces every old byte. All 16 other generated members remain identical. Source/matrix agrees at 312/357 reconciled and 45 pending. All 23 dependency inputs match current source; only the root documentation lint and 19 crate documentation metadata blocks change fingerprints. Profile counts 225/362/366/417, host flags false/false/false/true and runtime/oracle/hardware claims remain unchanged. Push ordinary combined CI for 3,087 expected tests; this report-only source validation establishes no new test or milestone acceptance. No archive/extraction retention, local build/test/codegen or new filing. |
| 2026-10-16 | Documentation guard correction and focused client contracts | Ordinary run 38902988661 fails before Nextest because core-surface rejects docsrs feature labels; the five other jobs pass. Preserve bounded failure evidence and restrict the exception to documentation-only attributes, with two regressions retaining conditional-export/path refusals. Add five query fixtures and missing prompt format hints, eight Null-weight fixtures with task-scoped fake encryption, one public blocking PoW-to-plan fixture and one original 83-sample schema/constructor fixture. Seventeen new functions expect 3,104 tests in the next ordinary hosted run; no test success or source-row promotion is claimed. Refresh the historical miner Docify gap and authentication documentation-only identities without extending the cold review. No local builds/tests, services, upstream filing or milestone acceptance. |
| 2026-10-16 | Partial 3,104-test result and readiness-fixture diagnosis | Ordinary 38904971807 at exact29178a0 passes five jobs and 3,103 tests, with one readiness-fixture failure and four skipped. All17newfunctions and61selectedwitnesses pass. The invalid-plan case replaces only the old unchecked M2-14 text after that source-machinery step became checked, so its no-op leaves a valid Q30 declaration. Repair the fixture to exercise both valid states and construct explicit invalid claims; production guard stays intact. Retain bounded partial/failure identities; no raw logs, doctest success, source-row promotion or new milestone acceptance. Local target remains absent. |
| 2026-10-16 | Raw-call review and deterministic readiness repair | Show ordered reviewed raw-call member names/backends before confirmation and report automatic approval planning through existing human output suppression. Add scripted human/quiet/JSON refusal evidence with zero private-source/unlock/author/cache effects and an actual absent-shield-key fallback/forced/required fixture. Strengthen existing planner provenance assertions and split its test attribution into exact canonical headers. Repair the sole readiness failure without changing production readiness/history guards; test count increases by2 to3,106. Independent source review, direct formatting and whitespace pass; hosted tests remain next, source rows pending. Canonical ABI migration remains a separate source/candidate batch. No local build/test, resource campaign, filing or milestone acceptance. |
| 2026-10-16 | Canonical EVM ABI source and hosted candidate preparation | Keep test_evm.py ported/pending and implement its missing canonical-Solidity counterpart comparison through maintained import/extraction. Derive canonical extras from all watched direct SDK JSONs; narrow migration requires exact existing parity commit/origin/tree and21fileinventory, bounds bytes and preserves existing pins/artifacts. Add3importer+6extractionfixtures and a separate hosted canonical dispatch reusing the existing9jobinventory. Retained proof requires all9newnames among21focusedtests and only21ABIinputs+manifest+NOTICE candidates, with no automatic adoption. Independent complete source review, direct formatting and Node syntax pass. Skip ordinary source CI until reviewed hosted import/adoption supplies missing required inputs, then run full applicable hosted gates. No local clone/build/test/import, new filing, source-row promotion or milestone acceptance. |
| 2026-10-16 | Canonical EVM ABI candidate adoption | Hosted run 38930971428 at exact 0dc258b passes normal import/extraction and all 21 focused tests (12 importer, nine extractor). Authenticate artifact 12059321729 (63,793 bytes, SHA256 7db7d069d9165c6c4e7b4c5871836e121dd2a1711b4920bcc24e581811b0ca88), inspect all 24 members in memory, bind 21 ABIs/181,888 bytes to pinned SDK bytes/hash/blob and exact named PASS records, and remove the 21 new manifest/NOTICE lines to reproduce baseline bytes. Separate authenticated refetch adopts exactly 23 source paths; pins, existing artifacts, watched tree, map and generated code remain unchanged. Retain bounded proof only. Preserve current CLI drafts byte-exactly during adoption and restore them after commit. Full ordinary tests, report refresh and EVM row acceptance remain pending. No local builds, tests, imports, retained archives or raw logs. |
| 2026-10-16 | CLI globals and metagraph source corrections | Restrict global proxy types to the existing 19-choice catalog, retain command-owned schemas, and make repeated shield flags use the last occurrence at each scope. Share Python strip/lower normalization across backend validation, prompt policy and wallet/external source routing while retaining original inputs and exact raw-call leaf choices. Restore metagraph local-name precedence, natural ordering, contact fallback, truthy identity display and overview/emission/burn annotations; raw JSON stays intact. Seven globals and two metagraph regression functions exercise actual parsers, lazy injected backends, fake-runtime execution and human output. Independent reviews resolve a fixture API misuse and maximum-burn display expectation; direct formatting and whitespace checks pass. These nine functions and the nine canonical ABI functions await full hosted validation, expecting 3,124 tests. Source rows remain pending and reports require hosted refresh. Ordinary run 38930880204 continues at ab369bd with five jobs passed and its main check active. M2-S02 remains open/unaccepted; no milestone exit or local build/test. |
| 2026-10-16 | Canonical ABI compatibility report adoption | Hosted report run 38933162020 passes at exact ea9d7b7, including extraction, regeneration and source parity. Inspect authenticated artifacts 12059933304 and 12060725242, then independently refetch before adopting three compatibility/weight reports. All 19 generated outputs and the parity matrix remain byte-identical. Of 23 dependency inputs, only the canonical ABI additions in upstream.toml change the fingerprint; role counts, feature graphs, compiler and host flags stay unchanged. Source/matrix remains 312/357 reconciled and 45 pending. This report-only result establishes no new test, oracle, platform or milestone acceptance. No local build/test/codegen or retained archive. |
| 2026-10-16 | Intent prompt and weights help corrections | Whole-module review finds that the unstake-all notice bypasses JSON/quiet suppression and follows signing-wallet confirmation, while missing root-hotkey refusal loses its remediation. Correct those paths and add three actual generated-request fixtures using FakeChain and captured terminal/output events; generic missing-input help remains unchanged. Independent review and formatting pass; hosted execution is pending, bringing the next expected total to 3,127 tests. Restore the weights validating guide, set example, clipping/preflight explanation and forced/automatic/legacy reveal guidance without changing parsing or submission. Replace shorthand source comments with canonical headers in query/schema consumers before their pending map adoption; implementation bodies are preserved. No source-row or milestone acceptance, local build/test or new filing. |
| 2026-10-16 | Green 3,106-test checkpoint and four source dispositions | Ordinary run 38930880204 succeeds at exact ab369bd: all six jobs, 3,106 tests passed, four skipped, nine doctests across 19 suites, documentation upload and final gates. Authenticate 65 selected PASS records, including the repaired readiness case, raw review/refusal, prepare/attach, query, Null weights and public PoW witnesses. Rebind the four complete source audits to current files: schema and query construction/execution bodies are unchanged; separate transaction-prompt changes match their reviewed proposal and remain outside this older result. Adopt query.py, test_prepare_attach.py, test_null_weights.py and test_pow_registration.py source rows. Source reaches 316/357 reconciled and 41 pending; generated matrix remains 312/45 until hosted refresh. Native bounds, fake-encryption, oracle, GPU/live/platform and milestone limits remain explicit. Current canonical/globals/metagraph/prompt work still needs its own full 3,127-test checkpoint. M2-S02 remains open/unaccepted. |
