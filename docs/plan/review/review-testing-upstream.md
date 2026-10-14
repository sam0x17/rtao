# Review: testing and upstream tracking

Lens: PLAN.md §9 (testing), §10 (upstream), §11 (docs), §12 exit criteria, §14.2-§14.3; architecture.md §11, §12, §13,
Appendix E; inventories where they define test scope. Evidence: `UP/` at `850f2755` (`sdk/` = `v477`), live GHCR tag list,
GitHub releases API, PyPI JSON, finney RPC, `scratchpad/meta`, `REG/` (local cargo registry). No cargo or Python was run
inside upstream directories.

Verified with no finding: `PYT/fixtures/golden.json` (1,508,346 B, spec 461, V15+V14 metadata; 10 calls, 10 storage keys,
2 signature payloads immortal/mortal-64, 2 extrinsics, 3 multisig, 4 ss58); `shape_corpus/corpus.json` (797/802 types,
2,899 samples); `ledger_proof_vector.json`; `CORE/timelock/epoch_schedule_vectors.rs` (private module); digest
`affbcc25…11c0` (`CORE/digest/mod.rs:184-201`); PoW seals and `twox_128("System")`; btauth vectors
(`test_http_auth.py:19-36`, `signed-requests.mdx:252-289`); every table-shaped PYT unit test named in §11.2;
`e2e-manifest.json` = 112 (73 `intent_*` + 39 `test_*`, all present in `e2e.rs`); `PY/_generated/*.py`; `record_golden.py`,
`record_shape_corpus.py`, `bench_*.py`; upstream docker harness (`e2e_support/mod.rs`, `E2E_ENDPOINT`/`LOCALNET_IMAGE`/
`SKIP_PULL`, 180 s, `Imported #1`, `monorepo-sdk` tag absent); GHCR `ghcr.io/raofoundation/subtensor-localnet` tags
`v432 v473 v475 v477 mainnet testnet devnet main latest`; image layout (`/target/{fast,non-fast}-runtime/...`, wasm path
used by scenario 8, HEALTHCHECK); `SUBTENSOR_LOCALNET_RUNTIME_WASM`; release `v477` final, `v476` pre-release; Alice-sudo
setters present in 477 metadata (`sudo_set_network_min_lock_cost` 36, `sudo_set_lock_reduction_interval` 38,
`sudo_set_weights_set_rate_limit` 7, `sudo_set_serving_rate_limit` 3, `sudo_set_tx_rate_limit` 2,
`sudo_set_commit_reveal_version` 71, `sudo_set_subnet_limit` 37); `subxt-cli 0.51.1` (`subxt diff` exists); cargo-audit
0.22.2 pin; finality smoke (`ts-tests/.../test-block-finalization.ts`); release-process doc; churn table (exact);
`btcli` entry point, Typer, `--n-words`, `--no-password`, `BT_WALLET_PASSWORD`; FastAPI recipe at
`signed-requests.mdx:111-141`; live catalogs are byte-identical to `CAT/` at `850f2755`.

---

## TU-01 (blocker, architecture.md) Exact call hashes include the whole `RuntimeCall`, so any new call anywhere breaks 12 intents

Location: §3.7 "Shape hashes" (`node(id) = Composite(fs) … Variant(vs) …`, lines 1164-1171; "exact, for calls (argument
names, order and types)", 1175) and "an intent whose wrapped call changed shape returns `TxError::SurfaceChanged`" (1185);
§12.5 `breaking` row "changed args of a wrapped call" (3587) vs `additive` "new calls" (3585).

Problem: every call that carries a nested call takes the outer `RuntimeCall` enum as an argument type, so its exact hash
recurses through every pallet's call enum. Adding or changing any call anywhere changes the exact hashes of
`Utility.batch_all`, `Multisig.as_multi`, `Multisig.as_multi_threshold_1`, `Proxy.proxy_announced` and `Sudo.sudo`. The
drift job then classifies a routine call addition as `breaking` (minor bump per §12.8, spec range not "validated"), and
`verify_surface` fail-closed makes every rtao build compiled before that spec refuse the 12 catalog intents that wrap
these calls: `batch`, `remove_stake`, `remove_stake_limit`, `move_stake`, `move_swap_stake`, `unstake_all`,
`multisig_approve`, `multisig_execute`, `multisig_threshold_1`, `execute_proxy_announced`, `set_root_claim_threshold`,
`set_subnet_emission_enabled`. The Scenario builder's Alice `Sudo.sudo` knobs also fail on `testnet`/`devnet`/`main`
images ahead of the pin, so the early-warning e2e lanes turn red for a self-inflicted reason.

Evidence: finney 477 metadata (`scratchpad/meta/finney.json`): `Utility.batch_all(calls)` field typeName
`Vec<<T as Config>::RuntimeCall>`, element type 141 = `outer_enums.call_enum_ty`; `Proxy.proxy_announced(…, call)`,
`Multisig.as_multi(…, call, …)`; `jq` over `CAT/intents.json`: the 12 intents above wrap `Utility.*`, `Sudo.*`,
`Multisig.as_multi[_threshold_1]` or `Proxy.proxy_announced`; `CORE/codec/encode.rs:528-532` ("Param values may embed
pre-composed calls as `Value::Bytes` … they splice in verbatim"), so rtao never encodes `RuntimeCall` structurally; added
`#[pallet::call_index(` lines per release pair (`git diff` over `pallets`, `runtime/src`): v467→v468 1, v469→v473 1,
v473→v475 4, i.e. 3 of the last 6 pairs.

Fix: in §3.7 after the `node(id)` block add: "Outer enums are opaque. When `id` is V15 `outer_enums.call_enum_ty`,
`event_enum_ty` or `error_enum_ty`, `node(id) = ("O", "RuntimeCall" | "RuntimeEvent" | "RuntimeError")`. Nested calls are
spliced as bytes by `compose_call` (`CORE/codec/encode.rs:528-532`), and each inner call is verified by its own descriptor
when it is composed, so a wrapper's hash covers only its own arguments. Property test: adding a variant to an unrelated
pallet's `Call` enum leaves the exact hashes of `Utility.batch_all`, `Multisig.as_multi`, `Proxy.proxy_announced` and
`Sudo.sudo` unchanged." In §12.5 append to the `additive` row: "a new call in any pallet (wrapper-call hashes treat
`RuntimeCall` as opaque, §3.7)". Add a drift unit test: committed 477 V15 plus one synthetic `SubtensorModule` call
classifies `additive`, not `breaking`.

## TU-02 (major, architecture.md) Bot PRs opened with `GITHUB_TOKEN` never run CI

Location: Appendix E `drift.yml` (`GH_TOKEN: "${{ secrets.GITHUB_TOKEN }}"` … `--apply --open-pr`) and `pin-bump.yml`
(`gh pr create` with `secrets.GITHUB_TOKEN`); §12.2 "`surface-unchanged` auto-merges"; pin-bump "merge needs green
`pr-gates`, `e2e` on `:vS`, `interop`".

Problem: branches pushed and PRs opened with the repository token do not start `pull_request`/`push` workflow runs. Drift
and pin-bump PRs therefore get no pr-gates, e2e or interop runs: auto-merge waits forever on required checks (or merges
untested if none are required), and pin-bump PRs cannot meet their merge conditions without a manual close and reopen.
The YAML also never creates or pushes the head branch before `gh pr create`.

Evidence: GitHub Docs, "Automatic token authentication": events triggered by the `GITHUB_TOKEN`, except
`workflow_dispatch` and `repository_dispatch`, do not create new workflow runs; Appendix E lines 4201-4203 and 4237-4243;
PLAN §10.6 counts on "drift PR auto-merges" for zero human work.

Fix: in both jobs mint a GitHub App installation token (`actions/create-github-app-token@<sha>`, `app-id: ${{
vars.RTAO_BOT_APP_ID }}`, `private-key: ${{ secrets.RTAO_BOT_PRIVATE_KEY }}`; contents and pull-requests write). Check out
with that token, then run `git switch -c bot/pin-<tag>` (drift: `bot/drift-<net>-<spec>`), `git commit -am …` and
`git push -f origin HEAD`, and run `gh pr create` / `gh pr merge --auto --squash` with `GH_TOKEN` set to the app token.
Branch protection requires `pr-gates`, the e2e aggregator (TU-05) and, on `pin-bump` PRs, `interop`. State in §12.2: "bot
PRs are opened with the rtao-bot App token so that CI runs on them."

## TU-03 (major, architecture.md) `pin-bump` races upstream publication of `:vS` and of finney's spec-S metadata

Location: §12.3 step 3 "descriptors from `vendor/metadata/finney/<N>.v15.scale` (fetched by the drift job)", step 4
"`xtask record` runs upstream's recorders against `:vN`"; Appendix E `pin-bump.yml` `--check` comment ("final release vS
exists AND finney finalized spec == S AND S > parity.spec") and the single `continue-on-error` step that runs oracle
build, oracle run and record.

Problem: the bump fires as soon as vS is final and finney reports S, but two of its inputs are published later. The
`:vS` image is built only after the mainnet mirror push, which happens in the same watcher job that finalizes the
release. `vendor/metadata/finney/S.v15.scale` is written by the 6-hourly drift job and merged through its own PR. A
30-minute poll lands inside both windows: `xtask record --image …:vS` fails inside the continue-on-error oracle step, so
the PR is labelled `oracle-stale` although the oracle built; `[localnet].pinned` gets no digest; `xtask regen` has no
spec-S metadata.

Evidence: `UP/.github/workflows/watch-mainnet-release.yml:9-14` ("That push publishes the :mainnet, :v<spec_version> …
localnet images"), `:266` (mirror push) and `:275` (release promoted) in the same `release` job;
`UP/.github/workflows/docker-localnet.yml:15` (`push: branches: [main, devnet, testnet, mainnet]`), `:124`
(`timeout-minutes: 90` for the node build from source), `:193` (image smoke test, 20 min); arch §12.2 drift cron
`17 */6 * * *` vs pin-bump `*/30`.

Fix: §12.3 step 1 add "…and an anonymous `GET https://ghcr.io/v2/raofoundation/subtensor-localnet/manifests/vS` succeeds;
its digest is written to `[localnet].pinned`. Otherwise `bump=false` and the next poll retries." Replace step 3's "(fetched
by the drift job)" with "fetched by `xtask pin` itself at finney's finalized block (`Metadata_metadata_at_version(15|16)`,
the drift job's step 2) and committed in the PR". In `pin-bump.yml` split the step into `oracle` (build and run; failure
labels `oracle-stale`) and `record` (`continue-on-error: true`; failure labels `recordings-stale`).

## TU-04 (major, architecture.md) Upstream's recorders cannot record spec 477 as specified

Location: §11.2 "Spec-current fixtures. … upstream's own recorders, run by `xtask record` inside the oracle image against
the `:v<spec>` localnet … rtao's loaders run unchanged" (3279-3283); PLAN §9.2 last bullet; §16 decision #38; §11.6 lane
4 ("`record_golden.py` mode").

Problem: `record_golden.py` hard-codes `SubtensorModule.Alpha`, which no longer exists in spec-477 metadata (only
`AlphaV2`), so `storage_entry` raises `StorageFunctionNotFound` and aborts the whole recording. It also refuses any
endpoint other than localhost unless `RECORD_GOLDEN_ALLOW_REMOTE=1` is set, and the oracle container reaches the localnet
through a compose service name. `record_shape_corpus.py` does not talk to the localnet at all: it works offline from the
registry in `tests/fixtures/golden.json`, so it yields a spec-current corpus only after a successful golden re-record.
Interop lane 4 inherits the same failure.

Evidence: `UP/sdk/python/scripts/record_golden.py:85` (`("SubtensorModule", "Alpha", …)`), `:39` (OUT is
`tests/fixtures/golden.json`), `:41-49` (localhost guard); `PY/_transport/codec.py:257-261`;
`scratchpad/meta/explore/SubtensorModule_storage.txt` (only `AlphaV2`); `UP/pallets/subtensor/src/lib.rs:1975`;
`record_shape_corpus.py:12` ("Runs fully offline"), `:34, :152` (`golden_codec()` from `tests/conftest.py:17-26`).

Fix: replace the paragraph with: "`xtask record` runs rtao's `ci/oracle/record.py` in the oracle container joined to the
localnet's network namespace (`docker run --network container:<localnet>`), so the endpoint is `ws://127.0.0.1:9944` and
the localhost guard holds. `record.py` imports upstream's `scripts/record_golden.py` as a module, drops every `*_CASES`
entry whose pallet, item, call or runtime API is absent from the live metadata (listing them in
`vendor/recorded/<spec>/dropped.json`; today `SubtensorModule.Alpha`), calls its `record()` and writes the checkout's
`tests/fixtures/golden.json`. It then runs `record_shape_corpus.py`, which derives its registry offline from that file.
Both outputs are copied to `vendor/recorded/<spec>/`. rtao files an upstream fix replacing `Alpha` with `AlphaV2`;
interop lane 4 uses the same wrapper."

## TU-05 (major, architecture.md) `e2e` cannot be a required check as written, misses its own inputs, and cannot block `main`

Location: Appendix E `e2e.yml` (`on: pull_request: paths: […]`, `push: { branches: [main] }`, 4157-4172); §11.4 lane table
("`pinned` … required on PRs that touch transport, executor, intents or reads"; "`testnet` … opens a pre-break issue
instead of blocking") vs §12.2 ("`mainnet`, `testnet` blocking on `main`") and PLAN §9.4/§9.7.

Problem:

1. A workflow skipped by a workflow-level `paths` filter leaves its checks pending. Requiring `pinned` blocks every PR
   outside the filter; not requiring it makes the lane advisory.
2. The path list omits code the suite exercises: `crates/rtao-testkit/**` (the e2e tests themselves), `rtao-core-ext`
   (staged receipts, fee probe, epoch schedule, timelock commit), `rtao-core`, `rtao-runtime`, `rtao-signer`,
   `rtao-wallet`, `rtao-btauth` (scenario 9), `rtao-catalog`, `rtao-primitives`, `Cargo.toml`/`Cargo.lock` (Renovate
   bumps of tokio-tungstenite and reqwest), `codegen/**`, `ci/**` and the workflow file itself.
3. `push` runs on `main` start after merge, so they cannot block anything. `testnet` is described as both blocking and
   pre-break.

Evidence: GitHub Docs, "Handling skipped but required checks" (a path-filtered workflow that does not run leaves its
required checks Pending); upstream avoids workflow-level paths for this reason, `UP/.github/workflows/
check-bittensor-e2e-tests.yml:17-19`, with an always-run `plan` job; Appendix E 4159-4160.

Fix:

- Triggers: `pull_request` (no `paths`), `merge_group`, `schedule`, `workflow_dispatch`.
- First job `changes` always runs and sets `run=false` only when every changed file matches `book/**`, `docs/**`,
  `**/*.md` or `templates/**`. Job `localnet` runs `if: needs.changes.outputs.run == 'true'`.
- Final job `e2e-required` (`if: always()`) fails only when `localnet` failed. Make only `e2e-required` a required check,
  and enable the merge queue.
- Lanes:
  - pull_request: pinned fast and slow.
  - merge_group: pinned and `mainnet`.
  - nightly: everything, plus `testnet` (allowed failure; opens or updates a `pre-break` issue with `gh issue`), `devnet`
    and `main`.

Use this wording in §11.4, §12.2 and PLAN §9.4/§9.7.

## TU-06 (major, architecture.md) The localnet knob set leaves the admin freeze window, owner rate limit, slow-lane tempo and subnet limit in place

Location: §11.4 Isolation, "Alice-sudo knobs keep subnet creation cheap and fast. They lower the doubling lock cost (…)
and the per-test rate limits (…)" (3351-3354); scenario 1 ("3 slow" epochs), 5, 10.

Problem: on this runtime the default admin freeze window (10 blocks) with fast tempo 10 makes
`is_in_admin_freeze_window` true in every block but one per epoch. Owner and root hyperparameter calls (scenario 5's
`sudo_set_weights_version_key`, scenario 10's planner, owner flows) therefore fail with
`AdminActionProhibitedDuringWeightsWindow`, and the owner hyperparameter rate limit also stays in force. On the 12 s lane,
new subnets get tempo 360, so "3 slow epochs" plus the reveal epoch take about 4 h. In a shared container, `SubnetLimit`
128 combined with a 1,296,000-block network immunity makes registration #129 fail with `SubnetLimitReached`.

Evidence: `UP/pallets/subtensor/src/lib.rs:1436-1438` (`DefaultAdminFreezeWindow` = 10); `UP/runtime/src/lib.rs:904`
(`INITIAL_SUBNET_TEMPO = prod_or_fast!(360, 10)`); `utils/misc.rs:59-71` and `coinbase/run_coinbase.rs:1247-1255`;
`UP/pallets/admin-utils/src/lib.rs:444-454` (`ensure_admin_window_open` for owner and root); upstream's localnet tests
zero these knobs: `UP/sdk/python/tests/e2e/test_null_consensus.py:39-46`, `test_pow_registration.py:33-38`;
`coinbase/tempo_control.rs:11-22` (root may set any tempo); `lib.rs:1454-1456` (`SubnetLimit` 128),
`UP/runtime/src/lib.rs:962` (immunity 1,296,000), `subnets/subnet.rs:205-213`.

Fix: replace the knob sentence with:

- "`Localnet::bootstrap()` runs once per container, as Alice through `Sudo.sudo`: `sudo_set_admin_freeze_window(0)`,
  `sudo_set_owner_hparam_rate_limit(0)`, `sudo_set_network_rate_limit(0)`, `sudo_set_tx_rate_limit(0)`,
  `sudo_set_start_call_delay(0)`, `sudo_set_network_min_lock_cost(1 TAO)`, `sudo_set_lock_reduction_interval(1)`,
  `sudo_set_subnet_limit(u16::MAX)`.
- Per subnet, by the subnet owner where owner-settable: `sudo_set_weights_set_rate_limit(netuid, 0)`,
  `sudo_set_serving_rate_limit(netuid, 0)`, `sudo_set_min_allowed_weights(netuid, 0)`. On the 12 s lane, also root
  `sudo_set_tempo(netuid, 10)` (3 epochs plus reveal ≈ 9 min).
- Scenario 10's planner test runs in the `global` container with `sudo_set_admin_freeze_window(5)` and
  `sudo_set_tempo(netuid, 40)`, to exercise waiting."

## TU-07 (major, architecture.md) "One container per test-binary group" does not exist under nextest; parallel tests race on Alice's nonce

Location: §11.4 Isolation, "One container per test binary group (nextest test groups). Inside it, each test gets a fresh
subnet and freshly funded accounts." (3348-3350); PLAN §9.4.

Problem: nextest runs each test in its own process, and test groups only cap concurrency, so a per-binary `Localnet`
cannot be shared. As written, every test starts its own container, as upstream's harness does per `TestContext::new()`.
Sharing needs a container started outside the test processes. Parallel processes that fund accounts or send sudo knobs
all sign with Alice and race for her nonce, and the ported "priority is too low" retry is bounded, so a blocking lane
becomes flaky.

Evidence: nextest docs ("How nextest works": one process per test; setup scripts are experimental,
`experimental = ["setup-scripts"]`); `UP/sdk/bittensor-core/tests/e2e_support/mod.rs:33-50, 62-139`; `PY/executor.py:52-56`;
Appendix E `e2e.yml` has no container setup step.

Fix: replace the bullet with:

- "`e2e.yml` starts the containers itself: one per nextest partition plus one `global` container.
- It runs `cargo xtask localnet bootstrap --endpoint <ws>` once per container: the §11.4 knobs plus one
  `Utility.batch_all` from Alice funding 64 slot accounts (`//rtao-e2e//slot/<n>` and `//rtao-e2e//slot/<n>//hot`).
- It then runs `cargo nextest run -p rtao-testkit --features docker --profile e2e-<blocks> --partition hash:<k>/<n>` with
  `E2E_ENDPOINT` set.
- A test signs only with the accounts of `NEXTEST_TEST_GLOBAL_SLOT` (nextest ≥ 0.9.90), so concurrent processes never
  share a nonce, and it sets per-subnet knobs as its subnet's owner.
- Tests that change global state (scenarios 6, 8, 10's queued path and planner, 16) are in test group `global`
  (`max-threads = 1`) and run in a second invocation against the `global` container.
- `rtao localnet up` gives developers the same setup."

## TU-08 (major, architecture.md) Commit-reveal scenarios depend on public drand endpoints that the plan never mentions

Location: §11.4 Harness "readiness is the image healthcheck … or `Imported #1` …, with a 180 s cap" (3329); scenario 1
"CR on for 20 fast epochs … every reveal observed"; scenarios 3, 5, 6, 7, 16.

Problem: pallet-drand's offchain worker fetches quicknet pulses from public HTTPS endpoints. Readiness does not wait for
the drand bridge, so an unreachable endpoint shows up as a lost reveal in the blocking `pinned` lane, indistinguishable
from an rtao bug. On the fast lane a reveal epoch lasts about tempo × 0.25 s ≈ 2.5 s, which is less than one drand period
(3 s), so a late pulse expires the commit and "every reveal observed" over 20 epochs is flaky.

Evidence: `UP/pallets/drand/src/lib.rs:81-87` (api.drand.sh, api2, api3, drand.cloudflare.com, secureweb3), `:91-92`
(quicknet hash `52db9ba7…e971`), `:275` (offchain worker); upstream waits up to 30 s for `Drand.LastStoredRound`
(`UP/sdk/bittensor-core/tests/e2e.rs:548-563`; `UP/sdk/python/tests/e2e/test_timelock_commitment.py:38-43`);
`coinbase/reveal_commits.rs:52-58, 72-88, 204-206` (retry within the reveal epoch, then expire); `CORE/timelock/
constants.rs:20` (`DRAND_PERIOD = 3`); `UP/runtime/src/lib.rs:904`.

Fix: add to §11.4 Harness:

- "Readiness also requires `Drand.LastStoredRound > 0` within 30 s, as upstream's e2e does. A timeout raises
  `LocalnetError::DrandUnreachable`; the job retries once with a fresh container, then fails with an infrastructure
  annotation.
- The e2e job first checks egress:
  `curl -fsS https://api.drand.sh/52db9ba70e0cc0f6eaf7803dd07447a1f5477735fd3f661792ba94600c84e971/public/latest`.
- Commit-reveal scenarios on the fast lane set root `sudo_set_tempo(netuid, 48)`, a 12 s reveal epoch spanning 4 drand
  periods, so one late pulse cannot expire a commit and 'every reveal observed' stays strict."

Also state in the local-development guide that the localnet needs outbound HTTPS to the drand endpoints.

## TU-09 (major, architecture.md) The MSRV job runs on stable, and the workspace cannot pass on 1.89 anyway

Location: Appendix E `pr.yml` `msrv` (`dtolnay/rust-toolchain@1.89`; `cargo check --workspace --all-features --locked`,
4125-4130), `nightly.yml` `fuzz` (`dtolnay/rust-toolchain@nightly`; `cargo xtask fuzz`); §2.5 "CI builds once on the MSRV".

Problem: the repo pins current stable in `rust-toolchain.toml`. The dtolnay action only runs `rustup default 1.89`, and
the toolchain file takes precedence over the default, so `cargo check` runs on stable and never tests 1.89. Even with
`cargo +1.89`, `--workspace` includes `xtask`, whose `ruff_python_parser` dependency (git) declares
`rust-version = "1.97"`. The `fuzz` job hits the same override, while cargo-fuzz needs nightly.

Evidence: arch §2.1 (`rust-toolchain.toml # dev/CI toolchain (current stable …)`), §2.5 (1.99), PLAN §12.14;
dtolnay/rust-toolchain `action.yml:111-112` (`rustup default "$toolchain"`, no `RUSTUP_TOOLCHAIN`); rustup precedence
(`+toolchain` > `RUSTUP_TOOLCHAIN` > override > `rust-toolchain.toml` > default); `astral-sh/ruff` main
`Cargo.toml:8` (`rust-version = "1.97"`).

Fix: replace the `msrv` steps with:

```yaml
- uses: dtolnay/rust-toolchain@<sha>
  with: { toolchain: "1.89" }
- run: |
    cargo +1.89 check --locked -p rtao --no-default-features --features validator,miner,prometheus
    cargo +1.89 check --locked -p rtao --features full
    cargo +1.89 check --locked -p rtao-cli
```

Add to §2.5: "MSRV covers the facade, its dependency closure and `rtao-cli`; `xtask` and `rtao-testkit` build only with
`rust-toolchain.toml`." In `fuzz`, set `RUSTUP_TOOLCHAIN: nightly` on the fuzz step (or have `xtask fuzz` call
`cargo +nightly fuzz`).

## TU-10 (major, architecture.md) Chain rules the framework hand-copies come from files nobody watches

Location: §4.9 `map.toml` "Watched roots: … `runtime/src/{fee_filters,check_mortality,transaction_payment_wrapper}.rs`,
`pallets/subtensor/src/rpc_info/**`, `pallets/subtensor/src/coinbase/{run_coinbase,reveal_commits}.rs`, …" (1947);
layer 5.

Problem: the framework mirrors chain rules that Python does not, and no tracking job watches their source files:

- `AxonPublisher` (§9.5): `subnets/serving.rs`.
- `HyperparamPlanner` (§9.7): the freeze window, per-parameter rate limits, bounds and the owner-versus-root split, from
  `utils/misc.rs`, `utils/rate_limiting.rs`, the `lib.rs` constants and `admin-utils`.
- `WeightScheduler`: `subnets/weights.rs`.
- Owner-triggered epochs: `coinbase/tempo_control.rs`.
- The queued-registration follower: `subnets/subnet.rs`.

A behavior-only runtime release such as 478 that changes these files is `surface-unchanged`, auto-merges, and no blob or
symbol diff flags it.

Evidence: §9.5 ("The chain rules come from `pallets/subtensor/src/subnets/serving.rs`"); §9.7 (freeze window,
"tempo 360-50,400", "version key 5 tempos; mechanism count 7,200 blocks; trims 216,000"); `git diff --name-only` over
v466…v477: `utils/rate_limiting.rs` changed in v473→v475 and v475→v477 (v477 adds `Hyperparameter::MinDifficulty = 37`,
`Difficulty = 38`), `utils/misc.rs` and `subnets/weights.rs` in v473→v475, `pallets/admin-utils/src/lib.rs` in 3 of 6
pairs, `pallets/subtensor/src/lib.rs` in 4 of 6.

Fix:

- Add watched rows of kind `ported`:
  - `pallets/subtensor/src/subnets/serving.rs` → the axon publisher;
  - `pallets/subtensor/src/utils/{misc,rate_limiting}.rs` and `pallets/admin-utils/src/lib.rs` → `HyperparamPlanner`;
  - `pallets/subtensor/src/coinbase/tempo_control.rs` → `rtao-core-ext::epoch_schedule` and `HyperparamPlanner`;
  - `pallets/subtensor/src/subnets/weights.rs` → `WeightScheduler`;
  - `pallets/subtensor/src/subnets/subnet.rs` → `OwnerOps::create_subnet`.
- `pallets/subtensor/src/lib.rs` is too large for a blob row. Instead, layer 5 extracts with `syn` `MIN_TEMPO`,
  `MAX_TEMPO`, `MIN/MAX_ACTIVITY_CUTOFF_FACTOR_MILLI`, `DefaultAdminFreezeWindow` and the `rate_limit_on_subnet` arms
  into `vendor/upstream/extracted/chain_rules.json`.
- Playbook row: "chain rule mirrored by the framework changes → blob or extraction diff; ½ day".

## TU-11 (major, architecture.md) PR gates read unvendored monorepo blobs, so they are not offline

Location: §4.9 "`cargo xtask parity check` runs offline on every PR"; layer 1 "listed from a cached partial clone (`git
clone --filter=blob:none`; `git ls-tree -r`)"; layer 4 "re-extracts every literal table from the vendored sources at
`parity_tag`"; §1.2 `[artifacts]`; PLAN §8.3, §9.7 "blocking, offline, no Python".

Problem: the Python and runtime sources that layers 1-5 read are not vendored. `[artifacts]` and `vendor/upstream/` hold
only catalogs, fixtures, static assets and `signed-requests.mdx`. Every PR gate therefore reads blobs from a `blob:none`
partial clone, which fetches blob contents from GitHub on first read and is cached under `target/upstream.git`. On a cache
miss or a GitHub outage the gate needs the network, and "the vendored sources" of layer 4 do not exist.

Evidence: §1.2 `[artifacts]` (243-250: catalogs, golden.json, corpus.json, ledger vector, "PY/vault/static/*,
PY/extension/static/bridge.html, PY/evm/abi/*.json, docs/guides/signed-requests.mdx"); §2.1 `vendor/upstream/`; Appendix E
`check` `cache-directories: "~/.cargo/git\ntarget/upstream.git"`; extraction targets such as `PY/settings.py`,
`PY/_transport/rpc.py` (PLAN §12.3) and the runtime files of layer 5 appear in no artifact list.

Fix: add to §1.2/§4.9: "`xtask import-upstream` also vendors every file under the watched roots at `parity.commit` into
`vendor/upstream/src/<monorepo path>`, with `vendor/upstream/src.lock` (path, blob SHA, sha256) and `vendor/upstream/
tree.txt` (`git ls-tree -r <parity.commit> -- <watched roots>`). PR-gate layers 1-5 read only these files, and
`--verify-only` checks them. The partial clone is used only by `pin-bump`, `upstream-next` and `core-canary`, which need
old and new blobs." PR gates then need the network only for `cargo fetch`.

## TU-12 (major, architecture.md) The oracle's PyPI fallback has no valid tag mapping

Location: §11.5 Build "When a PyPI version maps exactly to the tag, the wheel is used instead" (3392-3393); R5 and PD-13;
§1.2 `python_package_version = "11.3.1.dev0"`; §12.7 "parity tag (Python version)".

Problem: no PyPI version identifies a release tag. The fallback either never applies or, under the obvious
`X.Y.Z.dev0 → X.Y.Z` mapping, runs a different tag's Python as the specification:

- `sdk/python` was `11.3.0.dev0` at nine tags whose Python sources differ, and `bittensor==11.3.0` was published once,
  after v475.
- v477's `11.3.1` is not on PyPI (only `11.3.1rc68`), and neither is `bittensor-core 0.1.6` (only rc68), because stable
  publication waits for a manual `mainnet` environment approval.
- The `.dev0` string in `upstream.toml` and `compat.json` names no installable package.

Evidence: `git show <tag>:sdk/python/pyproject.toml`: `11.3.0.dev0` at v464, v466-v471, v473, v475, v476; `11.3.1.dev0`
at v477 (core-py 0.1.4/0.1.5/0.1.6). PyPI JSON today: `bittensor` 11.3.0 uploaded 2026-10-07T22:56, `11.3.1rc68` on
2026-10-08, no 11.3.1; `bittensor-core` 0.1.6 only as rc68. `UP/.github/workflows/watch-mainnet-release.yml:19-21`,
`publish-sdk` (`:370-377`, `environment: mainnet`); `.github/scripts/prepare-sdk-dist.py` stamps `X.Y.Z.dev0 → X.Y.Z`.

Fix: replace the sentence with "There is no PyPI fallback: if the image cannot be built, the PR is labelled
`oracle-stale`." If a fallback is wanted: "`xtask oracle` may install `bittensor==X.Y.Z` only after verifying that every
file of the wheel's `bittensor/` package has the git blob SHA of the same path at `parity.commit`, and that
`bittensor-core` matches `sdk/bittensor-core-py/pyproject.toml` at `parity.commit` with an unchanged core fingerprint;
otherwise `oracle-stale`." `compat.json`'s Python column records the PyPI version actually tested, or `unpublished`.
Update R5 and PD-13.

## TU-13 (major, PLAN.md) "Compare numbers as f64" lets 1-rao errors pass

Location: §14.3 #1 default "compare numbers as f64"; R19 "compare oracle JSON numerically".

Problem: f64 cannot distinguish integers above 2^53, which is where balances live. Finney's total issuance is
11,614,906,923,326,217 rao; at that size adjacent f64 values are 2 apart, and near `u64::MAX` alpha they are 2,048
apart. Wrong reads, fees and spends would pass the oracle.

Evidence: finney `state_getStorage(Balances.TotalIssuance)` = `0x099b4341b2432900` = 11,614,906,923,326,217 >
2^53 = 9,007,199,254,740,992; arch §5.3 relies on values above 2^53; Python's `_jsonable` keeps integers exact.

Fix: "Compare oracle JSON numbers by token kind. Integer tokens (no `.`, `e`, `E`) compare as exact integers (`i128`/`u128`,
else a normalized digit string); other numbers compare by f64 bit equality after parsing, so `1e-05 == 1e-5` while a 1-rao
difference above 2^53 fails. M0 test: `11614906923326217 != 11614906923326216`, `1e-05 == 1e-5`." Use the same wording in R19.

## TU-14 (major, architecture.md) Appendix E omits or contradicts lanes the plan relies on

Location: Appendix E (4089-4310) vs §11.1, §11.4, §12.2, §13, PLAN §9.7, §12.11.

Problem:

1. `interop` runs only on the nightly schedule, yet it is "blocking on pin bumps" and a pin-bump merge requirement.
2. No `core-oracle` job exists, although it is scheduled nightly and on pin bumps.
3. `probe.yml` is referenced but not defined.
4. The `main` e2e lane is "weekly" in §11.4 but runs nightly in `e2e.yml`, and `upstream.toml [localnet]` has no `main`
   entry for `xtask localnet-image --lane main`.
5. `core-canary` "opens an issue on a new break", but it has `continue-on-error: true`, no `issues: write` and no issue
   step, and it lacks §12.2's `:main` localnet smoke.
6. Node-dependent docify examples "run in the e2e lane" (§13), but `e2e.yml` runs only `-p rtao-testkit`.
7. The weekly mainnet-clone lane (PLAN §12.11, R17) has no job.
8. `oracle-refresh` "manual" has no trigger.
9. The drift matrix includes `localnet-main`, while §2.1 says `vendor/metadata/<net>/` holds "live networks only, never
   localnet".

Evidence: `grep core-oracle` in Appendix E: none; `probe.yml` appears only in a comment (4291); §1.2 `[localnet]` lists
pinned, mainnet, testnet and devnet; upstream clone tooling exists (`UP/clones/scripts/clone-mainnet.sh`,
`start-local-clone.sh`, `UP/docs/internals/mainnet-clone.mdx`).

Fix: add to Appendix E:

- (a) `interop.yml`: `pull_request` (types opened, synchronize, labeled; job
  `if: contains(github.event.pull_request.labels.*.name, 'pin-bump')`), nightly `schedule` and `workflow_dispatch`. It
  runs the interop lanes, then `core-oracle` (`cargo nextest run -p rtao-testkit --features core-oracle --profile
  core-oracle`).
- (b) `probe.yml`: `cron: "41 */6 * * *"`, runs `cargo xtask probe --networks finney,testnet`, has `issues: write`, and
  updates a `live-probe` issue on failure.
- (c) `e2e.yml`: daily schedule for pinned, mainnet, testnet and devnet, plus a weekly schedule adding `main`; add
  `[localnet] main` (tag and digest), refreshed by the drift job.
- (d) `core-canary`: `issues: write`, a `:main` localnet smoke step, and an `if: failure()` step
  `gh issue create --label core-canary` (or edit the open one).
- (e) Keep node-dependent examples as `#[docify::export]` items in `crates/rtao-testkit/tests/`, so the e2e run executes
  them.
- (f) `mainnet-clone.yml`: weekly, large runner, allowed failure. Build state with `clones/scripts/clone-mainnet.sh` at
  `parity.commit` using the node from `:v<parity spec>`, start `start-local-clone.sh`, then run the read-only suite
  (`--features mainnet-clone`).
- (g) `oracle.yml` with `workflow_dispatch: { inputs: { tag } }`.
- (h) §2.1: `vendor/metadata/localnet-main/` holds the `:main` drift baselines.

## TU-15 (major, PLAN.md) The M2 "no C toolchain" exit criterion cannot pass, and the `no-host` job never tests it

Location: §1.5 #5 ("need no libsodium, OpenSSL, Perl or C compiler"); §7.5; §12.4 M2 exit "the no-host validator image
builds without a C toolchain" (same in architecture §14 M2).

Problem: the neuron graph still compiles C:

- `validator`/`miner` enable `btauth-client`: reqwest 0.13.5 with `rustls`, whose crypto provider is aws-lc-rs, built
  from the C sources of `aws-lc-sys`.
- The wss:// session uses tokio-tungstenite with rustls, which needs a provider: ring or aws-lc-rs, both compiled with
  `cc`.

The `no-host` CI job only checks that certain crates are absent, so it never tests the claim. Other reviews raise the same
defect; this finding adds the CI test.

Evidence: arch §2.7 (`tokio-tungstenite 0.30 (rustls-tls-native-roots)`, `reqwest 0.13.5 (rustls)`); §2.4;
`REG/reqwest-0.13.5/Cargo.toml:169-172`; `REG/aws-lc-sys-0.45.0/` (C sources, `CMakeLists.txt`);
`REG/ring-0.17.14/build.rs:507-508` (`cc::Build`); `REG/tokio-tungstenite-0.28.0/Cargo.toml:157-160`; Appendix E `no-host`.

Fix:

- Restate the claim: "neuron images need no libsodium, OpenSSL, Perl or CMake; they need a C compiler (gcc or clang) for
  rustls' `ring` provider."
- Select ring explicitly: reqwest `rustls-no-provider`, rustls `ring`, and install
  `rustls::crypto::ring::default_provider()` in `NeuronRuntime`. Ban `aws-lc-sys` in the no-host profile of `deps-check`.
- New M2 exit and `no-host` job step: "build the no-host validator in `debian:bookworm-slim` plus rustup with only `gcc`
  installed, then connect it to a TLS `FakeNode` over `wss://`."

## TU-16 (major, architecture.md) Scenario 16 cannot be built inside the rtao workspace

Location: §11.4 scenario 16 ("a build of a timelock commit with crates.io `w3f-bls` (std), decrypted by pallet-drand");
§1.3 item 4; PLAN §9.4, §12.4, Q8.

Problem: three constraints combine. `lock.rs` and `deps-check` rule 6 require exactly one `w3f-bls`, the `core_tag` git
copy. Without the `[patch]` the core fails with E0277. The core's manifest pins `w3f-bls` to the workspace copy. A build
through the core with crates.io `w3f-bls` therefore needs a separate project and a core carrying PR-U4's manifest change,
and the plan specifies neither. Other reviews raise the same defect.

Evidence: PLAN §4.2 rule 6; arch §1.3 items 1 and 4; `UP/sdk/bittensor-core/Cargo.toml:61-62`; `docs/plan/bench/nopatch/`.

Fix: add to scenario 16:

- "It lives in `ci/pr-u4-proof/`: its own `[workspace]`, listed in the root `exclude`. `cargo xtask pr-u4-proof prepare`
  makes a `git worktree` of the monorepo at `core.commit` and rewrites `sdk/bittensor-core/Cargo.toml:62` to
  `w3f-bls = { version = "=0.1.3", features = ["std"] }`. The crate depends on that core by `path`.
- (a) Offline: encrypt to a fixed past quicknet round with this build and with rtao's patched build, then cross-decrypt
  with `decrypt_with_signature` and the recorded round signature (`vendor/recorded/drand/quicknet-<round>.json`).
- (b) Localnet: the proof binary writes a `commit_timelocked_weights` ciphertext; rtao submits it; the test asserts
  `TimelockedWeightsRevealed`.
- e2e.yml builds the proof crate in its own step."

## TU-17 (minor, architecture.md) Catalog source line numbers turn every runtime bump into a non-zero diff that needs a human bless

Location: §4.9 catalog layer and auto-bless ("only if … its symbol diff is empty, its extracted tables are unchanged
…"); §13 `tx/<op>.md` "… the chain `sources`"; §12.9 / PLAN §10.6 "runtime bump, surface unchanged … pin-bump PR with zero
generated diff".

Problem: the catalogs carry `sources[].line`, `end_line` and `url`, which shift with any edit to `runtime/src/lib.rs` or
the pallets, so a semantically unchanged catalog changes its blob, its artifact sha256 and the generated pages on every
bump. Auto-bless is defined only through Python symbol diffs, so each bump needs a human bless.

Evidence: `git diff v477 850f2755 -- website/apps/bittensor-website/public/catalog/reads.json`: 27 changed lines, all
`line`/`end_line`/`url` (`runtime/src/lib.rs` moved 3 lines in the 478 bump); live reads.json sha256 `1b55578a…` matches
HEAD, not v477 (`b8965358…`).

Fix: add to §4.9: "Catalog comparison, bless and auto-bless use the normalized catalog (`sources[].{line,end_line,url}`
removed). A catalog row whose normalized content is unchanged auto-blesses." Add to §13: "Generated pages link sources by
`path` and `container::name`, without line numbers."

## TU-18 (minor, PLAN.md) `e2e_map.toml` covers only the 112 manifest names

Location: §8.1 "Every upstream e2e name is mapped in `parity/e2e_map.toml`"; §9.4 "All 112 upstream e2e names are mapped";
arch §4.9 (draft-notes inventory #13, inventory §8 #10).

Problem: upstream's three Python localnet tests are outside the manifest, so they and any future `PYT/e2e` test stay
unmapped, although `sdk/python/tests/e2e/**` is a watched root.

Evidence: `UP/sdk/python/tests/e2e/test_null_consensus.py:22`, `test_pow_registration.py:20`,
`test_timelock_commitment.py:28`; `e2e-manifest.json` (112 names, none of these).

Fix: "Keys = manifest names plus `pyt:e2e/<file>::<test fn>`, discovered with `ruff_python_parser` from the vendored
`PYT/e2e/*.py` (115 today):

- `test_null_yuma_null_emissions_via_python_sdk` → scenario 4;
- `test_pow_register_unfunded_coldkey_and_owner_toggle` → scenario 14;
- `test_timelocked_commitment_reveals_via_python_sdk` → the M7 timelocked-commitment test.

`parity check` fails on an unmapped key." Make the same edit in arch §4.9 and §11.2.

## TU-19 (minor, PLAN.md) Decide §14.3 #4: plan-test all 83 ops on localnet

Location: §14.3 #4 "8 ops have no localnet test anywhere | … | oracle vectors only"; §12.7 "the 72 `intent_*` plan tests
as one table-driven test".

Problem: deferring is unnecessary. Upstream's `intent_*` tests only `plan()` a sample on localnet, and the sample table
already covers every op, so the table test can iterate all 83 ops.

Evidence: `UP/sdk/python/tests/harness/samples.py:35` (`INTENT_SAMPLES`, 83 keys, including `move_swap_stake` :57,
`set_root_claim_threshold` :138, `set_subnet_emission_enabled` :139, `set_reject_locked_alpha` :143, `stake_into_basket`
:144, `swap_basket` :145, `swap_basket_many` :146, `set_min_collateral` :154); `PYT/unit/test_intents_table.py:62-66`;
`UP/sdk/bittensor-core/tests/e2e.rs:81-96`.

Fix: "Resolved: the M5 localnet table test iterates all 83 catalog ops from the extracted `INTENT_SAMPLES`, with netuid
and hotkey fields rewritten to the scenario subnet and slot accounts. It asserts that `plan()` succeeds, `call_data` is
non-empty and the composed tree equals the oracle's." Change §12.7 to "all 83 ops, covering the 72 `intent_*` names".

## TU-20 (minor, PLAN.md) Decide §14.3 #3: a reviewed hits superset

Location: §14.3 #3 "open: a reviewed per-read superset in `parity/`, or derive `ReadSpec.hits` from the recording".

Problem: deriving hits from the recording makes the `RecordingView` test tautological, so the reviewed superset is the only
meaningful option, and it can be decided now.

Evidence: parity-inventory §5.15 (12 reads, 7 with non-empty catalog hits); `UP/website/apps/bittensor-website/scripts/
generate.py:405-420` (hits are scraped from the read's own `st.`/`api.`/`constants.` references).

Fix: "Resolved: `parity/read_hits_extra.toml` lists, for the 12 reads of inventory §5.15, the extra descriptors with Python
citations, e.g. `coldkey_lock = [{ api = 'StakeInfoRuntimeApi.get_coldkey_lock', py = 'PY/reads/locks.py:33-40' }, …]`.
`RecordingView` asserts touched = catalog hits ∪ extra, exactly, for all 102 reads. `parity check` fails when an extra
entry appears in the catalog (shrink-only) or its citation stops naming the item. File an upstream issue for a helper-aware
scan."

## TU-21 (minor, architecture.md) The extractor's literal subset rejects negative numbers, so the RPC policy codes fail

Location: §4.9 layer 4 "The extractor accepts only literals: `str`, `int`, `float`, … Anything else fails loudly"; §11.3
"The extracted `DEFAULT_STORAGE`, … load unchanged".

Problem: in the Python AST, `-32004` is a unary minus applied to a literal, not a literal, so `_POLICY_RPC_ERROR_CODES` and
`_POLICY_RPC_ERROR_MESSAGES` fail extraction. The same happens to `10**9` and `os.getenv(...) or ...` in `PY/settings.py`
and in the FakeSubstrate defaults (another review also raises these two).

Evidence: `PY/_transport/rpc.py:52` (`frozenset({-32004, -32005})`), `:57`; `PY/settings.py:15, 38`;
`PYT/harness/fake_substrate.py:48-56, 70-74`.

Fix: append to layer 4: "The subset also includes unary `-`/`+` on numeric literals; `+ - * // % ** << >> | &` on int
literals and `+ - * /` on float literals, constant-folded with overflow checks (|result| < 2^256); and
`os.getenv('<VAR>') or <literal>` / `os.environ.get('<VAR>', <literal>)`, extracted as
`{ "env": "<VAR>", "default": <literal> }`."

## TU-22 (minor, architecture.md) A blocking pre-1.0 semver gate fails normal development PRs

Location: §12.2 `pr-gates` "`cargo semver-checks` against the last `rtao-v*` tag" (blocking); Appendix E `semver` (4143-4151);
§12.8 "release-plz in changelog-only mode".

Problem: every milestone reshapes APIs, so the gate fails any in-cycle breaking change unless the PR also bumps the
workspace version, which release-plz is meant to own. Before `rtao-v0.0.0` exists, `git describe` fails outright.

Evidence: PLAN §12 (M1-M8 add or reshape crate APIs); Appendix E `semver` step.

Fix: "Before 1.0, `semver` is informational (`continue-on-error: true`) and skipped when no `rtao-v*` tag exists. It
becomes blocking from `rtao-v1.0.0`. release-plz runs cargo-semver-checks on release PRs and bumps the minor version
(pre-1.0) or the major version on reported breaking changes."

## TU-23 (minor, architecture.md) The depth-2 feature powerset is 379 checks per PR

Location: §12.2 `cargo hack --feature-powerset --depth 2 check -p rtao`; Appendix E `features`.

Problem: 27 non-default facade features give 1 + 27 + C(27, 2) = 379 `cargo check` runs, many of which toggle
`bittensor-core/host` or `subxt`. That is hours of runner time for a blocking gate.

Evidence: §2.4 feature list.

Fix: on PRs, run `cargo hack check --locked -p rtao --each-feature --no-dev-deps` plus the role profiles (`validator,prometheus`,
`miner,redis`, `owner,ledger`, `full`, `--no-default-features`). Nightly, run
`cargo hack check --locked -p rtao --feature-powerset --depth 2 --exclude-features full`.

## TU-24 (minor, architecture.md) The macOS Keychain interop lane cannot run, and it does not need Python

Location: §11.6 lane 3 "Keychain entries work both ways on the macOS runner".

Problem: the interop lane runs Python in the oracle image through docker compose, and Docker exists only on the Ubuntu
runners (§11.1 keeps Python inside containers). Python's Keychain contract is a `security` command line, so a Python-free
test covers it.

Evidence: Appendix E preamble; `nightly.yml` `interop` on ubuntu-24.04; `PY/macos_password.py:8`
(`KEYCHAIN_SERVICE = "bittensor.coldkey"`), `:58-80` (`find-generic-password -s … -a <wallet> -w`), `:85-120`
(`delete-generic-password`, then `add-generic-password -s … -a <wallet> -U -w` with the password twice on stdin).

Fix: "Keychain compatibility is a Python-free test in the macos-14 `check` job. An entry written with Python's argv is
unlocked by rtao; an entry saved by rtao is read back with `security find-generic-password -s bittensor.coldkey -a <wallet>
-w`. The extraction layer pins the service name and argv from `PY/macos_password.py`."

## TU-25 (minor, architecture.md) The interop lanes need Python packages and programs the oracle image does not have

Location: §11.6 lanes 1-3 ("FastAPI recipe miner built verbatim", "a Python httpx client", "A v11 Python miner", "Legacy
Fernet and Ansible files decrypt"); §11.2 "cross vectors produced once by btcli … legacy Fernet, Ansible".

Problem:

- `uv sync` of `sdk/python` installs no fastapi, uvicorn, httpx, cryptography or ansible-vault.
- v11 ships no miner or validator runtime.
- btcli writes only `$NACL` keyfiles, so it cannot produce Fernet or Ansible vectors.

Evidence: `UP/sdk/python/pyproject.toml:13-31` and its `dev` group; `uv.lock` has none of these packages; no Axon, Miner
or Validator class in `PY/`; `CORE/keyfiles/mod.rs:86-91`; `PYT/unit/test_keyfiles_golden.py:66-77` (Fernet via
`cryptography`, skipped when absent); `signed-requests.mdx:111-141` (FastAPI) and `:169-197` (httpx).

Fix: add to §11.6:

- "`ci/interop/pyproject.toml` plus a committed `uv.lock` pin fastapi, uvicorn, httpx, cryptography and ansible-vault,
  installed as a layer on the oracle image.
- `ci/interop/py/{miner,client}.py` are generated from the FastAPI and httpx code blocks of `signed-requests.mdx` at
  `parity_tag`; `validator.py` is rtao-authored.
- Fernet vectors use `cryptography.fernet` with the legacy salt and iterations of `test_keyfiles_golden.py:72-77`;
  Ansible vectors use `ansible-vault`. Only `$NACL` and plaintext files come from btcli."

Fix §11.2 to match.

## TU-26 (minor, PLAN.md) Q10 is answered by the source

Location: §14.2 Q10 ("localnet experiment | M4"); arch §11.4 scenario 10 "if a setter exists".

Problem: the setter exists and the queue logic is readable in the source, so scenario 10 can be fixed now rather than
after an experiment.

Evidence: 477 metadata `AdminUtils` 37 `sudo_set_subnet_limit`, 35 `sudo_set_network_immunity_period`;
`UP/pallets/admin-utils/src/lib.rs:1253-1258`; `subnets/subnet.rs:194-262` (prune and `NetworkRegistrationQueued`);
`coinbase/root.rs:344-376`; `UP/runtime/src/lib.rs:962`.

Fix: "Resolved: scenario 10's queued path runs in the `global` container. Root sets `sudo_set_network_immunity_period(0)`
and then `sudo_set_subnet_limit(<current non-root count>)`. The next `register_network` prunes the lowest-price subnet and
emits `NetworkRegistrationQueued`, and the follower waits for that registration's `NetworkAdded`."

## TU-27 (minor, PLAN.md) Decide §14.3 #11: `allow-invalid` on host-gated `clippy.toml` entries

Location: §14.3 #11 "recent clippy versions warn about unresolvable configured paths [I] … check a no-host clippy run".

Problem: the open question has a documented answer. Since Rust 1.88 clippy validates `disallowed_*` paths and reports an
error for paths that do not resolve, and `allow-invalid` opts an entry out.

Evidence: clippy CHANGELOG, Rust 1.88: "Paths in `disallowed_*` configurations are now validated (#14397)" and #14733;
clippy `book/src/lint_configuration.md` `disallowed-methods` → `allow-invalid` ("instead of emitting an error");
`CORE/lib.rs:10-31`.

Fix: "Resolved: every `clippy.toml` entry under a host-gated module carries `allow-invalid = true`. The `--all-features`
clippy run enforces those entries, and the `no-host` job adds `cargo clippy --locked -p rtao --no-default-features
--features validator,miner,prometheus -- -D warnings`."

## TU-28 (minor, architecture.md) `wasm-override` boots the production runtime; Q13 is answered

Location: §11.4 lane "`wasm-override` | latest node + a released `subtensor.wasm` … | for specs without images";
§15.2/PLAN §14.2 Q13.

Problem: the released wasm is the production srtool build, without `fast-runtime` or `pow-faucet`. A localnet on it has
12 s slots, tempo 360 and a 7,200-block network rate limit, so the suite cannot run on this lane. Separately, `:v<spec>`
images are now published by design.

Evidence: `UP/.github/workflows/release-train.yml:89-90` (`--features=metadata-hash`, `PROFILE=production`);
`UP/scripts/localnet.sh:40, 47` (localnet features); `UP/node/src/chain_spec/localnet.rs:6-26`;
`UP/runtime/src/lib.rs:904, 967`; `UP/pallets/subtensor/src/lib.rs:1022-1027`; `UP/.github/workflows/
docker-localnet.yml:7-9`; GHCR tags v432, v473, v475, v477.

Fix: "`wasm-override`: a manual fallback, not a CI lane. It runs the 12 s profile with the bootstrap knobs,
`sudo_set_network_rate_limit(0)` and `sudo_set_tempo` per subnet." For Q13: "Resolved: upstream publishes `:v<spec>` on
every mainnet mirror move; `pin --check` waits for it (TU-03)."

## TU-29 (minor, architecture.md) `release.yml` cannot build the advertised targets and never deploys docs

Location: Appendix E `release.yml` (one `ubuntu-24.04` job; `cargo dist build … # linux x86_64/aarch64, macOS
arm64/x86_64`; `pages: write`); §13 GitHub Pages.

Problem: one Ubuntu job cannot produce macOS binaries of a CLI that builds C code (vendored OpenSSL, libsodium), and the
job deploys neither rustdoc nor the book. Another review raises the same defect.

Evidence: Appendix E 4295-4310; `UP/sdk/bittensor-core/Cargo.toml:24-44`; §12.8.

Fix: generate the workflow with `dist init` for `x86_64/aarch64-unknown-linux-gnu` and `aarch64/x86_64-apple-darwin` on
`ubuntu-24.04`, `ubuntu-24.04-arm`, `macos-14` and `macos-13`. Add a pages job: `cargo doc --workspace --no-deps
--all-features`, `mdbook build book`, `actions/upload-pages-artifact`, `actions/deploy-pages`.

## TU-30 (minor, architecture.md) golden.json `constants` are never replayed

Location: §11.2 golden row (lists every section except `constants`); PLAN §12.2 replay list.

Problem: `ViewExt::constant` and the generated `constants::*` descriptors get no golden check.

Evidence: `jq 'keys'` on `PYT/fixtures/golden.json` includes `constants`: `Balances.ExistentialDeposit` 500,
`Aura.SlotDuration` 250, `System.SS58Prefix` 42 (`record_golden.py` `CONSTANT_CASES`).

Fix: append to the §11.2 golden row: "`constants` (3), decoded through `ViewExt::constant` and the generated `constants::*`
at the golden metadata, must equal `decoded`." Add it to M1's replay tasks.

## TU-31 (minor, PLAN.md) The M1 drift exit criterion depends on upstream timing

Location: §12.3 M1 exit "the drift job opens a correctly classified PR on a real change (devnet 478 as
`surface-unchanged`)".

Problem: devnet already runs 478, so a drift job bootstrapped in M1 records 478 as devnet's first row and never sees the
477→478 change. Whether any real change arrives during M1 is up to upstream.

Evidence: CORRECTIONS #2; `scratchpad/meta/finney.scale` and `devnet.scale` differ in one byte (`cmp`); §12.2 compares
each network against its own `index.toml`.

Fix: "M1 exit: unit tests of `xtask drift diff` on committed fixtures:

- finney 477 vs devnet 478 → `surface-unchanged`;
- 477 plus an added call → `additive`;
- 477 with a changed storage default → `behavioural`;
- 477 with a moved call index, or a new extension with payload → `breaking`.

Plus one live `metadata-drift` run that exits 0 on an up-to-date index."

## TU-32 (minor, architecture.md) `--all-features` turns the Docker tests on inside the offline PR gate

Location: Appendix E `pr.yml` `cargo nextest run --workspace --all-features --locked --profile ci`.

Problem: `--all-features` enables rtao-testkit's `docker`, `interop` and `core-oracle` features, which gate the localnet
tests. The undefined `ci` profile would therefore run them in the PR gate, including on macOS, which has no Docker.

Evidence: PLAN §4.3 `rtao-testkit` features; Appendix E e2e/interop invocations; Appendix E preamble;
`UP/sdk/bittensor-core/Cargo.toml:13-18` (upstream keeps its suite out with `[[test]] e2e … test = false`).

Fix: "Localnet, interop and core-oracle tests live in `[[test]]` targets `e2e`, `interop` and `core_oracle` with
`test = false` and `required-features`. `.config/nextest.toml` sets
`[profile.ci] default-filter = 'not (binary(e2e) | binary(interop) | binary(core_oracle))'`, and the lane workflows select
the targets with `--test`."

## TU-33 (minor, architecture.md) "FakeChain is seeded identically" needs the oracle to dump its state

Location: §11.5 Rust side "`FakeChain` is seeded identically"; §11.3 seeding from the extracted `DEFAULT_*`.

Problem: Python's per-op seeds are test code, and the fee every plan reports comes from FakeSubstrate's constructor. If
the dumps do not record their state, rtao must re-create the seeds by hand and keep them in sync, and the oracle's `fee`
cannot be matched.

Evidence: `PYT/unit/test_intents_table.py:27-31` (fixture `seed_runtime(StakeInfoRuntimeApi …)`), `:99-105`
(`claim_root*` funding), `:44-60`; `PYT/harness/fake_substrate.py:121, 147` (`fee = Balance.from_rao(124_414)`).

Fix: "Each dump records per sample the FakeSubstrate state it ran with (storage seeds including `seed_default`,
constants, runtime results, `fee`) in `vendor/oracle/seeds/<op|read>.json`. `FakeState::from_python_seed` loads it through
the core's lenient encoder, and FakeChain answers `TransactionPaymentApi_query_info` with the dumped `fee`."
