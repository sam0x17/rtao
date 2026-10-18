# Verification of the milestones-critic findings (MC-01 … MC-27)

Skeptic pass. Every finding was re-checked against PLAN.md, architecture.md, both inventories, `UP/` at `850f2755`
(and tags v445…v477), `REG/` sources in `~/.cargo/registry` and `SP/meta/explore`. Nothing was built or run inside
upstream directories.

Summary: 11 confirmed, 15 partially confirmed (problem real, fix corrected), 1 refuted (MC-24).

| ID | Verdict | One-line reason |
|---|---|---|
| MC-01 | confirmed | 10 of M6's 24 commands submit M5 intents; `pending.toml` lists intents; the two-engineer table closes M6 before M5 ends |
| MC-02 | partially confirmed | M1 exit uses `client.subnets()` (namespaces are M6, dispatch M2); the fix also needs the no-inherent-method rule and the full commitment line ranges |
| MC-03 | partially confirmed | M4 commands prompt (`typer.prompt`, `masked_input`, `..prompt`); PLAN puts the prompt module and all aliases in M8. The fix leaves M5-M7 aliases unassigned and misdescribes `rpassword` |
| MC-04 | partially confirmed | imbalance verified (8,389 machinery lines in M4 vs 2,851 M8-only lines); "scope moved" is overstated (architecture §14 M4 always had CLI v0); numbers are [I] |
| MC-05 | partially confirmed | owner member needs M4/M7 APIs; the stronger M3 exit needs a stated subnet-setup path (`Scenario`) |
| MC-06 | partially confirmed | ring/aws-lc-sys compile C via `cc`; rustc links via `cc`. The fix's exit list drops `fernet`, `ansible-vault` and the core's `reqwest` from the §5.7 union |
| MC-07 | confirmed | rustls `from_crate_features` panics with 0 or 2 providers; tokio-tungstenite calls `ClientConfig::builder()`; reqwest 0.12 = ring, 0.13 `rustls` = aws-lc-rs |
| MC-08 | confirmed | `_jsonable` passes ints through; `max_difficulty` default `u64::MAX/4` and U64F64 `bits` exceed 2^53; f64 collapses neighbours |
| MC-09 | partially confirmed | churn verified exactly; the two-engineer arithmetic is off and the batching policy is an unforced process change |
| MC-10 | partially confirmed | M9-only review and optional attestation are real; the "second reviewer" gate needs a named reviewer for a one-engineer team |
| MC-11 | partially confirmed | M7 exit covers only Ledger/PoW; extension interop needs a scripted browser-side peer; moving `RemoteSigner` needs `full`/§10.2 edits |
| MC-12 | confirmed | every listed architecture contradiction verified (§1.3, §2.6, §4.9, §10.6, §11.2, §14) |
| MC-13 | partially confirmed | GITHUB_TOKEN PRs trigger nothing, no `core-oracle` job, interop never on PRs, no concurrency, narrow paths. Requiring a path-filtered `e2e` check would block every non-matching PR |
| MC-14 | partially confirmed | single-runner `cargo dist build` and no Pages deploy verified; the replacement builds on ubuntu-24.04 (glibc 2.39 floor) and omits the `github-pages` tag rule |
| MC-15 | confirmed | 22 constructors at `transaction.rs:251-778`; only `transfer`, `transfer_all` pair in M2 |
| MC-16 | confirmed | decision is consistent; M4 would also be consistent, but M2 is a valid choice |
| MC-17 | partially confirmed | tautology argument right; exact test also needs rows for zero-hit reads (`timestamp`, `block_time`, `is_fast_blocks`, `block_info`, `commitments`) |
| MC-18 | confirmed | `INTENT_SAMPLES` covers all 83 ops; upstream plan test only plans (`e2e.rs:81-96`) |
| MC-19 | partially confirmed | gaps real (testkit has no `rtao-intents`; patch blocks crates.io `w3f-bls`); `rtao_client::compose` holds only wrappers, so setup calls come from `rtao_runtime::calls` |
| MC-20 | partially confirmed | M0 check job cannot run `regen --check`; semver needs a tag; no M1 lane. `test_balance_properties` is hypothesis-based, so nothing literal to extract |
| MC-21 | partially confirmed | nightly fuzz/bench in M9 verified; the fix misdates targets (`UnsignedExtrinsic` import is M5, JSON-RPC frames M1) |
| MC-22 | partially confirmed | lane 3 Keychain vs M7 and Ubuntu docker verified; `btcli wallet keychain save` prompts via `osascript`, so CI must use library calls |
| MC-23 | partially confirmed | Q10 answered by source; the queued case dissolves a subnet and lowers a global limit, so it needs a dedicated container |
| MC-24 | refuted | Q18 is a user decision; PLAN already recommends early filing and states its interim default |
| MC-25 | partially confirmed | Apache-2.0 §4(a)/(b) gaps real; license metadata must cover every crate with derived files, not three |
| MC-26 | confirmed | tag-per-milestone vs breaking-drift minors collide; `semver-checks` fails breaking PRs without a pre-bump |
| MC-27 | confirmed | no platform statement; macOS x86_64 untested; lead-time line implies docker on macOS |

---

## MC-01: confirmed

Checked `PY/cli/commands/lock.py:11` (imports) and submits at `:264-265, :286, :311, :330`; `root.py:24`, `:487-488`
(`ClaimRootWithHotkey` in `_claim_position`), `:633-635` (`StakeIntoBasket`, `allocate`), `:708` (`RootRegister`);
`root_move.py:12, :202-203`; `root_swap.py:11, :150, :201`; `collateral.py:10, :128-129, :170`. Inventory §3: all ten
are M5, `root_register` M4. PLAN §12.4 says `pending.toml` lists 74 intents, so "empty" at M6 needs M5. 24 = 14 + 10
checks out. The proposed table is internally consistent (M7 after M5; B ports CLI v0 in 10-13). If MC-02 is applied,
read "M6 reads, namespaces" as "M6 reads and the remaining namespace methods".

## MC-02: partially confirmed

Verified: PLAN §12.3 exit uses `client.subnets().metagraph(NetUid(1))`; §12.4 puts `ReadName`/`dispatch` in M2 and
§12.8 puts "the 14 namespaces" and the curated extras in M6; §14.3 #5 keeps an "or". `PY/namespaces.py:161-173`
defaults `commitments=True`, which runs `_fetch_commitment_map` and `_commitments` (`PY/metagraph.py:238-295, 406`).
The M1 checklist names only `PY/metagraph.py:297-403`. The catalog's `python` is `null` for `metagraph` (jq over
`CAT/reads.json`).

The fix needs two additions: if the generator emits an inherent `Subnets::metagraph`, it shadows the extension-trait
method (inherent methods win), so it must say no method is generated when `python` is null. And the commitment
machinery spans `:37-125`, `:258-295` and `:406-562`. `reveals_at` uses `PY/timelock.py:175-177`, which `rtao-reads`
must compute from core constants, not from `rtao_tx::timelock` (an L5 peer, M7).

## MC-03: partially confirmed

Verified: `PY/cli/commands/wallet.py:27` (`masked_input`), `:56` (`..prompt`), `:57` (`..secrets`), `:164`
(`typer.prompt(..., hide_input=True)`), `:202` (`masked_input`), `:343` (new-coldkey prompts). `PY/keyfiles.py:42-62`.
The module map marks `_live.py`, `masked_input.py`, `cli/prompt.py`, `context.py`, `main.py`, `globals.py`, `call.py`,
`call_names.py`, `intent_prompts.py`, `secrets.py` and `logs.py` M4, and `output.py` M4-M8. PLAN §12.10 puts the prompt
module and all aliases in M8; inventory §7.3 says each alias "lands with its target group".

Overstated: §12.6 does list `globals`, `call` and generated `tx`/`query`. The fix gives aliases only to "groups
shipped so far" (M4) and to M8, which leaves the M5-M7 groups (`st`, `rt`, `lock`, `collateral`, `timelock`,
`extension`) unassigned. `rpassword` hides input; it does not give masked (`*`) entry.

## MC-04: partially confirmed

`wc -l` confirms 8,389 lines for the 12 M4 machinery files and 2,851 for the M8-only files. M4 also carries 11 intents,
the shield pipeline, `PowSolver`, `OwnerOps`, `OwnerMonitor` and 22 reads. The imbalance is real. Overstated:
architecture §14 M4 always contained "CLI v0 (§10.6)", so the claim that scope moved out of M8 without the EW does not
hold. Only §14 M8's wording ("every hand-written group") disagrees with §10.6. Both the old and the new figures are
[I], and R7 uses M8's EW as the long-tail buffer. The corrected fix keeps the reviewer's numbers, fixes the rationale
and mirrors the change in architecture §14.

## MC-05: partially confirmed

Architecture §2.4 `owner` enables `rtao-neuron/owner` (OwnerOps, M4) and the `proxy`/`multisig`/`collateral` groups
(M5). `ledger`'s `LedgerSigner` is M7. PLAN §7.5 ships an owner tool in M3, so in M3 it can only be a stub. The
stronger exit is right, but a generated project cannot set up a subnet in M3: `rtao localnet` is M4 CLI and
registration/staking intents are M4/M5. The corrected fix names `rtao-testkit`'s `Scenario` (M2) as the setup path.

## MC-06: partially confirmed

`REG/reqwest-0.13.5/Cargo.toml`: `default-tls = ["rustls"]`, `rustls = ["__rustls-aws-lc-rs", ...]`.
`REG/aws-lc-sys-0.45.0/Cargo.toml:107-112` lists `cc` and `cmake` as build dependencies. `REG/ring-0.17.14/build.rs:507-508`
uses `cc::Build`. Every named preset is `wss://` (`PY/settings.py:33-36`), and rustc links through `cc` on
`*-linux-gnu`, so "no C compiler" is false. The fix's exit list drops `fernet`, `ansible-vault` and the core's blocking
`reqwest` from the §5.7 union, and does not add `libsodium-sys` or `secp256k1-sys` to deps-check rule 5.

## MC-07: confirmed

- `REG/tokio-tungstenite-0.28.0/Cargo.toml`: `rustls` and `tokio-rustls` use `default-features = false`.
- `src/tls.rs` builds its config with `ClientConfig::builder()`.
- `REG/rustls-0.23.45/src/crypto/mod.rs:241-285`: `from_crate_features` returns `None` for zero or two providers, then
  `expect(...)` panics.
- `REG/reqwest-0.12.28` `rustls-tls-native-roots` enables `__rustls-ring`; the core uses it under `host`
  (`UP/sdk/bittensor-core/Cargo.toml:60`).
- `REG/reqwest-0.13.5` `rustls` enables aws-lc-rs.
- Architecture §2.7 names no provider (grep finds no `ring`, `aws-lc` or `CryptoProvider`).

The fix's APIs exist: `connect_async_tls_with_config(req, Option<WebSocketConfig>, bool, Option<Connector>)`,
`Connector::Rustls(Arc<ClientConfig>)`, `builder_with_provider`, `with_safe_default_protocol_versions`,
`tls_backend_preconfigured`, and `rustls-no-provider`, whose hyper-rustls and rustls dependencies have default
features off.

## MC-08: confirmed

`PY/cli/query.py:104-116` passes ints through unchanged. `max_difficulty` defaults to `u64::MAX / 4`
(`UP/runtime/src/lib.rs:942`), and `float(a) == float(a+1)` is `True` (python3). `subnet_hyperparameters` also returns
raw U64F64 `bits` (`PY/reads/subnets.py:133-147`), which are at or above 2^64 for values of 1.0 or more.

## MC-09: partially confirmed

`git` re-check: 48,735 / 58,269 / 59,723 `.py` lines at v445 / v459 / v477. `git diff --shortstat v459 v477` gives 42
files, +2,636/−942. There are 20 tags between 2026-08-12 and 2026-10-08. Two corrections:

- The two-engineer anchor is wrong. The load is per calendar week and shared by two people. Solving
  T = base + (r/2)(T − 5) for r = 0.125-0.25 and base = 26-28 gives about 27.5-31.5, not 29-33. The one-engineer
  figure, including the extra tracking weeks, is about 49-56.
- "Merge Python-only pin bumps in batches" needs a rolling pin-bump PR. That conflicts with the per-tag PRs of §10.3
  and MC-13, and the evidence does not require it.

## MC-10: partially confirmed

Architecture §9.11: "`SHA256SUMS` and optional attestation verification". A checksum file from the same release gives
no authenticity, so attestation must be mandatory. PLAN §12.11 places the only review in M9, and R18 relies on it,
while M2 is labelled "production". The fix's "second reviewer" needs a named reviewer for a one-engineer team (the
user or an external party).

## MC-11: partially confirmed

PLAN §12.9's exit says "demonstrated" and has no criteria for EVM, timelock, Keychain or `RemoteSigner`. The upstream
tests exist (`test_extension_bridge.py` 94, `test_extension_signer.py` 48, `test_ledger_signer.py` 152,
`test_evm.py` 154 lines), and `PY/vault/uos.py:54-123` is pure. Python's `--signer` offers wallet, extension, ledger
and vault, with no remote (`PY/cli/globals.py:275-285`). Three corrections:

- The bridge relays to a browser page (`PY/extension/bridge.py`), so interop needs a scripted browser-side peer.
- Keychain must follow MC-22's corrected job.
- Moving `RemoteSigner` also means editing `full` (architecture §2.4), §10.2 and the M7 `--signer` list.

## MC-12: confirmed

All claims verified:

- §1.3 item 3 says `rtao-subnet-template`, while §2.1 has `templates/subnet/`.
- §2.6's owner block has no `[patch]`.
- §4.9 `e2e_map.toml` lists only "each of the 112". `PYT/e2e` holds three more tests.
- §10.6 puts set-take/stake-burn in M4 through `hparams`, and lists `inspect` in both M4 and M8.
- §11.2 says "73 intent-plan tests collapse". `e2e.rs:164-169` `intent_set_root_claim_type` asserts only `IntentCall`
  results.
- §14 M0-M6 texts match the quotes.

The replacement text is consistent with PLAN and the inventories. "covers all 83 ops" in §11.2 assumes MC-18,
which is confirmed.

## MC-13: partially confirmed

- Appendix E uses `secrets.GITHUB_TOKEN` for `gh pr create` and for the drift PR. GitHub does not start workflow runs
  for events created by `GITHUB_TOKEN`, except `workflow_dispatch` and `repository_dispatch`.
- `nightly.yml` has no `core-oracle` job.
- `interop` runs only on schedule.
- `pin-bump.yml` has no `concurrency`. Every 30-minute run would also find the same tag pending until merge.
- The `e2e` paths omit `rtao-testkit`, `rtao-runtime`, `Cargo.lock` and others.

Fix error: branch protection that requires `e2e / localnet (pinned, fast)` while `e2e.yml` keeps a workflow-level
`paths:` filter leaves the check pending and blocks every PR that does not match. A matrix job skipped by `if:` does
not report the expanded name either. The corrected fix uses a `changes` job, job-level `if` and a non-matrix gate job.
The checkout must also use the App token so branch pushes trigger runs.

## MC-14: partially confirmed

Appendix E `release.yml` is one ubuntu-24.04 job with `cargo dist build` and `pages: write`, but it has no `cargo doc`
and no deploy step. Upstream builds the host core per target, with macOS x86_64 cross-built on macos-14
(`UP/.github/workflows/build-core-wheels.yml:34-40, 120-133`), and uses manylinux containers to keep a low glibc floor
(same file, comments above the maturin step). The replacement builds Linux binaries on ubuntu-24.04, which needs
glibc 2.39, so the binaries do not run on Ubuntu 22.04 or Debian 12. Two further gaps: the `github-pages`
environment rejects tag deployments unless a tag rule admits `rtao-v*`, and `release-plz` stays named but unwired.

## MC-15: confirmed

`awk` over `CORE/transaction.rs` finds 22 constructors (`:251-778`). Inventory milestones give 2 in M2, 5 in M4 and
11 in M5. Four constructors have no catalog op.

## MC-16: confirmed

Inventory §4.1 shows 1/9/36 by milestone. The generator inputs (`intent_types.toml`, `intent_callmap.toml`, typed
builders from M1) exist by M2. "Only one answer fits" is overstated: generating in M4 and hand-writing `transfer_all`
once is also consistent. M2 is still a sound decision.

## MC-17: partially confirmed

Deriving `ReadSpec.hits` from the recording makes the equality test tautological, and `verify_surface` consumes
`hits` (architecture §4.8). However, an exact test "for every read, zero-hit reads included" also fails for the
zero-hit reads that reach items through view methods: `timestamp` (`view.timestamp()`), `block_time`,
`is_fast_blocks`, `block_info` (`PY/reads/chain.py:11-59`) and `commitments` (`fetch_commitments`). Inventory §5.15
names these five, but the fix gives rows only to its 12.

## MC-18: confirmed

`INTENT_SAMPLES` (`PYT/harness/samples.py:35`) has 83 keys, including all 11 ops without an `intent_*` test (checked
with `ast`). The upstream test only plans (`e2e.rs:81-96`). `stake_into_basket` needs only a root-registered hotkey
and an idle seed (`pallets/subtensor/src/staking/claim_root.rs:377-404`), so scenario 18 is feasible on localnet.

## MC-19: partially confirmed

Architecture §2.2: `rtao-testkit` depends on client, rpc, tx and wallet, not intents. The setup calls exist in
`SP/meta/explore` (SubtensorModule `:7,16,40,69`; AdminUtils `:31-32,47,55,66,78,95-97`). With the `[patch]`,
crates.io `w3f-bls` cannot be built inside rtao's workspace (architecture §1.3). The soak hotkey needs a permit before
any rtao registration or staking exists. Fix error: `rtao_client::compose` holds only `sudo`, `proxy`, `batch_all`
and `as_multi*` (architecture §3.12). Setup calls come from the typed builders in `rtao_runtime::calls` (§3.8),
submitted with `Client::submit`.

## MC-20: partially confirmed

Appendix E's `check` job runs `regen --check` (M1) and `extract --check` (M1). `semver` needs an `rtao-v*` tag.
§12.14 uses `rtao-testkit::fixtures` in M0, while §4.3 says M1. M1 has scenario 15 and `core-oracle` v0 but no lane.
§9.2 says inputs are "extracted, not copied", and the btauth goldens are Python literals
(`test_http_auth.py:19-36`). Fix error: `test_balance_properties.py` is hypothesis-based (`@given` strategies), so
there are no literal cases to extract. It is ported as proptests.

## MC-21: partially confirmed

Verified: PLAN §12.11 and architecture §14 M9 schedule the nightly lanes, while §2 and §9.6 promise 30% gates and M1
has the baselines and a fuzz target. Fix error, checked against PLAN §12.7 and architecture §11.7:
`UnsignedExtrinsic` import is M5, not M2, and JSON-RPC frame parsing is M1 (`rtao-rpc`), not M3.

## MC-22: partially confirmed

Verified: architecture §11.6 lane 3 says "Keychain entries work both ways on the macOS runner", yet no macOS interop
job exists and docker runs only on Ubuntu. Keychain is M7 (§12.9), while lane 3 is in M3. Fix error:
`btcli wallet keychain save` prompts through an `osascript` dialog (`PY/macos_password.py` `prompt_password`;
`cli/commands/wallet.py:847-880`), which headless CI cannot answer. Use `keychain_save`/`keychain_load` library calls
in a temporary default keychain.

## MC-23: partially confirmed

Verified: `sudo_set_network_immunity_period` (call_index 35, `admin-utils/src/lib.rs:1215-1228`) and
`sudo_set_subnet_limit` (37, `:1250-1258`) exist on finney (`AdminUtils_calls.txt:65, 83`). The prune branch and the
queue are at `subnet.rs:204-213, 233-260`, and immunity skipping at `coinbase/root.rs:358-361`. Missing from the fix:
the queued path dissolves the lowest-priced subnet and lowers a global limit, so it must run in a dedicated container
(architecture §11.4: "Global-state tests get a dedicated container").

## MC-24: refuted

Q18 is a decision about public upstream engagement on the maintainer's behalf. PLAN already recommends "file each PR when
its evidence exists" and states the interim default (M9), as the instructions require for items needing maintainer input.
The fix makes filing the default while also calling it "the maintainer's call", which contradicts itself. The R3 and R16
mitigations (template, `rtao doctor`, isolation) do not depend on early filing. Branch readiness is already shown
by §15 "Ready in".

## MC-25: partially confirmed

`UP/LICENSE` is Apache-2.0, and `UP/` has no NOTICE file at depth 3 or less. The plan's default (an MIT LICENSE plus a
NOTICE) omits the Apache-2.0 text (§4(a)) and in-file change notices (§4(b)). The fix labels only three crates
`MIT AND Apache-2.0`, although file-for-file ports land in most crates (`rtao-tx`, `rtao-intents`, `rtao-reads`,
`rtao-client`, `rtao-rpc`, `rtao-wallet`, `rtao-cli`, …). Forcing `rtao-core-ext` to Apache-2.0-only overrides the
maintainer's Q1 for code the maintainer authors.

## MC-26: confirmed

PLAN §12.1 (`rtao-v0.N.0` per milestone) conflicts with §10.5 (breaking drift takes a minor before 1.0).
`cargo semver-checks --baseline-rev <last tag>` fails breaking changes unless the version moved after the tag.

## MC-27: confirmed

Appendix E's `check` matrix has no macOS x86_64. §12.8 ships that binary. The lead-time line lists macos-14 among
"runners with docker", while Appendix E says docker exists only on Ubuntu. One evidence point is overstated: v11 has
an sdist fallback, so it can install on Windows with a Rust toolchain. The fix's own wording ("ships no Windows core
wheels") is accurate.
