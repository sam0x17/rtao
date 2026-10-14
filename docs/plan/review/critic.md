# Completeness critic: report

Scope: PLAN.md end to end, plus the parts of architecture.md, parity-inventory.md and python-module-map.md that it
relies on, checked against `UP/` at `850f2755`. Backups of the four files before my edits are in
`review/critic/backup/`. The edit scripts (exact-string replacements, each asserted to match once) are in
`review/critic/scripts/`. After the edits, every relative link and anchor in the four files resolves (0 broken), every
Markdown table row has its header's cell count, every YAML block in Appendix E parses (Ruby `YAML.safe_load`), and
every TOML block parses (`tomllib`).

## 1. Residuals the plan editor left for the orchestrator (all applied)

The fix-log listed PLAN-only decisions that architecture.md and the module map did not carry yet. Both documents now
agree on all of them:

- **§1.8, §1.9**: `allow-invalid = true` on the seven host-gated `clippy.toml` entries, and the ten paths as
  `core_surface.rs` compile probes (CB-08).
- **§2.2, §2.7 (new JSON row), §4.2, §4.8, §7.2**: the `serde_json/arbitrary_precision` decision. This covers
  `rtao_primitives::json`, the `Value`-based deserializers, `rtao_btauth::server::Json<T>`, and a comparator that
  compares integers exactly and non-integers as f64, replacing "parsed f64 values compare exactly" (CB-05, TU-13,
  MC-08).
- **§9.3 step 8, §16 #20**: an `EpochSkipped` slot or a lowered tempo, not deferral, causes the divergence; the
  safety-net vectors are added (CB-11).
- **§1.6 G3, §3.9.4, Appendix D PR-U2**: the `Sudo.Sudid` `sudo_result` defect and its tests (CC-12).
- **§14, rewritten**:
  - M4 is 7 EW and M8 5.5.
  - Upstream tracking has its own row, and the total reads 43.5 + 5.5-12.5.
  - The calendar is updated.
  - Per-milestone security passes are added.
  - `extract` v0 moves to M0; `core-oracle` v0 moves from M1 to M2.
  - The exits for M3 and M7 are per MC-05 and MC-11.
  - M6's CLI part depends on M4 and M5.
  - Fuzz and bench start in M1.
- **§15.1**: R7 and R18 updated; R19 and R20 added.
- **§15.2**: Q1's default stated, Q10 marked resolved, Q18 and Q19 added.
- **`RemoteSigner` moved to M10 and out of `full`**: §2.1, §2.2, §2.4, §6.4, §6.5, §10.2, §10.6, D-031, and a new
  §17 #22.
- **Other sections**:
  - §9.11 and §17 #18: the auto-update attestation check is mandatory.
  - §11.4: the scenario 10 recipe, and `Scenario` setup through the typed builders.
  - §11.6 lane 3 and Appendix E: a temporary keychain, with the Keychain test landing in M4.
  - §12.8: milestone version numbering and the supported platforms.
  - §10.6: the M4 CLI machinery and prompt module, aliases added together with their groups, and M8's alias check.
  - §11.2: the 83-op plan test's assertions.
  - §11.5: the `vault.uos` dumps.
- **Module map**: `macos_password.py` moves to M4, and the prompt-module gap is decided.

## 2. New findings, fixed in place

| # | Sev. | File(s) | What was wrong | Evidence | Fix applied |
|---|---|---|---|---|---|
| N1 | major | PLAN §12.1/§12.4/§12.5/§1.4/§12.13, arch §14 | The M2 validator API needs M3 deliverables. `NeuronCx` holds `metagraph: MetagraphHandle` and `http: SignedClient`. `Validator::on_metagraph` takes a `MetagraphDiff`. The M2 example calls `cx.http.fan_out`. But `MetagraphSync`, `SignedClient` and `rtao_primitives::axon` were scheduled in M3, so M2 could not compile its own design | arch §9.1 (`NeuronCx`) and §9.2 (trait, example); old PLAN §12.5 and arch §14 M3; facade `neuron = [..., "btauth-client", ...]` (§2.4) | Moved the `MetagraphSync` validator preset and the btauth client adapter to M2. M2 is now 5.5 EW and M3 2.5 [I]. Anchors: validator about week 11 (one engineer); the critical path is 33 EW and the off-path work 10.5 EW. M3 keeps the miner preset, event-triggered refresh and `persist` |
| N2 | major | PLAN §12.3/§12.4/§14.4, arch §14, §12.3 | No milestone delivered any of the following, although tracking is budgeted from week 5.5 and each milestone ends with a release: `pin-bump`, `oracle-refresh`, `upstream-next`, `core-canary`, `live-probe`, `features-powerset`, `release-plz`/`release`, `xtask compat` and the run-time compatibility guard | grep of PLAN §12 and old arch §14: only "the tracking row" and M1's `metadata-drift` | M1: `xtask compat` v0 and the guard, the release workflows, `live-probe`, and `core-canary` (build only) plus `features-powerset`. M2: `pin-bump` end to end with `oracle.yml`, `record`, `parity report` and auto-bless; `upstream-next`; the canary's `:main` smoke. New M2 exit: `xtask pin --parity <tag> --force` must leave `git diff --exit-code` clean. The workflow only bumps when S > `parity.spec` (Appendix E), so this re-pin is the only way to exercise the pipeline before a real bump |
| N3 | major | PLAN §9.4/§12.4/§12.7, arch §11.4/§14 | Localnet scenario 8 (runtime upgrade mid-session) had no milestone | PLAN scenario lists: M2 1,2,3,5,6,7,15,16,17; M3 9; M4 4,10,11; M5 11,12,13,18; M7 14,19 | M2, with the offline-payload assertion in M5. The wasm source is the `[localnet].main` image by digest (`UP/Dockerfile-localnet:103, 115`). Skip rule: `can_set_code` rejects a spec that does not increase (polkadot-sdk `cacb431` `frame/system/src/lib.rs:2320-2349`) |
| N4 | major | arch §4.9/§11.4, inventory §2.5, PLAN §8.1/§9.4/M2/M4/M6/M7/M8 | `e2e_map.toml` had no state for a key whose rtao test does not exist yet. As a result, "fails on an unmapped key" from M2 was either impossible to satisfy or inert, since keys could point at test names that do not exist. Also, 15 of the 39 upstream `test_*` names appeared in no document | `e2e-manifest.json` against the three documents (15 names absent); `e2e.rs:288 … 1319` | States `rtao`/`na`/`pending = "M<k>"`. `rtao` values are resolved with `syn` against the testkit test functions, and strict mode (M8) allows no `pending`. The 15 names are assigned in inventory §2.5: M2 6, M4 4, M6 4, M7 1 |
| N5 | major | arch §2.2/§2.3/§2.7/§6.5, PLAN §4.2/§4.3/M7, module map | The Vault signer is a loopback HTTP+WebSocket server, but dependency rule 2 allowed tokio-tungstenite only under `extension`. No QR encoder was planned (Python renders byte-mode, ECC M, border-4 SVG frames) | `PY/vault/server.py:26, 69-80, 181-242`; `PY/vault/qr.py:15-28`; `PY/vault/__init__.py` `__all__` | Rule 2 now also covers `vault`. §6.5 specifies `VaultSessionServer`: routes, JSON protocol, close codes 1008/1012, hyper upgrade. A QR row is added to §2.7 (crate chosen in M7 [I]; tests decode the symbol back to the frame). Module map rows updated, with a decided Gaps row |
| N6 | major | arch §11.6/§10.3/§14 M4/M8, PLAN §8.1/§9.5/M4/M8 | Nothing compared the `--json` output of the 163 hand-written commands with btcli's, although JSON is the stated parity contract and `--json` is a global flag. Lanes 5 and 8 cover only reads, `tools` and `tx` plans | arch §11.6 lanes 1-8; PLAN §8.1 | Interop lane 9. Both CLIs run through `rtao-testkit::PinProxy`, a proxy built on FakeNode that pins head queries and hash-less `state_*` calls to one block (Python queries the head without a hash, `PY/client.py:206-207`). Read-only commands are compared with `--json` and mutating ones with `--dry-run --json`. Wall-clock fields are listed in `cli_json_volatile.toml`. The lane starts in M4 and gates the M8 exit |
| N7 | major | arch Appendix E e2e.yml/§12.2, PLAN §9.7/M3 | No rtao CI job generated the template and built it against the PR's commit, so the template's own CI would build the last tag. A PR touching only `templates/` skipped the localnet as docs-only | e2e.yml `changes` filter `"!templates/**"`; no template job | A `template` job (`xtask template check`): `cargo generate`, then a path `[patch."https://github.com/sam0x17/rtao"]` for `rtao` and `rtao-testkit`, a BuildKit named context for the images, then the generated project's CI steps. It is part of `e2e-required`, and `templates/` now counts as code |
| N8 | minor | arch Appendix E nightly.yml | `nightly.yml` was invalid YAML: the `run:` scalar contained `"core-canary: …"`. GitHub would reject the whole workflow, which also drops `upstream-next`, `fuzz`, `bench` and `features-powerset` | Ruby `YAML.safe_load`: "mapping values are not allowed … line 26" | Changed to a block scalar. All 11 Appendix E workflows parse |
| N9 | minor | PLAN §4.3/§11/§12.14, arch §2.1/§12.8 | `cargo doc --workspace` collides: the lib `rtao` and the bin `rtao` both write `target/doc/rtao/`, so the Pages docs can be overwritten | reproduced on cargo 1.99.0 ("output filename collision", cargo#6313); `doc = false` removes it | Set `[[bin]] doc = false` in `rtao-cli` |
| N10 | minor | module map, arch §11.2, PLAN §9.2/M1/M4/M6 | The module map marked 11 unit tests `ported`. But arch §11.2 ports `test_lock_ownership_gate`, and the M7 exit requires ports of the ledger, extension bridge, extension signer and EVM tests. Five behavior tests the oracle cannot reach were `oracle-only`: PoW challenge, owner pruning advancing on chain, natural name order, facades, commitments accounting | `PYT/unit/*` headers; module map row; PLAN M7 exit | 21 tests `ported`, each in the milestone of its code |
| N11 | minor | arch §4.9 layer 8, PLAN §8.3 | The completeness layer checked only the root `__all__`. 14 other public `__all__` lists (`evm` 38 names, `intents` 97, `reads` 21, `extension` 16, `vault` 5, …) were unchecked | grep and `ast` over `PY/` | Each listed name needs a `#[doc(alias)]` or a reasoned row in the new `parity/exports_extra.toml` |
| N12 | minor | PLAN M0/M2-M4/M9, arch §2.4/§9.11/App. E | No milestone for: `rtao wallet save-password-env`, the three profile examples (which the `features` job compiles), `rtao_metagraph::persist`, `auto-update`. `xtask lint-upstream` was wired into no workflow | grep | Scheduled them in M4; M2/M3/M4; M3; M9. `lint-upstream` runs in the `check` job from M0 |
| N13 | minor | PLAN M2/§12.13, arch §14 | The 24 h testnet soak had no host, and GitHub-hosted jobs stop after 6 h | — | A long-lived host as a lead-time item; the evidence is a `/metrics` record in `docs/soak/m2.md` |
| N14 | minor | arch §2.1, PLAN §4.3 | The xtask subcommand lists omitted commands that Appendix E calls: `fuzz`, `bench`, `ci issue`, `upstream parity-tag`, `localnet run/smoke`, `mainnet-clone`, `template` | grep of `cargo xtask` uses | Lists completed |
| N15 | minor | arch §4.8 | Where the per-branch read samples live was unspecified | — | `vendor/oracle/samples_extra.json` under `reads`, dumped by the oracle |
| N16 | minor | arch §11.6, PLAN §9.5 | "`python -m bittensor`" does not exist: the package has no `__main__` | `find UP/sdk/python -name __main__.py` finds only `codegen/` | Reworded |
| N17 | minor | module map | The `cli/call.py` row still said `--allow-raw`, which PP-22 removed | arch §10.1 | Row corrected |
| N18 | minor | arch §2.7, PLAN §5.7 | The ring-TLS rule's client list omitted the CLI's HTTP fetchers (GitHub releases for `upgrade`, the indexer for `wallet history`) | `PY/cli/upgrade_helpers.py:103-129`, `PY/cli/commands/wallet.py:1335-1352` | Added to the list |

PLAN §14.4 (new, rows 15-27) indexes these decisions, and PLAN §18.2 gains re-checked facts 22-27.

## 3. Still open

- **User decisions** (defaults stated in PLAN §14.1 and arch §15.2): Q1 license, Q2 the "Python only in CI" reading,
  Q3 binary name, Q4 btauth `<Bytes>` default, Q5 repository, Q17 Ledger in CI, Q18 upstream PR timing, Q19 who
  performs the security passes.
- **Upstream**: acceptance of PR-U1…U16 (Q7); deferral in the reveal predictor (Q9); timing of extrinsic v5 (Q11);
  exposing `SelectiveMetagraphIndex` (Q12); `Metagraph.total_stake` typing (Q14).
- **Measurement**: endpoint quotas (Q6, M1); `subxt` without jsonrpsee (Q15, M7); the tokio-tungstenite 0.30
  handshake API (Q16, M1).
- **Estimates and choices marked [I]**: the 0.5 EW moved from M3 to M2; about 0.5 EW for `PinProxy` and lane 9 inside
  M4's CLI estimate; the QR crate (M7); release-plz key names for 19 `publish = false` crates with one facade tag; the
  `governor` crate. The two-engineer anchors are left at validator ≈ week 8, on the assumption that engineer B builds
  the btauth client adapter within "M2's needs".
- **Not re-verified here**: GitHub's 6-hour limit for hosted jobs (from GitHub's documentation, not measured); whether
  a GitHub-hosted macOS runner allows `security create-keychain` without a UI prompt (the plan's temporary-keychain
  steps are the common CI recipe, not run here).
