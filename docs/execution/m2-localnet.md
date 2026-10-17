# M2 localnet scenarios and hosted validation

## Current source and scope

M2-19 and M2-20 are optional targeted hosted infrastructure/integration under
Q30. The retained Localnet/Scenario sources and scenario mappings provide a
harness reference; source presence or historical compilation establishes no
live scenario result. No node, Docker service, full campaign or replacement
local run is required. Q29 waives the testnet soak.

The [e2e](../../.github/deferred-workflows/e2e.yml),
[interop](../../.github/deferred-workflows/interop.yml) and
[nightly](../../.github/deferred-workflows/nightly.yml) workflow drafts are
inactive. Any specifically needed integration selects a small relevant case
on a bounded free hosted runner. Multi-network schedules and a blocking
`e2e-required` summary are outside the development exit. M2-S02 is fixed by
the combined core patch at fork rev `626f2a4f`, and M2-26 is complete.

## Historical preparation and validation

The initial preparation implemented ten M2 scenario entry points and compiled
the source with the core-oracle feature. It did not execute hosted localnet
scenarios. Those compilation results remain historical source evidence.

`Localnet` uses the Docker CLI and digest pins from `upstream.toml`.
`E2E_ENDPOINT` reuses a loopback node; `LOCALNET_IMAGE` and `SKIP_PULL` retain
upstream's override contract. Readiness requires a healthy image or its first
imported-block marker, then positive `Drand.LastStoredRound` within 30 seconds.
A drand timeout receives one fresh-container retry. Owned containers are removed
on Drop or transferred to a workflow report for exact-ID cleanup.

`xtask localnet up` records ownership beneath `RTAO_TEMP_ROOT` and bootstraps the
eight global AdminUtils controls through Sudo. One atomic batch funds 64 slot
owners with 10,000 TAO and their hotkeys with 100 TAO, totaling 646,400 TAO.
Owner, validator and additional miner identities are distinct. Every Alice
submission holds the endpoint's advisory file lock from fresh nonce lookup
through inclusion. `compose::sudo` preserves an existing wrapper and checks the
runtime stamp.

`Scenario` submits generated setup calls directly through `Client`; intent
usage is confined to test dev-dependencies. A subnet comes from the scenario's
own `NetworkAdded` receipt and verified owner. Per-subnet setup sets rates,
minimum weights, CR enablement, mechanisms and version keys. Fast CR uses tempo
48; the slow profile uses tempo 10. Framework fixtures write only published
localnet development keys beneath the recorded root. They close their contexts,
retain bounded metrics, inspect the fixture tree for symlinks, then remove the
exact development wallet and state directory.

The retained scenario implementations target:

- 1: `NeuronRuntime` produces one commit per actual epoch for 20 fast or three
  slow epochs. A gap-free finalized-block observer requires every matching
  reveal, the stored miner row and incentive movement.
- 2: direct weights reach inclusion, fail within the chain rate window, and
  succeed after it; `LastUpdate` and the stored row are checked.
- 3: the production scheduler commits both mechanisms, observes their reveals
  and advances each mechanism's `LastUpdate` entry.
- 5: an owner version bump causes scheduler refusal with remediation. An
  explicit policy update resumes submission.
- 6: global CR version 5 refuses the framework before signing or submission,
  while typed `SetWeights` receives `IncorrectCommitRevealVersion` from chain.
  The global value is restored even when the scenario fails.
- 7: a loopback proxy forwards commit bytes once, then cuts the production
  Session socket after the node acknowledges the watch. The scheduler must
  reconcile a successful finalized receipt, retain its reveal watch, and submit
  the next call with the immediately following nonce.
- 8: CI extracts the fast runtime wasm from the digest-pinned `main` image.
  A non-increasing spec prints an explicit skip reason. Otherwise an active
  scheduler spans `System.set_code`, the production cache switches codec and
  verifies the new surface, and an Executor held at key resolution rebuilds
  exactly once with one key unlock. Compatible weight calls resume submission;
  incompatible calls must remain paused without submitting. A spec bump alone
  does not imply an incompatible weight shape.
- 15: a dedicated unregistered hotkey proves the native zero Owner default and
  its `None` interpretation.
- 16: the isolated PR-U4 workspace requires registry `w3f-bls = 0.1.3`, records
  its source metadata, compares deterministic ciphertext with the workspace
  implementation, decrypts a recorded past round and submits a live future
  round that must reveal the expected weight row.
- 17: two public Executor instances pipeline 50 transfers. Included SCALE
  extrinsics must carry exactly the uninterrupted nonce range, unique inclusion
  identities and the full recipient balance increase.

The additional protocol tests and six upstream client counterparts remain.
The initial `parity/e2e_map.toml` snapshot covered all 115 upstream keys: six source mappings,
one retired-constructor exemption and 108 later-milestone entries. Mapping a
source implementation does not claim its hosted run passed.
`xtask localnet contracts --check` rejects missing, ignored and conditionally
omitted required entry points before CI provisions a node.

The archived `e2e.yml` design uses free `ubuntu-latest` runners. Its former
push branches select pinned fast and slow
plus mainnet; nightly adds testnet and devnet, and weekly adds main. Each job
owns two partition nodes, a serialized global node, and a main fast-runtime
source node. Global state scenarios use the separate invocation and test group.
Cleanup validates exact container IDs and the harness label, attempts every
owned container, and retains bounded node logs, JUnit results, runtime hash and
probe JSON. The non-matrix required gate admits documentation-only skips and
fails if a required chain job fails or is cancelled.

The archived `interop.yml` design supplies the Rust core-oracle lane on one finalized setup block:
storage, metagraph, neurons, hyperparameters, stake and fees on identical full
extrinsic bytes. All 21 non-retired trusted core constructors have independent
generated runtime-call byte counterparts. Actual M2 Transfer/TransferAll
intents cover 12,000 Policy comparisons, including fees, bounded and unbounded
spend, subnet allowlists and raw-call permissions. The remaining 18 semantic
intent counterparts belong to M5 and are printed as coverage gaps. The upstream
blocking client is confined to the differential test's blocking task.
The archived `nightly.yml` design invokes `ci/core-canary-smoke.sh` after resolving actual upstream
main and passing its build, surface and offline checks. That script verifies the
main image's OCI source/revision, runs the hosted localnet oracle smoke, retains
bounded evidence and removes its exact owned container.

Local validation on 2026-10-14:

- The original four Localnet tests, four Scenario FakeChain tests, Sudo wrapper
  test and two localnet command/contract tests passed on current Rust and 1.89.
- The complete testkit source, e2e helpers and core-oracle target compile with
  `cargo check --locked --offline -p rtao-testkit --features core-oracle --tests`.
- The e2e, interop and nightly YAML parse; every declared job uses
  `ubuntu-latest`, every embedded Bash step passes `bash -n`, and both standalone
  CI scripts pass Bash syntax checks.
- The isolated PR-U4 lock and full probe build pass with Rust 1.89 offline,
  without edits to `sp-core` or the workspace lock. Cargo metadata identifies
  one active registry BLS package at 0.1.3. The built executable produced its
  299-byte fixed past-round ciphertext.
- Focused current nextest passes the production Session proxy recovery,
  identical ciphertext and Quicknet cross-decryption, and all 12,000 Policy
  comparisons. The strengthened test also requires decryption inside the
  actual registry build and passes independently after that change.
- Nextest discovers 20 e2e cases and two core-oracle cases, with zero ignored.
  The actual `xtask localnet contracts --check` gate passes.
- Final e2e and core-oracle targets compile on current Rust and Rust 1.89;
  scoped strict Clippy passes on both. No live chain scenario has executed
  locally.

Historical local scratch, builds and logs used `target/execute-plan.6Ro0PC`.
That root is retired and absent; no task build/source/tool cache there is a
current live dependency. Preserve shared global Cargo/toolchain caches. Earlier
isolated command rehearsals and lock files were removed after process,
open-file, launchd, configuration, symlink and virtual-environment checks.
The isolated PR-U4 source clone and probe rehearsal were also removed after
their build and decryption checks in both builds finished and the same audit found no
live dependency. The compiled probe and Cargo metadata belonged to that
historical run; they are not current live dependencies. Its recorded source-
manifest path is historical build provenance.
No local Python, Docker node, user wallet, private validator state, commit,
push or PLAN checkbox mutation was performed by this integration.
