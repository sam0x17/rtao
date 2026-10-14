# Skeptic verification: testing-upstream findings (TU-01 … TU-33)

Method: every finding was checked against the plan text (PLAN.md, architecture.md, parity-inventory.md) and the
cited evidence on disk (UP/ at 850f2755, CORE/, PY/, PYT/, CAT/, scratchpad/meta), plus read-only live checks
(GHCR registry API, GitHub releases/actions API, PyPI JSON, GitHub/nextest/cargo-hack docs). Two small probes ran
outside upstream dirs: a clippy probe (`scratchpad/verify-tu/clippy-probe2`) and a blob comparison of the
`bittensor==11.3.0` wheel against tag trees (`scratchpad/verify-tu/`).

Tally: 22 confirmed, 11 partially confirmed, 0 refuted.

| ID | Verdict | One-line basis |
|---|---|---|
| TU-01 | confirmed | batch_all.calls = Vec<141>, 141 = outer call enum; exact structural hash cascades; 12 intents wrap the 5 nested-call calls |
| TU-02 | confirmed | GitHub docs: GITHUB_TOKEN events do not create workflow runs (except workflow_dispatch/repository_dispatch) |
| TU-03 | partially_confirmed | race measured (v477 promoted 14:33Z, :v477 built 15:05Z); fix's bare anonymous GHCR GET returns 401 |
| TU-04 | confirmed | record_golden.py:85 `Alpha` absent at 477 (only AlphaV2); no try/except; localhost guard; corpus recorder offline |
| TU-05 | confirmed | workflow-level `paths` leaves required checks Pending; path list misses testkit/core-ext/etc.; testnet row self-contradicts |
| TU-06 | confirmed | freeze window 10 vs fast tempo 10 → open one block per epoch; serving/version-key/tempo setters check it |
| TU-07 | partially_confirmed | nextest is process-per-test; but fix still needs Alice for root-only per-subnet knobs → needs a cross-process lock |
| TU-08 | confirmed | pallet-drand OCW uses public HTTPS; upstream waits ≤30 s for LastStoredRound; fast reveal epoch 2.5 s < 3 s period |
| TU-09 | confirmed | dtolnay action only `rustup default`; rust-toolchain.toml wins; ruff main rust-version 1.97 |
| TU-10 | confirmed | serving/rate_limiting/misc/weights/admin-utils/lib.rs unwatched; changed in 1-4 of 6 release pairs |
| TU-11 | partially_confirmed | "vendored sources" don't exist in [artifacts]; but impact minor and full vendoring is more than needed |
| TU-12 | partially_confirmed | dev0 string spans 10 tags, 11.3.1 unpublished; but 11.3.0 == v475 file-for-file (claim overstated) |
| TU-13 | confirmed | float(11614906923326217) == float(11614906923326216) (measured) |
| TU-14 | confirmed | all nine Appendix E gaps verified (interop trigger, core-oracle, probe.yml, main lane, canary issue, docify, clone, oracle-refresh, localnet-main) |
| TU-15 | confirmed | reqwest 0.13.5 `rustls` → aws-lc-rs; ring uses cc; also linux-gnu linking and blake3 need cc |
| TU-16 | confirmed | rtao's root [patch] + rule 6 make a crates.io w3f-bls build impossible inside the workspace; no setup specified |
| TU-17 | partially_confirmed | line-only catalog churn verified; "every bump needs a human bless" not shown (auto-bless conditions vacuous for JSON) |
| TU-18 | partially_confirmed | 3 PYT e2e tests absent from manifest; but files already need map.toml rows (watched root) |
| TU-19 | partially_confirmed | INTENT_SAMPLES covers 83; fix's "call tree equals the oracle's" can't hold after rewriting netuid/hotkey |
| TU-20 | partially_confirmed | gap real; "only meaningful option" overstated (oracle-recorded access log is independent) |
| TU-21 | confirmed | `-32004`, `10**9`, `os.getenv(...) or ...` are not literals in the AST |
| TU-22 | confirmed | `git describe --match 'rtao-v*'` fails before first tag; version bumps only in release PRs |
| TU-23 | partially_confirmed | real cost, but cargo-hack de-dupes: 315 runs (94 with host), not 379 |
| TU-24 | confirmed | interop runs on Ubuntu/Docker; Keychain contract is the `security` CLI, which rtao also shells out to |
| TU-25 | confirmed | fastapi/uvicorn/httpx/cryptography/ansible-vault absent from uv.lock; no v11 miner/validator; btcli writes $NACL only |
| TU-26 | confirmed | calls 35/37 exist; subnet.rs:194-262 + root.rs:344-376 give a deterministic queued-path recipe |
| TU-27 | partially_confirmed | measured: unresolved path is a non-fatal *warning*, not an error; allow-invalid resolution is right |
| TU-28 | confirmed | released wasm is production (metadata-hash only): 12 s, tempo 360, network rate limit 7200 |
| TU-29 | partially_confirmed | single Ubuntu job can't build darwin binaries; no Pages deploy; but macos-13 retired 2025-12-04 → macos-15-intel |
| TU-30 | confirmed | golden.json has `constants` (3); not in the replay list |
| TU-31 | confirmed | devnet already 478 (1-byte metadata diff vs finney 477); exit depends on upstream timing |
| TU-32 | confirmed | `--all-features` enables docker/interop/core-oracle tests; no nextest gating defined; no Docker on macOS |
| TU-33 | confirmed | per-op seeds are imperative Python; fee is FakeSubstrate's 124_414 constructor attribute |

---

## Details and corrected fixes

### TU-01 — confirmed
finney 477 metadata: Utility call type 179; `batch_all.calls` type 180 = `Vec<141>`; 141 = `outer_enums.call_enum_ty`
(`node_subtensor_runtime::RuntimeCall`, 23 variants). `Sudo.sudo`, `Multisig.as_multi[_threshold_1]`,
`Proxy.proxy_announced` carry `Box<RuntimeCall>`. jq over CAT/intents.json: exactly 12 intents wrap these.
arch §3.7 node(id) is fully structural with only an `("R", depth)` cycle marker; calls use exact hashes; §3.8
`Call::compose` "verifies the descriptor's shape first"; §12.5 breaking row "changed args of a wrapped call".
Added `#[pallet::call_index(` per pair (git diff pallets runtime/src): v467→v468 +1, v469→v470 +1, v473→v475 +4.
CORE/codec/encode.rs:528-532 splices nested calls as `Value::Bytes`. Fix is sound. (Effect is fail-closed
unavailability rather than mis-signing; "major" would also be defensible.)

### TU-02 — confirmed
docs.github.com/en/actions/concepts/security/github_token: "events triggered by the GITHUB_TOKEN will not create a new
workflow run", exceptions workflow_dispatch / repository_dispatch. drift.yml 4201-4203 and pin-bump.yml 4237-4243 use
`secrets.GITHUB_TOKEN`; pin-bump never creates/pushes a head branch before `gh pr create`.

### TU-03 — partially_confirmed
Verified race: watch-mainnet-release.yml mirror push and promotion are adjacent steps of one job; docker-localnet.yml
builds the node from source on the mainnet push (artifacts timeout 90 min, smoke 20 min). Live: v477 release updated
2026-10-12T14:33:19Z; docker-localnet run on `mainnet` for 5c6e83e2 ran 14:33:17Z→15:05:33Z. Drift runs at :17 every
6 h and lands metadata through its own PR. Fix defect: `GET https://ghcr.io/v2/.../manifests/v477` without a token
returns 401 (measured); a pull token is required. Also the pin PR and the drift PR would both write
`vendor/metadata/finney/S.*` and index rows.

Corrected fix: §12.3 step 1, append: "… and the `:vS` localnet image is published. `xtask pin --check` requests an
anonymous pull token (`GET https://ghcr.io/token?scope=repository:raofoundation/subtensor-localnet:pull`), then
`GET https://ghcr.io/v2/raofoundation/subtensor-localnet/manifests/vS` with that bearer token and
`Accept: application/vnd.oci.image.index.v1+json` (an unauthenticated GET returns 401). On 200 it writes the
`Docker-Content-Digest` to `[localnet].pinned`; otherwise `bump=false` and the next 30-minute poll retries (v477: release
promoted 14:33Z, `:v477` built 15:05Z)." Step 3: replace "(fetched by the drift job)" with "(fetched by `xtask pin`
itself at finney's finalized block with `Metadata_metadata_at_version(15)` and `(16)`, and committed in the pin-bump PR
with the `vendor/metadata/finney/index.toml` row; the drift job opens no PR for a `(spec, :code)` already recorded on
`main` or in an open pin-bump PR)". In pin-bump.yml split the step into `oracle` (build + run; failure → `oracle-stale`)
and `record` (`continue-on-error: true`; failure → `recordings-stale`).

### TU-04 — confirmed
record_golden.py:85 `("SubtensorModule","Alpha",…)`; 477 storage has `AlphaV2` only; every other `*_CASES` entry exists
at 477 (checked). `record()` calls `codec.storage_entry` with no try/except (:169-181) → `StorageFunctionNotFound`
(codec.py:257-261). Localhost guard :41-49. record_shape_corpus.py:12 "Runs fully offline", :34/:152 `golden_codec()`
reads tests/fixtures/golden.json. §11.2 itself notes Alpha was removed after 461. Fix sound
(`RECORD_GOLDEN_ALLOW_REMOTE=1` is an equally valid alternative to the shared network namespace).

### TU-05 — confirmed
GitHub "Troubleshooting required status checks": a workflow skipped by path filtering leaves required checks Pending.
Upstream check-bittensor-e2e-tests.yml:17-19 avoids workflow-level paths for this reason. Appendix E paths omit
rtao-testkit, rtao-core-ext, rtao-core, rtao-runtime, rtao-signer, rtao-wallet, rtao-btauth, rtao-catalog,
rtao-primitives, Cargo.lock/Cargo.toml, codegen, ci, the workflow file. §11.4 testnet row says both "required on
`main`" and "instead of blocking"; §12.2 says "blocking on `main`"; post-merge push runs cannot block.

### TU-06 — confirmed
lib.rs:1436-1438 freeze window 10; runtime lib.rs:904 tempo `prod_or_fast!(360, 10)`; misc.rs:59-71 `remaining < window`
with remaining = LastEpochBlock + tempo − block (run_coinbase.rs:1247-1255) → open only in the epoch block. Owner/root
setters call `ensure_admin_window_open`: version key (:444-454), serving rate limit (:343-353, one of the plan's own
knobs), min allowed weights (:592-602), `do_set_tempo` (tempo_control.rs:11-30). Owner hparam rate limit default 2
(lib.rs:1441-1444). Upstream PYT e2e zero these knobs (test_null_consensus.py:39-55, test_pow_registration.py:33-38).
SubnetLimit 128 (lib.rs:1454-1456), immunity 1,296,000 (runtime lib.rs:962), subnet.rs:205-213. All setters exist in 477
metadata (AdminUtils 2, 3, 7, 14, 29, 30, 35-38, 74, 75, 85). Note `sudo_set_weights_set_rate_limit` is root-only
(admin-utils :479-484), so it is sent by Alice ("where owner-settable" covers it).

### TU-07 — partially_confirmed
nextest docs: each test runs in its own process; test groups cap concurrency; setup scripts experimental
(`experimental = ["setup-scripts"]`); `NEXTEST_TEST_GLOBAL_SLOT` since 0.9.90. Upstream starts a container per
`TestContext::new()` (e2e_support/mod.rs:33-50, 62-139). Python `execute(retries=0)` by default (executor.py:785, 832),
so an Alice nonce collision fails outright. Fix gap: root-only per-subnet knobs (`sudo_set_weights_set_rate_limit`,
root `sudo_set_tempo` from TU-06/TU-08) still need Alice per test, so "concurrent processes never share a nonce" does
not follow.

Corrected fix: replace the bullet with: "`e2e.yml` starts the containers itself, one per nextest partition plus one
`global` container, and runs `cargo xtask localnet bootstrap --endpoint <ws>` once per container: the §11.4 global
knobs, then one `Utility.batch_all` from Alice funding 64 slot accounts (`//rtao-e2e//slot/<n>`,
`//rtao-e2e//slot/<n>//hot`). It then runs `cargo nextest run -p rtao-testkit --features docker --profile e2e-<blocks>
--partition hash:<k>/<n>` with `E2E_ENDPOINT` set. A test signs its own transactions only with the accounts of its
`NEXTEST_TEST_GLOBAL_SLOT` (nextest ≥ 0.9.90). Every Alice (Sudo) submission a test still needs, such as the root-only
per-subnet knobs `sudo_set_weights_set_rate_limit` and `sudo_set_tempo`, goes through `Localnet::sudo()`, which holds an
exclusive advisory file lock (`target/e2e/<container>.alice.lock`) from the `account_nextIndex` fetch until inclusion, so
concurrent processes never share Alice's nonce. Tests that change global state (scenarios 6, 8, 10's queued path and
planner, 16) are in test group `global` (`max-threads = 1`) and run in a second invocation against the `global`
container. `rtao localnet up` gives developers the same setup."

### TU-08 — confirmed
drand lib.rs:81-87 (public HTTPS endpoints), :91-92 quicknet, :275 offchain worker. Upstream waits ≤30 s for
`Drand.LastStoredRound > 0` (e2e.rs:548-563; test_timelock_commitment.py:38-43); plan readiness is healthcheck /
`Imported #1` only and never mentions drand egress. reveal_commits.rs: missing pulse → retry each block within the
reveal epoch, purged once the epoch advances. Fast epoch = 10 × 250 ms = 2.5 s < DRAND_PERIOD 3 s; the commit targets
first reveal block + 3 (CORE/timelock/mod.rs:170-183).

### TU-09 — confirmed
dtolnay/rust-toolchain action.yml:111-112 runs `rustup default` only (no `RUSTUP_TOOLCHAIN`); rust-toolchain.toml (§2.1,
current stable) outranks the default. ruff main Cargo.toml `rust-version = "1.97"`; xtask depends on
`ruff_python_parser` (§2.7). Fuzz job likewise resolves to stable. Facade deps fit 1.89 (reqwest 0.13.5 and
tokio-tungstenite 0.30 declare 1.85).

### TU-10 — confirmed
Watched roots (§4.9) omit serving.rs, utils/{misc,rate_limiting}.rs, admin-utils lib.rs, tempo_control.rs,
subnets/{weights,subnet}.rs and lib.rs constants (MIN/MAX_TEMPO etc. at lib.rs:2325-2331; rate_limit_on_subnet arms at
rate_limiting.rs:38-53), which §9.5/§9.7 copy. git diff: rate_limiting.rs v473→v475 and v475→v477 (Hyperparameter 37/38),
misc.rs + weights.rs v473→v475, admin-utils lib.rs 3/6 pairs, lib.rs 4/6 pairs. Code-only arm changes produce no
metadata drift.

### TU-11 — partially_confirmed
Real: [artifacts] (§1.2) and vendor/upstream (§2.1) hold no Python/runtime sources, yet layer 4 "re-extracts … from the
vendored sources"; layer 1 uses a blob:none partial clone cached in `target/upstream.git`, so PR gates fetch blobs from
GitHub on a cache miss, contradicting "blocking, offline". Overstated: practical risk is low (runners always reach
github.com) and on ordinary PRs layer 2 needs no blobs (no row has `synced_blob` ≠ current blob unless parity.commit
moves), so vendoring every watched root is more than needed. Severity: minor.

Corrected fix: add to §1.2 and §4.9: "`xtask import-upstream` also vendors, at `parity.commit`, (a)
`vendor/upstream/tree.txt` = `git ls-tree -r <parity.commit> -- <watched roots>`, and (b) every source file read by the
extraction layer (`map.toml` rows of kind `extracted`, e.g. `PY/settings.py`, `PY/_transport/rpc.py`, `PY/executor.py`,
`PY/error_map.py`, `PY/hyperparams.py`, `PYT/harness/fake_substrate.py`, `PYT/harness/samples.py`) or by the
chain-source layer (`common/src/transaction_error.rs`, `runtime/src/fee_filters.rs`, `pallets/subtensor/src/rpc_info/**`,
…), under `vendor/upstream/src/<monorepo path>`, each with a sha256 row in `[artifacts]` checked by `--verify-only`. On
ordinary PRs layers 1, 4 and 5 read only `vendor/`. Layer 2 parses old and new blobs only for rows whose `synced_blob`
differs from `tree.txt`, which happens only when `parity.commit` moves (pin-bump, upstream-next); those jobs use the
partial clone."

### TU-12 — partially_confirmed
Verified: sdk/python version 11.3.0.dev0 at v464…v476 (distinct `sdk/python/bittensor` trees) and 11.3.1.dev0 at v477;
PyPI has bittensor 11.3.0 (2026-10-07), no 11.3.1 (only 11.3.1rc68), bittensor-core 0.1.6 only as rc68; stable
publication waits for the `mainnet` environment (watch-mainnet-release.yml:19-21, :370-377). Overstated: the 11.3.0
wheel's `bittensor/` equals v475's tree file for file (220/220 blobs, measured) and differs from v473 in 25 files, so a
stable release does identify its release commit; the hazard is the naive `.dev0` mapping (v473 → v475's Python) and
the absence of wheels for v477.

Corrected fix: replace the §11.5 sentence with "There is no PyPI fallback: if the image cannot be built, the PR is
labelled `oracle-stale` and the committed vectors from the previous tag stay authoritative (§12.3)." Delete "PyPI wheel
fallback" from R5 (arch §15.1, PLAN §13), PD-13 (Appendix A) and PLAN §9.5. Label `python_package_version` in §1.2 as
"source version from pyproject.toml at parity.tag (not a PyPI release)", and make §12.7's column "parity tag (source
version; PyPI release if one was published from that commit)".

### TU-13 — confirmed
`11614906923326217 > 2^53`; `float(11614906923326217) == float(11614906923326216)` (measured; ulp 2). `_jsonable`
(cli/query.py:104-116) stringifies only `Balance`; raw ints in call params, plan args and runtime-API dicts stay ints.
The fix is implementable (`serde_json::Number::as_str` under `arbitrary_precision`).

### TU-14 — confirmed
All nine sub-claims verified in Appendix E (interop only nightly; no core-oracle job; probe.yml only in a comment;
`main` lane nightly vs "weekly" and no `[localnet] main`; core-canary lacks issue permission/step and the `:main`
smoke; e2e runs only `-p rtao-testkit` vs §13 docify node examples; no mainnet-clone job though UP/clones/scripts exist;
oracle-refresh "manual" without a trigger; drift `localnet-main` vs §2.1 "never localnet"). Interop's pull_request
trigger only fires on bot PRs once TU-02's App token is adopted.

### TU-15 — confirmed
reqwest 0.13.5 `rustls = ["__rustls-aws-lc-rs", …]` (aws-lc-sys ships C/CMake sources), `rustls-no-provider` exists;
tokio-tungstenite 0.30's rustls dep is `default-features = false`; ring 0.17.14 build.rs uses `cc::Build`. Additional
reasons the claim fails: rustc links through `cc` on linux-gnu, and the no-host core's own lockfile has `blake3 → cc`
(SP/design-tagpin/Cargo.lock). The fix's restatement is correct.

### TU-16 — confirmed
In rtao's workspace the root `[patch.crates-io] w3f-bls` redirects every crates.io request to the git copy and lock.rs
/ deps-check rule 6 require exactly one w3f-bls, so no member (including rtao-testkit) can build with crates.io w3f-bls.
The core's manifest inherits w3f-bls and tle from the monorepo workspace (Cargo.toml:61-62; UP/Cargo.toml:299, 310,
334-335). Nothing in the plan says where scenario 16's build lives. The fix works: as its own workspace root the proof
crate ignores the monorepo's `[patch]`.

### TU-17 — partially_confirmed
Real: `git diff v477 850f2755 -- …/reads.json` is 27 lines, all sources line/end_line/url (9 each); live
bittensor.com reads.json sha256 1b55578a = HEAD, v477 = b8965358. §13 pages include "the chain sources", so they churn
and the playbook's "pin-bump PR with zero generated diff" fails. Overstated: auto-bless (§4.9) requires an empty symbol
diff, unchanged extracted tables and unchanged mapped oracle vectors, which a line-shift-only JSON change plausibly
meets; "every such bump needs a human bless" is not shown.

Corrected fix: add to §4.9 (Bless and auto-bless): "For the three catalogs, the file layer, the catalog layer and
auto-bless compare the normalized catalog (each `sources[]` entry without `line`, `end_line` and `url`). A catalog blob
whose normalized form is unchanged is auto-blessed, and the pin bump refreshes its `[artifacts]` sha256." Add to §13
(Generated reference): "Pages cite a source as path plus `container::name` and link `raw_url` without a line anchor, so
line shifts in runtime or pallet files change no generated page." With both, the §12.9 row "pin-bump PR with zero
generated diff" holds.

### TU-18 — partially_confirmed
Verified: test_null_consensus.py:22, test_pow_registration.py:20 and test_timelock_commitment.py:28 are not among the
112 manifest names. Overstated: they do not "stay unmapped" at file level. PYT/e2e/** is a watched root, so each file
needs a map.toml row, and new files or changed blobs fail the file layer. The gap is per-test mapping.

Corrected fix: replace in PLAN §8.1/§9.4 and architecture §4.9/§11.2: "`e2e_map.toml` keys are the 112 manifest names
plus one key per test function in `PYT/e2e/*.py`, written `pyt:e2e/<file>::<function>` and discovered with
`ruff_python_parser` at `parity.commit` (115 keys at v477):
- `pyt:e2e/test_null_consensus.py::test_null_yuma_null_emissions_via_python_sdk` → scenario 4;
- `pyt:e2e/test_pow_registration.py::test_pow_register_unfunded_coldkey_and_owner_toggle` → scenario 14;
- `pyt:e2e/test_timelock_commitment.py::test_timelocked_commitment_reveals_via_python_sdk` → the rtao test that
  `test_timelocked_commitment_reveals_plaintext` maps to.

`parity check` fails on any unmapped key."

### TU-19 — partially_confirmed
Verified: INTENT_SAMPLES has 83 keys (samples.py:35; the 8 ops at :57, :138, :139, :143-146, :154). Upstream's test only
plans (e2e.rs:81-96). Python `plan()` returns preflight blocks as violations instead of raising (executor.py:709-770).
None of the 8 samples uses `all`, and StakeIntoBasket/SwapBasket `build()` only compose (staking.py:1684-1696,
1808-1825). Fix flaw: after netuid/hotkey fields are rewritten, the localnet call tree cannot equal the oracle's
(the oracle dumps used the original sample values).

Corrected fix: replace PLAN §14.3 #4's default with "Resolved: the M5 localnet table test iterates all 83 catalog ops
from the extracted `INTENT_SAMPLES` (`PYT/harness/samples.py:35`), rewriting `netuid` fields to the scenario's subnet and
`*_hotkey_ss58` fields to the test's slot hotkey. For each op it asserts that `plan()` returns `Ok`, that
`plan.op == op`, that `call_data` is non-empty, and that the `(pallet, call)` decoded from the first two bytes of
`call_data` at the live metadata is one of the op's catalog `wraps`. An op whose `plan()` needs chain state gets a seed
step in the table, never an exemption. Call-tree equality with Python stays in the offline oracle tests, which use the
unrewritten samples." In PLAN §12.7, write "the 83-op table-driven plan test over `AnyIntent` (it covers the 72
`intent_*` names)".

### TU-20 — partially_confirmed
The gap is real: inventory §5.15 lists 12 reads, 7 of them with non-empty hits, and generate.py:405-420 scrapes only a
read's own `st.`/`api.`/`constants.` references. Deriving hits from rtao's own RecordingView would indeed make the test
vacuous. But "only meaningful option" is overstated: the Python oracle can record what FakeSubstrate served for each
sample, which is an independent source and also keeps a reviewed list honest.

Corrected fix: replace PLAN §14.3 #3's default with "Resolved:
- The oracle's `dump_reads.py` wraps FakeSubstrate and records, for each `READ_SAMPLE`, every storage item, runtime-API
  method and constant served (`touched` in `vendor/oracle/reads.json`).
- `parity/read_hits_extra.toml` lists, for each of the 12 reads in inventory §5.15, the reviewed extra descriptors with
  Python citations, e.g. `coldkey_lock = [{ api = "StakeInfoRuntimeApi.get_coldkey_lock", py = "PY/reads/locks.py:33-40" },
  { storage = "SubtensorModule.DecayingLock", py = "PY/reads/locks.py:33-40" }]`.
- For all 102 reads, the RecordingView test asserts that rtao's touched set equals catalog hits ∪ extra.
- `xtask oracle` fails if an oracle `touched` item falls outside catalog hits ∪ extra.
- `parity check` fails if an extra entry has appeared in the catalog hits (the list is shrink-only) or if its citation no
  longer names the item.
- An upstream issue asks generate.py to scan helpers."

### TU-21 — confirmed
The flagged expressions are not literals in the AST:
- rpc.py:52 `frozenset({-32004, -32005})` and rpc.py:57 `-32004: …` are UnaryOp;
- settings.py:15 `10**9` is BinOp;
- settings.py:38 `os.getenv(…) or …` is BoolOp;
- fake_substrate.py:48-56 and 70-74 use `10**9` and `10**7`.

§4.9 layer 4 accepts only literals and fails loudly on anything else, yet §4.1 promises "No Python constant is
hand-copied" and §11.3 says the `DEFAULT_*` tables "load unchanged".

### TU-22 — confirmed
`git describe --tags --abbrev=0 --match 'rtao-v*'` fails before the first `rtao-v*` tag, and M0 creates `rtao-v0.0.0`
only at its end (PLAN §12.14). Versions are bumped only in release-plz release PRs (§12.8), so a blocking
cargo-semver-checks gate fails any PR that makes a breaking change between releases.

### TU-23 — partially_confirmed
The 27 non-default facade features are verified. The raw depth-2 count is 379, but cargo-hack de-duplicates fully
equivalent combinations ("cargo-hack deduplicate any fully equivalent feature combinations"). The reviewer's own script
(review/work/hack_count.py) gives 315 runs, 94 of them with `host`. That is still hundreds of checks on a blocking gate.

Corrected fix:
- PR gate (features job): `cargo hack check --locked -p rtao --each-feature --no-dev-deps`, plus the role profiles:
  - `--no-default-features --features validator,prometheus`
  - `--features miner,redis`
  - `--features owner,ledger`
  - `--features full`
  - `--no-default-features`
- Nightly: `cargo hack check --locked -p rtao --feature-powerset --depth 2 --exclude-features full`. This is 315 runs
  after de-duplication, 94 of which enable bittensor-core/host.

### TU-24 — confirmed
Docker exists only on the Ubuntu runners (Appendix E preamble), and interop runs on ubuntu-24.04. The Keychain contract
is defined by PY/macos_password.py:8, :58-80 and :85-120, and arch §6.2 has rtao shell out to the same `security` CLI.
So the Python-free test on the macos-14 check job is exact.

### TU-25 — confirmed
- fastapi, uvicorn, httpx, cryptography and ansible-vault have zero entries in sdk/python/uv.lock.
- PY has no Axon, Miner or Validator class.
- CORE/keyfiles/mod.rs:86-95 writes `$NACL` only.
- test_keyfiles_golden.py:66-77 builds the Fernet vector with `cryptography.fernet` and skips when it is absent.
- The code blocks are at signed-requests.mdx:111-141 (FastAPI) and :169-197 (httpx); the plan cites :104-157.

### TU-26 — confirmed
In 477 metadata, AdminUtils calls 35 (`sudo_set_network_immunity_period`) and 37 (`sudo_set_subnet_limit`) are both
root-only (admin-utils:1217-1258). subnet.rs:194-262 prunes `get_network_to_prune()`, and root.rs:344-376 skips only
immune subnets; the code then emits `NetworkRegistrationQueued`, or `SubnetLimitReached` when nothing is prunable. The
recipe works in the global container.

### TU-27 — partially_confirmed
Measured with clippy 0.1.99 (`verify-tu/clippy-probe2`):
- An unresolvable host-gated path yields a non-lint warning ("does not refer to a reachable function"; help "add
  `allow-invalid = true`").
- The warning appears only when the crate is loaded, and `-- -D warnings` still exits 0.
- With `allow-invalid = true` the entry still fires when the path exists (`verify-cb/clippy-probe/out.txt`).
- The CHANGELOG lists both #14397 and #14733 under Rust 1.88.

So "reports an error" is wrong; PLAN §14.3 #11's "warn" was accurate. The proposed resolution is still right.

Corrected fix: replace PLAN §14.3 #11's default with "Resolved (measured with clippy 0.1.99):
- Since Rust 1.88, clippy validates `disallowed_*` paths (#14397).
- An entry whose path is compiled out (core without `host`) produces the non-lint warning "does not refer to a reachable
  function", only in crates that load `bittensor_core` (#14733). `-D warnings` does not escalate it.
- Every `clippy.toml` entry under a host-gated core module (`client`, `transaction`, `keyfiles`, the drand fetches)
  carries `allow-invalid = true`. The entry is still enforced wherever its path exists.
- The `--all-features` clippy run in the check job enforces these entries. The no-host job adds
  `cargo clippy --locked -p rtao --no-default-features --features validator,miner,prometheus -- -D warnings`."

### TU-28 — confirmed
release-train.yml:83-90 builds the release wasm with srtool, profile production, `BUILD_OPTS="--features=metadata-hash"`.
localnet.sh:40/:47 add `pow-faucet` and `fast-runtime`. In the runtime, lib.rs:904 sets tempo `prod_or_fast!(360,10)` and
lib.rs:967 sets the network rate limit to 7200; pallets lib.rs:1022-1027 drops it to 0 only with `pow-faucet`.
docker-localnet.yml:7-9 publishes `:v<spec>` on every mainnet mirror move. GHCR tags today: v432, v473, v475, v477.

### TU-29 — partially_confirmed
Real:
- release.yml is a single ubuntu-24.04 job, so it cannot produce darwin binaries of a CLI that compiles C.
- It has `pages: write` but neither runs `cargo doc` nor deploys Pages.

Fix defect: GitHub retired the macos-13 image on 2025-12-04 (github.blog changelog 2025-09-19). x86_64 macOS jobs now use
macos-15-intel, which is supported until fall 2027.

Corrected fix:
- Generate release.yml with `dist init` for x86_64-unknown-linux-gnu, aarch64-unknown-linux-gnu, aarch64-apple-darwin and
  x86_64-apple-darwin.
- Runners: ubuntu-24.04, ubuntu-24.04-arm, macos-14 and macos-15-intel (or build x86_64-apple-darwin on macos-14 with
  `--target`).
- Add a pages job (permissions `pages: write`, `id-token: write`; environment `github-pages`). It runs
  `cargo doc --workspace --no-deps --all-features` and `mdbook build book`, merges both into one site directory, then
  runs `actions/upload-pages-artifact` and `actions/deploy-pages`.

### TU-30 — confirmed
golden.json keys include `constants`, with three entries (record_golden.py:115-119 and :337-340):

| Constant | Value |
|---|---|
| ExistentialDeposit | 500 |
| SlotDuration | 250 |
| SS58Prefix | 42 |

The replay lists in §11.2 and PLAN §9.2 and §12.2 omit them.

### TU-31 — confirmed
Devnet already runs 478 (CORRECTIONS #2). `cmp finney.scale devnet.scale` shows exactly one differing byte. The drift job
compares each network only with its own index, and whether another change lands during M1 is up to upstream.

### TU-32 — confirmed
The check job runs `cargo nextest run --workspace --all-features --profile ci` on Ubuntu and macos-14. rtao-testkit
features are `docker`, `interop` and `core-oracle` (PLAN §4.3). No nextest profile or test-target gating is defined
anywhere. Upstream's pattern is `[[test]] e2e … test = false` (bittensor-core Cargo.toml:13-18).

### TU-33 — confirmed
The oracle's per-op seeds are imperative Python: test_intents_table.py:27-31, :44-60 and :99-105. The fee comes from the
FakeSubstrate constructor, `self.fee = Balance.from_rao(124_414)` (fake_substrate.py:147), and is asserted at :110. The
literal-only extractor cannot capture function bodies or instance attributes.
