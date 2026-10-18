# rtao plan documents

The plan starts at [`PLAN.md`](../../PLAN.md) in the repository root. This directory holds the detailed design, the
parity appendices and the evidence the plan was built from. Everything here describes the state of upstream on
2026-10-13: the RaoFoundation/subtensor monorepo at `850f2755` (tags `v477` for finney and testnet, spec 478 on `main`
and devnet). Later that day `850f2755` was released as `v478` and finney and testnet moved to spec 478, so M0 pins
`v478` ([`PLAN.md`](../../PLAN.md) M0-06). PLAN.md is at v2.1: it records the maintainer's answers of 2026-10-13 to the
open questions, including three given after v2 (PR-U17 carried as a Cargo patch of the core until it ships; one goal
loop that commits to `main` without PRs; the Ledger app fix filed with Zondax before the plan runs).

The plan deliberately carries no time estimates (a user decision): it states order, dependencies and objective exit
checks, and the effort figures in the archive documents ([`research/`](research/), [`panel/`](panel/),
[`review/`](review/)) are superseded.

| Path | Contents | Status |
|---|---|---|
| [`../../PLAN.md`](../../PLAN.md) | approach, decisions (with the maintainer's answers of 2026-10-13), parity and test strategy, rules for executing the plan step by step, milestones M0-M10 as numbered steps with objective exit checks, risks, progress log | authoritative entry point |
| [`architecture.md`](architecture.md) | the design: boundary with `bittensor-core`, crates, chain client, intents and reads, units, wallets and signers, btauth/1, metagraph, the neuron framework, CLI, testing, upstream tracking, docs, milestones, risks, decision log, divergence register, upstream PR plan, CI workflows | authoritative design |
| [`parity-inventory.md`](parity-inventory.md) | the 83 intents, 102 reads, 18 error codes and the CLI command tree, each with its rtao home, body kind and milestone | authoritative; seeds `parity/pending.toml` and `parity/e2e_map.toml` |
| [`python-module-map.md`](python-module-map.md) | every file of `sdk/python/bittensor` with its rtao home, kind and milestone | authoritative; seeds `parity/map.toml` |
| [`research/`](research/) | eleven research reports (A1-I), three measured feasibility checks, [`CORRECTIONS.md`](research/CORRECTIONS.md), and [`J-ledger-ci-shim.md`](research/J-ledger-ci-shim.md), the Ledger-in-CI research done after the maintainer's decisions, with the core-patch prototype | evidence; `CORRECTIONS.md` overrides the reports, and `architecture.md` wins wherever they disagree |
| [`panel/`](panel/) | the four competing architectures (A mirror-Python, B type-driven, C operator-first, D minimal custom surface) and the three judge reports | historical; `architecture.md` §16 records what was adopted and what was rejected, and why |
| [`review/`](review/) | the adversarial review: `review-<lens>.md` (findings), `verify-<lens>.md` (skeptic verdicts), `fix-log.md` (every applied change, by finding id), `critic.md` (final completeness pass), `draft-notes.md` (gaps found while drafting) | audit trail of every change made after the first draft |
| [`bench/`](bench/) | the experiments behind measured claims (below) | reproducible |

## How the plan was produced

1. **Research.** Eleven parallel reports covered `bittensor-core`, the v11 Python package (intents, transport, reads,
   metagraph, codegen), the runtime and pallets (extrinsics, hyperparameters, storage, runtime APIs), neuron
   protocols, existing Rust efforts, the Rust stack, the owner/validator/miner lifecycle, testing and upstream
   tracking, and the legacy SDK with the bittensor.com docs corpus. Feasibility builds confirmed that subxt 0.51 codegen
   works against finney, that `bittensor-core` builds as a git dependency with the replicated `w3f-bls` patch, and that
   both share one SCALE stack.
2. **Design.** Four independent architectures were scored by three judges (chain fidelity, maintainability,
   performance and developer experience). `architecture.md` synthesizes them on the consensus base (D) with grafts from
   A, B and C, and resolves every judge concern (its appendices A and B).
3. **Drafting.** The inventories were generated from the bittensor.com catalogs and checked against the Python source
   (zero differences). `PLAN.md` was written from the architecture.
4. **Review.** Six reviewers (chain correctness, Python parity, the core boundary, Rust/async/DX, testing and upstream
   tracking, milestones) filed 144 findings. A skeptic per lens re-verified each one against the sources: 71
   confirmed, 72 confirmed with a corrected fix, 1 refuted. Editors applied them to `architecture.md`, the inventories
   and `PLAN.md`. A final completeness critic fixed seven more major gaps. Three findings were blockers in the original
   design (netuid-0 alpha amounts, conflicting rustls crypto providers in `host` builds, and wrapper-call shape hashes
   that recursed through `RuntimeCall`); all three are resolved in `architecture.md`.
5. **Decisions (v2).** The maintainer answered the open questions on 2026-10-13. PLAN.md v2 records each answer and its
   consequences (PLAN.md §14.1, architecture.md §15.2), removes every time estimate, gives every milestone step a
   stable ID and every milestone numbered exit checks, and adds the rules for executing it step by step.
   The Ledger answer (Q17) asked whether a shim could automate Ledger testing. Three more research runs (the protocol
   and the core's integration points, the emulator tooling, a hands-on run against Ledger's emulator) are consolidated
   in [`research/J-ledger-ci-shim.md`](research/J-ledger-ci-shim.md), which led to architecture.md §11.9 and PR-U17.
6. **Decisions (v2.1).** Three later answers of the same day: rtao's own workspace carries PR-U17 as a Cargo patch of
   `bittensor-core` until a `core_tag` contains it ([`PLAN.md` §5.10](../../PLAN.md#510-the-pr-u17-core-patch),
   [architecture.md §1.11](architecture.md#111-the-pr-u17-core-patch)); M0 to M9 are developed as one goal loop
   that commits to `main` without PRs, with the jobs that open PRs off until a handover
   ([`PLAN.md`, Executing this plan](../../PLAN.md#executing-this-plan)); and the Ledger app fix was filed with Zondax
   before the plan runs. A prototype of the core patch in Docker then ran the core's own Ledger code against the
   emulator through PR-U17's seam and showed that the patch needs a second table
   ([`research/J-ledger-ci-shim.md` §8](research/J-ledger-ci-shim.md#8-core-patch-prototype)).

## Path conventions in these documents

- `UP/`, `CORE/`, `PY/`, `PYT/`, `CAT/`, `REG/` and `DOTAPP/` are defined in
  [architecture.md, Evidence conventions](architecture.md#evidence-conventions). `UP/` is the monorepo at commit
  `850f2755bb05a3851f948489f0ab27b0645cebce`.
- `SP/` is the planning session's scratch workspace, which was not kept. `SP/upstream/<dir>` and `upstream/<dir>` are
  the shallow clones listed below. `SP/meta/*` were metadata dumps, reproducible with `subxt metadata` against the
  named network at the named spec. `SP/design-bench*`, `SP/design-tagpin` and `SP/plan/ap-check` are in
  [`bench/`](bench/). Helper scripts mentioned under `SP/` (inventory renderers, edit scripts) were not kept; the
  documents they produced are here.

## Benchmarks and experiments

| Directory | What it shows | How to run |
|---|---|---|
| [`bench/decode-bench/`](bench/decode-bench/) | metagraph decode strategies on live finney payloads (netuids 1, 3, 64) and a synthetic 2,500-UID payload: `bittensor-core` `Value`, typed parity-scale-codec, `DecodeAsType`, and a borrowed view; results in `bench-run-*.txt` | `cargo run --release -- <data dir>`; the data directory needs `v15.optionhex` (the `Metadata_metadata_at_version(15)` result) and `mg_<netuid>.hex` (`SubnetInfoRuntimeApi_get_metagraph` results) captured at the finney block in `data/head.txt`; the 1.1 MB of hex payloads are not committed |
| [`bench/nopatch/`](bench/nopatch/) | without the replicated `[patch.crates-io] w3f-bls` line, Cargo locks two `w3f-bls 0.1.3` sources and `bittensor-core` fails to build (E0277) | `cargo build` |
| [`bench/tagpin/`](bench/tagpin/) | `bittensor-core` pinned by tag `v477`, without `host`: 241 packages, 12.9 s release build (`tagpin.log`) | `cargo build --release` |
| [`bench/arbitrary-precision/`](bench/arbitrary-precision/) | `serde_json`'s `arbitrary_precision` feature, which `bittensor-core` turns on for every crate in the build, breaks `#[serde(untagged)]` and `#[serde(flatten)]` parsing of non-integer numbers and makes `Value` equality spelling-sensitive | `cargo run` versus `cargo run --features ap` |

Each project declares an empty `[workspace]` table so it stays outside the rtao workspace.

## Upstream repositories examined

All clones were taken on 2026-10-13. The legacy `opentensor/*` repositories were archived in July 2026; the monorepo
supersedes them.

| Directory | Remote | Commit | Commit date |
|---|---|---|---|
| `upstream/async-substrate-interface` | <https://github.com/opentensor/async-substrate-interface> | `e48b55482b38efb5f1e5c9476fa02f325a0cfa8a` | 2026-07-10 |
| `upstream/bittensor-commit-reveal` | <https://github.com/opentensor/bittensor-commit-reveal> | `36a729139c34af26cd88c2d2d03097b99b07cd45` | 2026-07-10 |
| `upstream/bittensor-drand` | <https://github.com/opentensor/bittensor-drand> | `36a729139c34af26cd88c2d2d03097b99b07cd45` | 2026-07-10 |
| `upstream/bittensor-subnet-template` | <https://github.com/opentensor/bittensor-subnet-template> | `539b77c30d60dea236ab1c48a3bde7f495caf516` | 2026-07-10 |
| `upstream/bittensor` | <https://github.com/opentensor/bittensor> | `84152b5dbcc3f87d63ae0e4be25433aa22d88700` | 2026-07-10 |
| `upstream/bt-decode` | <https://github.com/opentensor/bt-decode> | `93cad1363e3058f86305c67c7ec1f20e24b1abb5` | 2025-10-28 |
| `upstream/btcli` | <https://github.com/opentensor/btcli> | `8da4626c5113745e1081109b3fe6d710665fb46b` | 2026-07-12 |
| `upstream/btwallet` | <https://github.com/opentensor/btwallet> | `5d63be571deb926aca5597c098ae26d59b85b314` | 2026-07-12 |
| `upstream/CortexLM_bittensor-rs` | <https://github.com/CortexLM/bittensor-rs> | `1ff7ff7b513f197bcbdb7ad54cfe6d78fecaef8a` | 2026-04-22 |
| `upstream/CortexLM_buyback` | <https://github.com/CortexLM/buyback> | `ef219909423cec27b00aab89c2a722c355135bcc` | 2026-09-26 |
| `upstream/developer-docs` | <https://github.com/opentensor/developer-docs> | `1d95723c369abdb81c8b7f96356d624f5462ae76` | 2025-06-19 |
| `upstream/ErgodicLabs_btt` | <https://github.com/ErgodicLabs/btt> | `cf6d9c7af0f8d18e84ba82512b1751984b3e579a` | 2026-07-09 |
| `upstream/fiber` | <https://github.com/rayonlabs/fiber> | `a41ab890708757140a3cf4aae8e5af57a8b03159` | 2025-12-20 |
| `upstream/inference-labs-inc_btlightning` | <https://github.com/inference-labs-inc/btlightning> | `53c090b436238f78a4a0dd30cbbfcba1cbebe2e0` | 2026-08-05 |
| `upstream/inference-labs-inc_subnet-2` | <https://github.com/inference-labs-inc/subnet-2> | `96d03c44729a350df79160e2ea8e3e175d492ce5` | 2026-09-08 |
| `upstream/k0jir0_hermes` | <https://github.com/k0jir0/hermes> | `95f5780a7dbeb65cfa14b1e5d147d88447bfdeee` | 2026-06-05 |
| `upstream/one-covenant_bittensor-rs` | <https://github.com/one-covenant/bittensor-rs> | `0f27fd41f67b45bdd806f510dbf70c46dccd773f` | 2026-07-20 |
| `upstream/prompting` | <https://github.com/macrocosm-os/prompting> | `da4a308ea7e097a1e357937e5a322a54bba9cc41` | 2026-10-02 |
| `upstream/RaoFoundation_bittensor-subnet-template` | <https://github.com/RaoFoundation/bittensor-subnet-template> | `539b77c30d60dea236ab1c48a3bde7f495caf516` | 2026-07-10 |
| `upstream/sam0x17_mev-scanner` | <https://github.com/sam0x17/mev-scanner> | `16647f0ae4b6d2623b9cde8afebc5ec8674d32d6` | 2026-02-16 |
| `upstream/subtensor` | <https://github.com/RaoFoundation/subtensor> (cloned via the `opentensor/subtensor` redirect) | `850f2755bb05a3851f948489f0ab27b0645cebce` | 2026-10-13 |
| `upstream/timehash13_subtensor-api-rs` | <https://github.com/timehash13/subtensor-api-rs> | `86d762ad10c7bb3cf395d1e4d983e467445451f5` | 2026-03-30 |
| `upstream/unarbos_agcli` | <https://github.com/unarbos/agcli> | `0ec313200f468ea6a63d51ba4cc97bd2898d1ee0` | 2026-05-29 |
| `upstream/unconst_Chi` | <https://github.com/unconst/Chi> | `126080c45aa7585705244c6992f420dd9e10b1f8` | 2026-03-16 |
| `upstream/womboai_rusttensor` | <https://github.com/womboai/rusttensor> | `df75a0097af430253d86f5cc9b76e40bd158069c` | 2024-11-22 |
