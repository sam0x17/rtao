# Review: plan quality as a whole (milestones critic)

Scope read: PLAN.md (all), architecture.md §0, §14-§17, Appendices C-E (plus the sections they cite), both
inventories, draft-notes.md. Verified against `UP/` at `850f2755` (and tags `v445`…`v477`), `REG/` sources in
`~/.cargo/registry`, and `SP/meta/explore`.

Link check: all 242 relative links in PLAN.md resolve, and every anchor matches a heading under GitHub slug rules
(script: `scratchpad/review/tools/anchors.py`); the inventories' and architecture's links also resolve. No finding.

Numbers re-checked and consistent across PLAN.md and the inventories: per-milestone intents/reads/CLI counts
(9/11/63; 2/13/1/22/1/63; 39/20/24/34/46 = 163), groups complete (3/2/4/3/7 = 19), 46 pass-through bodies (1/9/36),
EW total 43.5 and critical path 32.5, `pending.toml` after M2 (74 intents, 87 reads).

Severity: blocker = rtao built per the plan would be broken or miss scope; major = significant gap, inaccuracy or
contradiction; minor = small but real inaccuracy. No blockers found.

---

## MC-01 (major, PLAN.md) M6 cannot depend only on M2; its exit criterion and the two-engineer lanes are infeasible

**Location.** §12.1 row M6 (`M2 [I]`; "read oracle 102/102; `pending.toml` empty"); §12.8 "**Depends on** M2 [I: M6
is off the critical path; it needs the read infrastructure, the oracle image and `FakeChain`]" and exit "`pending.toml`
empty"; §12.13 two-engineer rows "8-13 | … | M3 (miner about week 10), then M6" and "13-19.5 | M5 | M6, then M8 groups…".

**Problem.** All 24 M6 CLI commands run on the M4 CLI v0 machinery; 10 of them construct M5 intents and `root register`
constructs the M4 `RootRegister`. `pending.toml` also lists intents (§12.4), so "empty" needs M5 done. The
two-engineer table closes M6 inside weeks 13-19.5 although M5 ends at week 19.5.

**Evidence.** `PY/cli/commands/lock.py:11` imports `LockStake, MoveLock, SetPerpetualLock, SetRejectLockedAlpha`
(submitted by `conviction add/mode/accept/move`); `PY/cli/commands/root.py:24` imports `ClaimRootWithHotkey,
RootRegister, StakeIntoBasket` (`root claim` :494 via `_claim_position` :436, `root allocate` :598, `root register`
:687); `root_move.py:12` `MoveStake`; `root_swap.py:11` `SwapBasket`; `collateral.py:10` `AddCollateral,
SetMinCollateral` (`add` :90, `set-min` :138). parity-inventory §3: `lock_stake`/`move_lock`/`set_perpetual_lock`/
`set_reject_locked_alpha`, `add_collateral`/`set_min_collateral`, `move_stake`, `stake_into_basket`, `swap_basket`,
`claim_root_with_hotkey` are M5; `root_register` is M4. PLAN §12.4: "`pending.toml` listing the 74 unported intents and
87 unported reads".

**Fix.** §12.1 row M6: Depends on → "M2 (reads, namespaces, `blocking`); M4 and M5 for its CLI [I]"; headline exit →
"read oracle 102/102; `pending.toml` holds no reads". §12.8: replace the Depends-on clause with "**Depends on** M2 for
the reads, namespaces and the `blocking` facade; M4 (CLI v0 machinery) and M5 for its 24 CLI commands: `root
claim/allocate/move/swap`, `conviction add/mode/accept/move` and `collateral add/set-min` construct M5 intents
(`ClaimRootWithHotkey`, `StakeIntoBasket`, `MoveStake`, `SwapBasket`, `LockStake`, `SetPerpetualLock`,
`SetRejectLockedAlpha`, `MoveLock`, `AddCollateral`, `SetMinCollateral`), `root register` the M4 `RootRegister`."
Exit: "`pending.toml` empty" → "`pending.toml` holds no reads (it is empty once M5 is also done)". §12.13: after "M3, M6
and M7 (11 EW) can run in parallel with it" add "(M6's CLI part follows M5)" and replace the two-engineer table with:

| Weeks [I] | Engineer A | Engineer B |
|---|---|---|
| 0-2 | M0 seam, primitives, btauth core | M0 xtask, CI; then extraction, oracle image, `map.toml`, codegen |
| 2-8 | M1, M2 (validator about week 8) | parity machinery through M2's needs; M3 from about week 7 |
| 8-13 | M4 owner tooling, shield, intents, reads (owner tooling about week 13) | M3 (miner about week 10), then the M4 CLI v0 machinery and commands (MC-04) |
| 13-19.5 | M5 | M6 reads, namespaces, `blocking`; M6's 14 commands that need no M5 intent; M8 groups whose intents and reads exist |
| 19.5-26 | M7, then M8 | M6's 10 M5-dependent commands (closes M6), then M8 |
| 26-28 | M9 | M9 |

## MC-02 (major, PLAN.md) The M1 exit criterion uses `client.subnets()`, which no M1 task builds, and the gap #5 cycle is left open

**Location.** §12.3 exit "`client.subnets().metagraph(NetUid(1))` on finney decodes…"; §12.4 "Generated interfaces,
`AnyIntent`, `defaults.rs`, `ReadName`/`dispatch`" (M2); §12.8 "the 14 namespaces" (M6); §14.3 #5 default "expose
`metagraph()` as an extension trait or from the facade [I]".

**Problem.** `Subnets` is one of the 14 generated namespace accessors (architecture §2.1 "generated specs/dispatch/namespaces", §4.8),
which PLAN schedules in M6, with `ReadName`/`dispatch` in M2; and the curated `metagraph()` it must expose needs
`rtao-metagraph`, which sits above `rtao-reads` (gap #5, still "or"). The curated method also defaults to fetching
commitments, whose state machine is in no M1 task. The M1 exit cannot be built from M1's task list.

**Evidence.** architecture §4.8 ("the 14 categories map to accessor structs"; curated extras
`subnets().…metagraph/commitments…`); §2.3 L6 `rtao-metagraph → reads`; `PY/namespaces.py:161-173` (`metagraph(…,
commitments: bool = True)` → `metagraph_module.fetch`); `PY/metagraph.py:45-125, 258` (commitment status machine,
`fetch_commitments`); the `metagraph` read itself is only `SubnetInfoRuntimeApi.get_metagraph` plus `utf8_text`
(`PY/reads/subnets.py:168-189`), so it has no cycle.

**Fix.** §12.3 "Metagraph and reads", add: "- [ ] `xtask regen` v1 also emits from vendored `CAT/reads.json`:
`ReadName`, `ReadSpec`, `dispatch` (100 arms as `NotYetPorted` stubs) and the 14 namespace accessor structs (named from
`category`, inventory §8 #12) on `Client` and `Snapshot`; `parity/pending.toml` starts here with 100 reads." and "- [ ]
Decided (§14.3 #5): the commitment status machine (`PY/metagraph.py:45-125`, `fetch_commitments` `:258`) lives in
`rtao_reads::commitments`; `Subnets::metagraph()` comes from `rtao_metagraph::SubnetsMetagraphExt` (extension trait,
re-exported in `rtao::prelude`), defaulting to `commitments = true` as Python does; the `metagraph` read stays in
`rtao-reads`." §12.4: delete "`ReadName`/`dispatch`" from the M2 bullet. §12.8: "the 14 namespaces" → "the remaining
namespace methods and the curated extras". §14.3 #5: Resolve in "M1 (decided)", Default = the decided text above.

## MC-03 (major, PLAN.md) CLI v0 needs the prompt module and the CLI machinery, which PLAN puts in M8 or nowhere

**Location.** §12.10 "- [ ] Hidden v9 aliases and snake_case aliases; `PyJsonFormatter` …; shell completions; the CLI
prompt module (`_live.py`, `masked_input.py` equivalents)."; §12.6 CLI v0 checklist (commands only).

**Problem.** M4's `wallet new-coldkey`, `regen-*` and every coldkey-signed M4 command (`hparams set`, `subnets
register/create`) prompt on a TTY, so the prompt module is an M4 dependency; python-module-map.md schedules it, and the
whole CLI machinery (about 8.4k Python lines), in M4, but §12.6 has no task for any of it. Aliases: inventory §7.3 lands
each alias with its target group, PLAN puts all in M8.

**Evidence.** `PY/cli/commands/wallet.py:164` (`typer.prompt(…, hide_input=True)` for the mnemonic), `:202`
(`masked_input` JSON password), `:282-283, 343` (new-coldkey "Prompts for an encryption password unless
--no-password"); coldkey unlock on a TTY is `PY/keyfiles.py:42-62` `_prompt_password` → `masked_input`.
python-module-map.md rows: `_live.py` M4, `masked_input.py` M4, `cli/prompt.py` (718) M4, `cli/context.py` (2,140) M4,
`cli/output.py` (2,484) M4-M8, `cli/main.py` (549) M4, `cli/globals.py` (542) M4, `cli/call.py` (597) M4,
`cli/call_names.py` M4, `cli/intent_prompts.py` M4, `cli/secrets.py` M4, `cli/logs.py` M4. inventory §7.3: "Each lands
with its target group"; snake_case aliases apply "including the generated `tx`/`query` commands".

**Fix.** §12.6 add before the CLI v0 command bullets: "- [ ] CLI machinery (python-module-map.md `cli` rows marked M4):
ports of `context.py`, `globals.py`, `main.py` (tree, hidden v9 and snake_case aliases for the groups shipped so far),
`output.py` (JSON value-equal; tables), `prompt.py`, `intent_prompts.py`, `call.py`, `call_names.py`, `secrets.py`,
`logs.py`, and the prompt plumbing of `masked_input.py`/`_live.py` (masked entry through `rpassword`), needed by `wallet
new-coldkey/regen-*` and every coldkey-signed command on a TTY (D-003)." §12.10 replace the bullet with "- [ ] Aliases
of the M8 groups; `PyJsonFormatter` (2-space indent, Python float `repr`, non-ASCII preserved); shell completions; the
interactive pickers and `stake wizard` (`stake_picker.py`)."

## MC-04 (major, PLAN.md) The M4/M8 effort split contradicts the CLI scope the inventories assign

**Location.** §12.1 rows M4 (4.5 EW) and M8 (8 EW); §1.4 "owner tooling, MEV shield, CLI v0 | M4 | about week 18";
§12.13 one-engineer row; §13 R7 "M8 has 8 EW".

**Problem.** Architecture §14 sizes M8 as "every hand-written group (§10.5)" at 8 EW, but §10.6 and the inventories
put the CLI machinery and 39 commands in M4 and 117 of the 163 commands in M4-M7, leaving M8 46 commands. The EW figures
were not moved with the scope, so M4 is under-budgeted and the owner-tooling date is optimistic.

**Evidence.** python-module-map.md line counts: CLI machinery whose first milestone is M4 = 8,389 Python lines
(`context.py` 2,140, `output.py` 2,484, `prompt.py` 718, `call.py` 597, `main.py` 549, `globals.py` 542, `tx.py` 498,
`query.py` 266, `call_names.py` 240, `intent_prompts.py` 177, `logs.py` 103, `secrets.py` 75) plus M4 command code
(`null_pruning.py` 392, most of `sudo.py` 528, `metagraph_view.py` 211, `hyperparams_view.py` 151, `weights.py` 195,
`axon.py` 68, `config.py` 94, part of `wallet.py` 1,784 and `subnets.py` 634). Files whose only milestone is M8 total
2,851 lines (`help_theme` 48, `stake_exit` 222, `stake_picker` 676, `upgrade_helpers` 721, `addresses` 271, `crowd` 58,
`misc` 204, `upgrade` 651) plus the rest of `wallet.py`, `stake.py`, `proxy.py`. M4's 4.5 EW also carry 11 intents,
the MEV-shield pipeline, `PowSolver`, `OwnerOps`, `OwnerMonitor` and 22 reads.

**Fix.** §12.1: M4 EW 4.5 → 7 [I]; M8 EW 8 → 5.5 [I] (total 43.5 and critical path 32.5 unchanged). §12.13 one engineer:
"| 0-2 | 2-5.5 | 5.5-10.5 | 10.5-13.5 | 13.5-20.5 | 20.5-27 | 27-31 | 31-35 | 35-40.5 | 40.5-43.5 |". §1.4: M4 row "about
week 20.5 | about week 13" (two engineers keep week 13 because engineer B ports the CLI v0 machinery in weeks 10-13; see
the MC-01 table). R7: "M8 has 5.5 EW after the CLI machinery moved to M4".

## MC-05 (major, PLAN.md) The M3 template ships an owner tool whose APIs arrive in M4 and M7, and "builds" is its only exit test

**Location.** §7.5 "a workspace with a validator, a miner and an owner tool … It lands in M3 ('the template builds from
`cargo generate`')"; §12.5 checklist "`templates/subnet`" and exit "the template builds from `cargo generate`"; §7.1
owner manifest `features = ["owner", "ledger"]` at `rtao-v0.5.0`.

**Problem.** The facade `owner` feature turns on `rtao-neuron/owner` (`OwnerOps`, M4) and intent groups whose bodies
land in M4/M5; `LedgerSigner` is M7. In M3 the owner member can only be a stub. "Builds" does not show the template
works, although it is the main entry point for subnet builders.

**Evidence.** architecture §2.4 `owner = ["neuron", "wallet-encrypted", "rtao-neuron/owner", "rtao-intents/registration",
…, "proxy", "multisig", "collateral"]`; PLAN §12.6 (OwnerOps, owner intents M4), §12.7 (proxy/multisig/collateral M5),
§12.9 (`LedgerSigner` M7).

**Fix.** §7.5: "It lands in M3 with the validator and miner members, their no-host Dockerfile stages, compose and CI;
the owner-tool member (features `["owner"]`, the owner image stage with Perl and a C compiler) joins in M4, and `ledger`
in M7." §12.5 exit: replace "the template builds from `cargo generate`" with "a project generated with `cargo generate`
passes its own CI: both neuron images build, and on the compose localnet the miner answers a btauth/1-signed request
from the validator and the validator's commit is revealed (`TimelockedWeightsRevealed`)". §12.6 add "- [ ] Template:
add the owner-tool member and owner image stage; its CI runs `OwnerOps` (create, start, one hyperparameter) against the
compose localnet." §7.1: illustrative tags → `rtao-v1.0.0`.

## MC-06 (major, PLAN.md) "No C compiler" for neuron images is false, and the M2 exit criterion built on it is not testable

**Location.** §1.4 row "weight-setting validator, no C toolchain"; §1.5 #5 "their images need no libsodium, OpenSSL,
Perl or C compiler"; §6.1 #5; §7.1 manifest comments; §7.5; §12.4 exit "the no-host validator image builds without a
C toolchain"; §17 glossary "no-host profile".

**Problem.** Neurons connect over `wss://`, so they need rustls and a crypto provider; both providers compile C
(`aws-lc-sys` or `ring`), and the `validator`/`miner` features pull reqwest 0.13 whose default TLS is aws-lc-rs.
Linking any Rust binary on `*-linux-gnu` also goes through `cc`. The real, measurable win is "no libsodium, OpenSSL,
Perl or secp256k1-sys", not "no C compiler".

**Evidence.** `PY/settings.py:34-37` (every named preset is `wss://`); `REG/reqwest-0.13.5/Cargo.toml` features
`default-tls = ["rustls"]`, `rustls = ["__rustls-aws-lc-rs", …]`; `REG/aws-lc-sys-0.45.0/Cargo.toml`
`[build-dependencies.cc]`, `[build-dependencies.cmake]`; `REG/ring-0.17.14/build.rs:422-560` (`cc::Build`);
architecture §2.4 `neuron = [… "btauth-client" …]`, §2.7 "reqwest 0.13.5 (rustls)". Without `host` the core leaves
out its C-backed crates (`UP/sdk/bittensor-core/Cargo.toml` comments on `host`: sodiumoxide, openssl/fernet,
`sp-core/std` with C secp256k1), so the remaining C comes from rtao's own TLS stack.

**Fix.** Replace the claim everywhere with "no C libraries to install: no libsodium, OpenSSL, Perl or
`secp256k1-sys`; neuron images build on the stock `rust:<MSRV>-slim` image, whose `gcc` (already the linker driver)
compiles the TLS provider (MC-07)". §1.4 row → "weight-setting validator, no libsodium/OpenSSL/Perl". M2 exit → "the
no-host validator image builds `FROM rust:1.89-slim-bookworm` with no additional system packages, and `cargo tree -p
<validator> -e normal,build` contains none of `openssl-sys`, `openssl-src`, `libsodium-sys`, `sodiumoxide`,
`secp256k1-sys`, `libloading`, `rayon`". Glossary "no-host profile" → "neuron builds without `bittensor-core/host`: no
libsodium, OpenSSL, Perl or `secp256k1-sys`; builds on a stock `rust:slim` image".

## MC-07 (major, architecture.md) The rustls crypto provider is undecided; as specified, host builds get two providers and `client`/`reads`-only builds none, so the first `wss://` connect panics

**Location.** §2.7 rows "WebSocket | `tokio-tungstenite 0.30` (`rustls-tls-native-roots`)" and "HTTP | … `reqwest
0.13.5` (rustls)"; §3.2.

**Problem.** tokio-tungstenite's rustls features carry no provider and its default connector calls
`ClientConfig::builder()`, which resolves the process provider from crate features and panics unless exactly one of
`ring`/`aws_lc_rs` is enabled. The core's host-only reqwest 0.12 enables `ring`; rtao's reqwest 0.13 enables
`aws_lc_rs`; so `owner` and `full` (the CLI) enable both, and `default-features = false, features = ["client"]` or
`["reads"]` enable neither. FakeNode is plain `ws://`, so PR gates never see it.

**Evidence.** `REG/tokio-tungstenite-0.28.0/Cargo.toml:157-160, 179-182` (`rustls`, `tokio-rustls` with
`default-features = false`), `src/tls.rs:126` (`ClientConfig::builder()`); `REG/rustls-0.23.45/src/crypto/mod.rs:243-285`
(`from_crate_features` returns `None` for two or zero providers; `expect("Could not automatically determine the
process-level CryptoProvider…")`); `REG/reqwest-0.12.28/Cargo.toml` `rustls-tls-native-roots = […, "__rustls-ring"]`
with `UP/sdk/bittensor-core/Cargo.toml` reqwest under `host`; `REG/reqwest-0.13.5/Cargo.toml` `rustls =
["__rustls-aws-lc-rs", …]`; architecture §11.3 FakeNode on `127.0.0.1:0` without TLS. [I] tokio-tungstenite 0.30 is not
in the local registry; 0.28 is the version the architecture verified.

**Fix.** Add to §2.7: "**TLS provider: `ring`, set explicitly.** `rtao-rpc` depends on `rustls` (`default-features =
false, features = ["ring", "std", "tls12", "logging"]`) and connects with
`tokio_tungstenite::connect_async_tls_with_config(req, cfg, false,
Some(Connector::Rustls(Arc::new(ClientConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider())).with_safe_default_protocol_versions()?.with_root_certificates(roots).with_no_client_auth()))))`,
never through the process default. `rtao-btauth`, `rtao-tx` (drand) and `rtao-evm` use reqwest 0.13 with
`default-features = false, features = ["rustls-no-provider", "http2", "json", …]` and
`ClientBuilder::tls_backend_preconfigured(config)` with the same provider. `ring` matches the core's reqwest 0.12, so
host builds keep one provider, and it needs only `cc`. `xtask deps-check` rule 7: no `aws-lc-sys` in any profile. M1
test: `FakeNode` gains a TLS mode (self-signed CA injected into the root store) and a `wss://` connect runs under the
`client`, `validator` and `full` profiles."

## MC-08 (major, PLAN.md) The default "compare numbers as f64" for oracle JSON silently accepts wrong integers above 2^53

**Location.** §14.3 #1 Default "…; compare numbers as f64; …"; §13 R19 mitigation "compare oracle JSON numerically".

**Problem.** Python renders integers exactly; converting them to f64 makes distinct u64 values equal. Read outputs carry
u64 values far above 2^53 (9,007,199,254,740,992), so the read oracle, the central correctness gate, would pass
off-by-one results.

**Evidence.** `PY/cli/query.py:104-116` (`_jsonable` passes ints through); `difficulty` (an M4 read,
`PY/reads/hyperparameters.py:18-22`, `SubtensorModule.Difficulty: u64`) and `subnet_hyperparameters` (raw values via
`get_subnet_hyperparams_v3`, `PY/reads/subnets.py:156-167`; `max_difficulty` mapped at `PY/hyperparams.py:424`), whose
runtime default is `u64::MAX / 4` = 4,611,686,018,427,387,903 (`UP/runtime/src/lib.rs:942`); as f64 it and its
neighbours collapse to 4.611686018427388e18. With `arbitrary_precision`, `serde_json::Number` keeps the digits, so an
exact comparison is available.

**Fix.** §14.3 #1 Default: replace "compare numbers as f64" with "compare JSON numbers by kind: two integer literals
(no `.`, `e` or `E`) compare exactly as i128/u128 (canonical decimal strings beyond 128 bits); two non-integers compare
as f64 (shortest round-trip, so `1e-05` == `1e-5`); integer vs non-integer is a mismatch; unit-tested with 2^53 + 1,
`u64::MAX / 4` and `1e-05`/`1e-5`". R19 mitigation: same wording instead of "compare oracle JSON numerically".

## MC-09 (major, PLAN.md) The effort model ignores upstream churn during the 43-week build, although `parity_tag` follows every release and M8 gates on zero pending

**Location.** §12.1 "Total to full parity 43.5"; §1.4; §10.6 "Budget a standing 0.25-0.5 engineer per week once parity
is reached"; §13 (no churn risk).

**Problem.** The 43.5 EW port the v477 surface, but `parity_tag` moves on every finalized release (§5.5) and the M8
strict gate needs zero pending at whatever tag is current then. No milestone budgets the porting of upstream changes
to files already ported, or of new ops, during M2-M9.

**Evidence.** Re-checked: `.py` lines in `sdk/python/bittensor` are 48,735 at v445 (2026-08-12), 58,269 at v459, 59,723
at v477 (2026-10-08), +22.5% in 8 weeks; `git diff --shortstat v459 v477 -- sdk/python/bittensor` = 42 files,
+2,636/−942 in 3.3 weeks; 20 release tags from v445 to v477 in 57 days, 3 of them abandoned (v470, v471, v476; `git for-each-ref refs/tags`, architecture §1.2); PLAN §10.1: Python
changed in all six measured release pairs.

**Fix.** §12.1 add a row before the total: "| | Upstream tracking during M2-M9 (pin bumps, re-ports of changed files,
new ops and reads) | 5-10 [I] | — | every pin-bump PR merged or labelled |" with the note "0.125-0.25 EW per week from
week 5.5: half the steady-state 0.25-0.5 of §10.6, because the ported surface grows from none to all". Total → "43.5 +
5-10 tracking [I]". §1.4 full-parity row → "about week 49-54 | about weeks 29-33" and a note that anchors after M2 move by
the tracking accumulated by then. Add risk "R20 (new) | upstream churn moves the parity target during development |
certain / medium | budgeted tracking share; until M8, pin-bump PRs that change only Python behavior are merged in batches
at milestone boundaries (core bumps and `breaking` drift at once); the porting report sizes each batch".

## MC-10 (major, PLAN.md) Key handling ships in M2-M4 as "production" but is first security-reviewed in M9

**Location.** §12.11 "- [ ] Security review of signers, keyfiles, secrets handling and the nonce store."; §12.4 Goal "A
production weight-setting validator"; §13 R18 mitigation "… M9 security review".

**Problem.** Untrusted keyfile parsing and the password chain (M2), the btauth/1 verifier and nonce store (M3), and
coldkey unlock, CLI wallet creation and owner operations (M4) are tagged and offered to operators about 30 weeks before
M9 (week 40.5 for one engineer). `auto-update` makes attestation verification optional.

**Evidence.** PLAN §12.4 (`rtao-wallet`, `keyfile_json`, password chain), §12.5 (`BtAuthLayer`, `RedisNonceStore`),
§12.6 (owner intents, `wallet new-coldkey/regen-*`); §1.4 operator dates; architecture §9.11 "`auto-update` … `SHA256SUMS`
and optional attestation verification".

**Fix.** Add a checklist item before the tag of M2, M3, M4 and M7: "- [ ] Security gate (second reviewer; findings in
`docs/security/mN.md`): M2 untrusted keyfile parsing (`keyfile_json` fuzz target green), password chain, `Zeroizing` and
no `Debug` on secrets, signing path; M3 btauth/1 verifier (freshness window, nonce-store bounds, body limit, header
fuzzing), `PerHotkeyLimiter`; M4 coldkey unlock, CLI prompts, MEV-shield envelope, `call --allow-raw` policy; M7
Ledger, Vault and extension-bridge token handling." §12.11 → "Full security review (external if available) of all of
the above". README and release notes until 1.0: "coldkey features are pre-audit". `auto-update`: "attestation
verification against the rtao release workflow is mandatory when the feature is on; SHA256SUMS alone is refused". R18
mitigation: "per-milestone security gates; full review in M9".

## MC-11 (major, PLAN.md) M7's exit criteria are not objective and cover only Ledger and PoW

**Location.** §12.9 exit "the Ledger proof vector is green plus one manual device run; Vault and extension flows are
demonstrated; `pow_register` e2e passes on the 12 s lane"; deliverables `RemoteSigner`, Keychain, `rtao-evm`, timelock
UX, 34 CLI commands.

**Problem.** "Demonstrated" is not a test; `rtao-evm`, the 25 `evm` commands, timelock UX, Keychain and `RemoteSigner`
have no exit criterion. `RemoteSigner` is an rtao addition defined only as a client.

**Evidence.** Upstream tests to port: `PYT/unit/test_extension_bridge.py` (94 lines), `test_extension_signer.py` (48),
`test_ledger_signer.py` (152), `test_evm.py`; UOS framing is pure and oracle-dumpable (`PY/vault/uos.py:54-123`:
`sign_tx_payload`, `sign_frame`, `multipart_frames`, `transaction_frames`, `transaction_frames_with_proof`);
architecture §6.5 `RemoteSigner`: "POST /sign with the SigningRequest minus secrets, authenticated by btauth/1" (no
server, schema or test anywhere).

**Fix.** M7 exit → "the Ledger proof vector is green plus one manual device run (Q17); `VaultSigner` UOS frames are
byte-equal to oracle vectors from `PY/vault/uos.py` and a localnet `submit_signature` round trip passes with a scripted
QR responder; ports of `test_extension_bridge.py` and `test_extension_signer.py` are green and an interop test signs a
localnet transfer through rtao's bridge with Python's `extension/client.py`, and through Python's bridge with rtao; ports
of `test_evm.py` are green and EVM keystore-V3 files cross-decrypt with Python; `rtao timelock encrypt/decrypt`
round-trips with Python both ways; Keychain entries round-trip with btcli on the macOS job (MC-22); `pow_register` e2e
passes on the 12 s lane." Move `RemoteSigner` and feature `remote-signer` to M10 (rtao addition with no Python
counterpart; it needs a wire schema, a reference server that decodes and policy-checks the call before signing, and
replay tests). Add `vault.uos` framing to the oracle's pure-function dumps (§9.5).

## MC-12 (major, architecture.md) §14, §10.6, §11.2, §4.9, §2.6 and §1.3 still carry the milestone text PLAN.md and the inventories corrected

**Location / problem / evidence.** PLAN.md calls architecture.md "the authoritative design" but resolves these
contradictions only in PLAN.md and the inventories (draft-notes inventory #2, #4, #7, #11, #13, #14, #15; plan step #7,
#8, #9, #14):
§14 preface "The parity machinery (D, A) is in place from M0" while M0 lists no parity task; §14 M2 "signers
(`KeypairSigner`)" while M0's btauth/1 core signs through `&dyn Signer` (§7.1) and M0's `rtao doctor` needs `rtao-cli`;
§14 M1 schedules the D-001 localnet check but `Localnet` is M2; §14 M3 and §1.3 item 3 say `rtao-subnet-template`,
§2.1 says `templates/subnet/`; §14 M4 exit "shielded scenarios green" includes `add_collateral` (M5); §14 M5 "the
remaining ~60 intents (46 pass-through bodies generated…)" vs 63 remaining with 36 generated; §14 M5 exit "all 73
`intent_*` e2e names mapped and green" and §11.2 "the 73 intent-plan tests collapse into one table-driven test" although
`intent_set_root_claim_type` asserts a retired constructor (`UP/sdk/bittensor-core/tests/e2e.rs:158-169`); §14 M6 "all
102 reads" vs 39 reads in M1-M5; §10.6 puts `set-take`/`stake-burn` in M4 (their intents are M5), lists `wallet
inspect` in M4 and M8, and leaves "proxy, multisig basics" undefined; §4.9 maps only the 112 manifest names although
three `PYT/e2e/*.py` tests are outside it; §2.6's owner-tool manifest has no `[patch.crates-io] w3f-bls` line, so a
copied owner tool fails with E0277 (§1.3).

**Fix.** Replace in architecture.md:
- §1.3 item 3 → "**Template.** The `cargo generate` template `templates/subnet/` (§2.1) ships the patch line once, in
  its workspace root (§2.6)."
- §2.6 owner-tool block: append `[patch.crates-io]` / `w3f-bls = { git = "https://github.com/RaoFoundation/subtensor.git",
  tag = "v477" }`, and after the blocks add "In a workspace holding all three, the line appears once, in the workspace
  root (Cargo ignores `[patch]` in member manifests)."
- §4.9 row `e2e_map.toml` → "each of the 112 manifest names, plus `PYT/e2e/test_null_consensus.py`,
  `test_pow_registration.py`, `test_timelock_commitment.py`, → rtao test or n/a".
- §10.6 M4 "owner `hparams`" → "owner `hparams` except `set-take` and `stake-burn` (their intents are M5)"; M5
  "`proxy`, `multisig` basics" → "`proxy` create/add/remove/kill/list/execute; `multisig` add/list/show/remove/pending;
  `hparams set-take`, `stake-burn`"; M8: delete `inspect` from the `wallet` list.
- §11.2 manifest row → "…; the 72 intent-plan tests collapse into one table-driven test over `AnyIntent`, which covers
  all 83 ops; `intent_set_root_claim_type` asserts a retired core constructor with no catalog op and is mapped n/a".
- §14 preface → "The parity machinery starts in M0 (file layer, first `map.toml` rows) and is complete in M2 (all
  layers), so nothing ported early has to be re-done."
- §14 M0 deliverables append "; `rtao-signer` `Signer` + `KeypairSigner`; `rtao-cli` with `rtao doctor` only; `xtask
  parity check` file layer; `rtao-testkit::fixtures`". M1 append "; minimal `Localnet` (D-001 check); reads
  `metagraph`, `token_symbols`". M2: "signers (`KeypairSigner`)" → "signer roles (`KeySource<R>`, `DynKeySource`)",
  "`Localnet` + `Scenario`" → "`Scenario`", append "; 13 reads". M3: "`rtao-subnet-template`" → "`templates/subnet`
  (validator and miner members)", append "; read `immunity_period`". M4: append "; 22 reads"; exit "owner scenario
  (immediate and queued create) and shielded scenarios green" → "scenario 10 (immediate and queued create) and
  scenario 11 for `burned_register` green (`add_collateral` joins in M5)". M5: "the remaining ~60 intents (46
  pass-through bodies generated; the rest ported)" → "the remaining 63 intents (36 with generated pass-through bodies;
  10 of the 46 land in M2/M4)", append "; read `proxies`"; exit "all 73 `intent_*` e2e names mapped and green" → "all
  73 `intent_*` names mapped: the 72 plan tests green, `intent_set_root_claim_type` mapped n/a". M6: "all 102 reads,
  14 namespaces, remaining models" → "the remaining 63 reads (39 land in M1-M5), the remaining namespace methods and
  models"; exit "`pending.toml` empty" → "`pending.toml` holds no reads".
- Keep the §14 exit cells of M2, M3 and M7 identical to PLAN §12.4, §12.5 and §12.9 once MC-06, MC-05 and MC-11 are
  applied.

## MC-13 (major, architecture.md) Appendix E cannot enforce the gates PLAN §9.7 and §10 promise

**Location.** Appendix E `drift.yml` (`--apply --open-pr … auto-merge if surface-unchanged`, `GH_TOKEN: "${{
secrets.GITHUB_TOKEN }}"`), `pin-bump.yml` (`gh pr create` with `GITHUB_TOKEN`, cron `*/30`, no `concurrency`),
`nightly.yml` (`interop` on schedule only; no `core-oracle` job), `e2e.yml` path filter; PLAN §9.7 "`surface-unchanged`
auto-merges", "merge needs green `pr-gates`, `e2e` on `:vS`, `interop`", "`core-oracle` | nightly; pin bumps".

**Problem.** Pull requests and pushes made with the repository `GITHUB_TOKEN` do not start `pull_request`/`push` workflow
runs, so drift and pin-bump PRs get no `pr-gates` or `e2e` runs: required checks never report (auto-merge stalls) or,
without branch protection, auto-merge lands untested code. `interop` never runs on a PR, the `core-oracle` lane has no
job, a 30-minute cron with an oracle build and recorders can overlap and open duplicate PRs, and `e2e` skips PRs that
change the signing seam, signers, generated descriptors, the test harness or `Cargo.lock`.

**Evidence.** GitHub Docs, "Automatic token authentication → Using the GITHUB_TOKEN in a workflow": events triggered by
the `GITHUB_TOKEN`, except `workflow_dispatch` and `repository_dispatch`, do not create new workflow runs. Appendix E
`e2e.yml` paths list only `rtao-rpc`, `rtao-client`, `rtao-tx`, `rtao-intents`, `rtao-reads`, `rtao-neuron`,
`rtao-metagraph`, `upstream.toml`, `vendor/**`.

**Fix.** (1) In `drift.yml` and `pin-bump.yml`, mint a GitHub App installation token
(`actions/create-github-app-token@<sha>` with `app-id: ${{ vars.RTAO_BOT_APP_ID }}`, `private-key: ${{
secrets.RTAO_BOT_KEY }}`; contents and pull-requests write) and use it as `GH_TOKEN` for branch pushes, `gh pr create`
and `gh pr merge --auto --squash` (the last only for `surface-unchanged`). (2) Branch protection on `main` requires
`pr-gates / check (ubuntu-24.04)`, `pr-gates / no-host`, `e2e / localnet (pinned, fast)` and `interop`; `core-oracle`
stays an alert, as PLAN §9.7 says.
(3) New `interop.yml` with `on: { pull_request: { types: [opened, synchronize, labeled] }, schedule: [{ cron: "0 2 * *
*" }] }`; jobs `interop` and `core-oracle` (`cargo nextest run -p rtao-testkit --features docker,core-oracle --profile
core-oracle --locked`), each with `if: github.event_name == 'schedule' || contains(github.event.pull_request.labels.*.name,
'pin-bump')` (a skipped required check counts as passed on other PRs; `core-oracle` gets `continue-on-error: true`); remove `interop` from `nightly.yml`. (4) `pin-bump.yml`:
`concurrency: { group: pin-bump, cancel-in-progress: false }`, and exit early when an open PR titled "pin: … →
${{ steps.pin.outputs.tag }}" exists. (5) `e2e.yml` paths add `crates/rtao-core/**`, `crates/rtao-core-ext/**`,
`crates/rtao-signer/**`, `crates/rtao-wallet/**`, `crates/rtao-runtime/**`, `crates/rtao-catalog/**`,
`crates/rtao-btauth/**`, `crates/rtao-testkit/**`, `Cargo.toml`, `Cargo.lock`. Update PLAN §9.7 triggers for `interop`
and `core-oracle` to "nightly; PRs labelled `pin-bump`".

## MC-14 (major, architecture.md) The release workflow cannot build the advertised binaries and never publishes rustdoc

**Location.** §12.8 "Binaries: `cargo-dist` builds the `rtao` CLI for linux x86_64/aarch64 and macOS arm64/x86_64";
Appendix E `release.yml` (one job, `runs-on: ubuntu-24.04`, `cargo dist build --tag …`); PLAN §3 "rustdoc goes to GitHub
Pages", §9.7 `release` row.

**Problem.** One x86_64 Linux job cannot produce macOS or aarch64 Linux binaries of a CLI that builds the core with
`host` (vendored OpenSSL via Perl, libsodium); the job has `pages: write` but no `cargo doc`/Pages deployment;
`release-plz` and the release notes with the downstream patch line are not wired.

**Evidence.** architecture §1.7 and R4 (the `host` C stack); upstream builds the same core's macOS artifacts on
`macos-14` for both `aarch64-apple-darwin` and `x86_64-apple-darwin` and Linux per target
(`UP/.github/workflows/build-core-wheels.yml:34-40, 123-133`).

**Fix.** Replace `release.yml` with a matrix build plus a publish job:

```yaml
name: release
on: { push: { tags: ["rtao-v*"] } }
jobs:
  build:
    strategy:
      fail-fast: false
      matrix:
        include:
          - { os: ubuntu-24.04,     target: x86_64-unknown-linux-gnu }
          - { os: ubuntu-24.04-arm, target: aarch64-unknown-linux-gnu }
          - { os: macos-14,         target: aarch64-apple-darwin }
          - { os: macos-14,         target: x86_64-apple-darwin }     # as UP/.github/workflows/build-core-wheels.yml:123-133
    runs-on: ${{ matrix.os }}
    steps:
      - uses: actions/checkout@<sha>
      - uses: actions-rust-lang/setup-rust-toolchain@<sha>
        with: { target: "${{ matrix.target }}" }
      - run: cargo build --release --locked -p rtao-cli --target ${{ matrix.target }}
      - run: tar -C target/${{ matrix.target }}/release -czf rtao-${{ github.ref_name }}-${{ matrix.target }}.tar.gz rtao
      - uses: actions/upload-artifact@<sha>
        with: { name: "rtao-${{ matrix.target }}", path: "rtao-*.tar.gz" }
  publish:
    needs: build
    runs-on: ubuntu-24.04
    permissions: { contents: write, pages: write, id-token: write, attestations: write }
    environment: github-pages
    steps:
      - uses: actions/checkout@<sha>
      - uses: actions-rust-lang/setup-rust-toolchain@<sha>
      - uses: actions/download-artifact@<sha>
        with: { path: dist, merge-multiple: true }
      - run: cd dist && sha256sum rtao-*.tar.gz > SHA256SUMS
      - uses: actions/attest-build-provenance@<sha>
        with: { subject-path: "dist/*" }
      - run: cargo xtask compat --write COMPAT.md --json compat.json && cargo xtask release-notes > notes.md   # prints the w3f-bls line on core bumps
      - run: gh release create "$GITHUB_REF_NAME" dist/* compat.json COMPAT.md --notes-file notes.md
        env: { GH_TOKEN: "${{ github.token }}" }
      - run: cargo doc --workspace --no-deps --all-features --locked && cargo xtask docs --llms && mdbook build book && cp -r target/doc book/book/api
      - uses: actions/upload-pages-artifact@<sha>
        with: { path: book/book }
      - uses: actions/deploy-pages@<sha>
```

§12.8 "Binaries": "built per target on native runners (macOS x86_64 cross-built on `macos-14`, as upstream does), with
SHA256SUMS and GitHub attestations; rustdoc and the book deploy to GitHub Pages from the same workflow".

## MC-15 (minor, PLAN.md) The M2 policy equivalence test can pair only 2 of its 18 constructors

**Location.** §12.4 `rtao-tx` bullet "`Policy` with the equivalence test against the core (18 paired constructors plus 4
hand-built subjects, §5.8 #9)"; §14.3 #10 "Resolve in M2".

**Problem / evidence.** The core's 22 constructors (`CORE/transaction.rs:251-826`) pair with catalog ops `transfer`,
`transfer_all` (M2), `register_subnet`, `start_call`, `burned_register`, `root_register`, `set_hyperparameter` (M4) and
`add_stake`, `add_stake_limit`, `remove_stake`, `remove_stake_limit`, `move_stake`, `swap_stake`, `transfer_stake`,
`unstake_all`, `unstake_all_alpha`, `set_take`, `fund_evm_key` (M5) (parity-inventory §3); 16 of the 18 intents do not
exist in M2.

**Fix.** Bullet → "`Policy` with a table-driven equivalence test against the core's 22 constructors: M2 pairs
`transfer`, `transfer_all` and builds subjects by hand for `transfer_allow_death`, `move_stake_limit`,
`transfer_stake_and_hotkey`, `set_root_claim_type`; M4 adds `register_subnet`, `start_call`, `burned_register`,
`root_register`, `set_hyperparameter`; M5 adds the other 11 (18 paired at the M5 exit)." §12.7 exit append "policy
equivalence 18 + 4 green". §14.3 #10 Resolve in "M2-M5".

## MC-16 (minor, PLAN.md) Decide §14.3 #2: the pass-through generator lands in M2

**Location.** §12.4 "(`transfer_all` is a pass-through body; decide whether the generator lands now, §14.3 #2)"; §12.6
"Nine of them are pass-through bodies (§14.3 #2)"; §14.3 #2 "open".

**Problem / evidence.** Hand-writing 10 bodies in M2/M4 and regenerating them in M5 contradicts §12.1 "nothing ported
early is redone". `transfer_all` is the simplest member (one call `Balances.transfer_all(dest, keep_alive)`, fields 1:1,
`PY/intents/transfer.py:103-106`); the generator reads the same `codegen/intent_types.toml`/`intent_callmap.toml` that
M2's interface generator already needs (architecture §2.1) [I].

**Fix.** §14.3 #2 → decided: "the pass-through generator (`generated/passthrough.rs`) lands in M2 with `transfer_all`
as its first output; M4's nine (`burned_register`, `register_subnet`, `root_register`, `set_mechanism_count`,
`set_subnet_identity`, `start_call`, `trim_null_subnet_batch`, `trim_subnet`, `update_symbol`) and M5's 36 come out of
it [I: its cost in M2 is offset by not hand-writing 10 bodies]". §12.4: replace the parenthesis with "(generated by the
pass-through generator, which lands here)". §12.6: "Nine of them use generated pass-through bodies."

## MC-17 (minor, PLAN.md) Decide §14.3 #3 (exact hits) before the first conflicting read

**Location.** §14.3 #3 Default "open: a reviewed per-read superset in `parity/`, or derive `ReadSpec.hits` from the
recording"; §12.8 "after deciding how to treat the 12 reads…".

**Problem / evidence.** Deriving hits from the recording makes the test tautological; the free superset for zero-hit
reads hides the same under-reporting. The first conflicting read is an M4 read (`validator_basket_summary`, inventory
§5.15), and `ReadSpec.hits` feeds `verify_surface` (architecture §4.8), which must see `SubtensorModule.DecayingLock`,
`MinerCollateral` and the BetaBasket APIs.

**Fix.** §14.3 #3 → decided: "`parity/read_hits_extra.toml` holds one reviewed row per read of inventory §5.15 (items the
catalog omits, Python helper path, blob SHA); the `RecordingView` test asserts `recorded == catalog.hits ∪ extra[read]`
for every read, zero-hit reads included; `ReadSpec.hits` is generated from the same union; a blob change of a listed
helper re-opens its row through the file layer; an upstream issue asks `generate.py:405-480` to follow module helpers."
Resolve in M2.

## MC-18 (minor, PLAN.md) Decide §14.3 #4: every op gets a localnet plan test

**Location.** §14.3 #4 "Resolve in M5 | oracle vectors only"; §9.4 last bullet; §12.7 "the 72 `intent_*` plan tests as
one table-driven test over `AnyIntent`".

**Problem / evidence.** Upstream's plan test only plans an intent on an owned localnet subnet and checks op and call
data (`UP/sdk/bittensor-core/tests/e2e.rs:81-96`), so a table over `AnyIntent` covers the 11 ops without an upstream
name at no extra cost; four of the eight uncovered ops move balances (`move_swap_stake`, `stake_into_basket`,
`swap_basket`, `swap_basket_many`).

**Fix.** §14.3 #4 → decided: "the M5 table-driven localnet plan test runs over all 83 `AnyIntent` samples (72 mapped to
upstream names; 11 rtao rows: `pow_register`, `add_collateral`, `trim_null_subnet_batch` and the eight); a new
scenario 18 (fast lane, M5) submits `move_swap_stake`, `stake_into_basket`, `swap_basket` and `swap_basket_many` and
asserts the resulting stake and basket positions". §12.7
bullet → "the plan test over all 83 ops (72 upstream names green, `intent_set_root_claim_type` n/a)". §9.4: replace "they
are covered by oracle vectors only unless localnet tests are added" with the decided text.

## MC-19 (minor, PLAN.md) M2's localnet work needs setup paths the plan does not define

**Location.** §12.4 "`Scenario` builder; `e2e` workflow lanes …; scenarios 1, 2, 3, 5, 6, 7, 16 and 17"; §12.13 lead
time "testnet TAO and a testnet subnet with commit-reveal on (M2 soak)"; §5.6 item 4 (scenario 16).

**Problem.** `Scenario::new(&ln).owner().subnet().start().validator().miner().stake(…)` needs `register_network`,
`start_call`, `burned_register`, `add_stake` and sudo knobs before those intents exist (M4/M5), and `rtao-testkit` does
not depend on `rtao-intents` anyway. Scenario 16 needs ciphertexts from a core built against crates.io `w3f-bls`, which
cannot happen inside rtao's workspace because its `[patch]` replaces that version. The testnet soak needs a registered
hotkey with a validator permit before rtao can register or stake.

**Evidence.** architecture §2.2 `rtao-testkit` depends on `rtao-client`, `rtao-rpc`, `rtao-tx`, `rtao-wallet`; §11.4
builder and knobs; metadata has every setup call (`SP/meta/explore/SubtensorModule_calls.txt:7, 16, 40, 69`;
`AdminUtils_calls.txt:31-32, 47, 55, 66, 78, 95-97`); §1.3 (two `w3f-bls 0.1.3` sources → E0277).

**Fix.** §12.4 add: "- [ ] `Scenario` composes setup calls by name with `rtao_client::compose` and signs them through
the M2 submission pipeline, never through intents: Alice-sudo `AdminUtils.sudo_set_network_min_lock_cost`,
`sudo_set_lock_reduction_interval`, `sudo_set_weights_set_rate_limit`, `sudo_set_serving_rate_limit`,
`sudo_set_tx_rate_limit`, `sudo_set_commit_reveal_weights_enabled` (scenario 2), `sudo_set_mechanism_count` (3),
`sudo_set_weights_version_key` (5), `sudo_set_commit_reveal_version` (6); `SubtensorModule.register_network`,
`start_call`, unshielded `burned_register` (the shield rail binds intents, not test setup), `add_stake`." and "- [ ]
Scenario 16 harness: `ci/pr-u4-proof/`, a standalone Cargo project with its own `[workspace]` and no `[patch]`, depending
on `bittensor-core` by path on a temporary checkout of `core.commit` with PR-U4's manifest line applied; `cargo tree -i
w3f-bls` must show one crates.io source; its binary encrypts (round, payload) and prints the ciphertext; the localnet
test submits it with rtao and asserts `TimelockedWeightsRevealed`." Lead time: "testnet TAO, a testnet subnet with
commit-reveal on, and the soak hotkey registered and staked to a validator permit (with btcli, or `Scenario`'s raw
calls pointed at testnet)".

## MC-20 (minor, PLAN.md) M0 and M1 use tooling and lanes scheduled later

**Location.** §12.14 days 5-8 "`rtao-testkit::fixtures`" and "`pyfmt` cases from `test_balance_properties`"; days 8-10
"`rtao-btauth` core with all vectors"; §12.3 "Extraction v0 …" (M1); §4.3 `rtao-testkit` "M1 (fakes, minimal
`Localnet`)"; §12.2 "the first `pr-gates` jobs" (Appendix E `check` runs `regen --check`, `extract --check`); §12.3
"`core-oracle` v0; localnet scenario 15" while `e2e` arrives in M2.

**Problem / evidence.** §9.2 requires the inputs of `test_http_auth` and `test_balance_properties` to be "extracted, not
copied", but the extractor is M1; the btauth/1 goldens are Python literals (`PYT/unit/test_http_auth.py:19-36`:
`FIXED_NONCE`, `BODY`, `METHOD`, `PATH`, `GOLDEN_SR25519_PAYLOAD`, `GOLDEN_ED25519_SIGNATURE`). `rtao-testkit` ships code
in M0. M1's scenario 15 and `core-oracle` v0 have no CI lane until M2 (and `core-oracle` none at all, MC-13).

**Fix.** Move "Extraction v0 (literal-subset extractor on `ruff_python_parser`)" to M0 days 2-4 (it replaces the spike),
first run on the btauth/1 goldens and the `test_balance_properties` cases; M1 extends it to `PY/settings.py` and
`PY/_transport/rpc.py`. §4.3 `rtao-testkit` → "M0 (`fixtures`: golden and corpus loaders), M1 (fakes, minimal
`Localnet`)". §12.2 Tooling and CI: "M0's `check` job runs `extract --check` but not `regen --check` (added in M1); the
`semver` job starts after `rtao-v0.0.0`". §12.3: "- [ ] `e2e.yml` with the `pinned` lane (scenario 15) and the
`core-oracle` job; M2 adds the `mainnet` and `testnet` lanes".

## MC-21 (minor, PLAN.md) Benchmark and fuzz gates start in M9 although baselines and targets exist from M1

**Location.** §12.11 "- [ ] Fuzz and bench in nightly; a weekly mainnet-clone read-only lane."; §12.3 "Criterion
baselines", "a fuzz target"; §2 "criterion gates fail on regressions above 30%"; §9.6 "Nightly fails above a 30%
regression".

**Problem.** For M1-M8 (about 38 weeks) no lane runs the baselines or fuzz targets, so §2's claim holds only after M9.

**Fix.** §12.3 add "- [ ] `nightly.yml` jobs `fuzz` and `bench` (fail above 30%) with the M1 baselines and targets; each
milestone adds its targets (M0 btauth header parsing, M2 `keyfile_json` and `UnsignedExtrinsic` import, M3 frame
parsing)". §12.11 → "- [ ] Review fuzz corpora and raise per-target time; a weekly mainnet-clone read-only lane."

## MC-22 (minor, PLAN.md) Interop lane 3 in M3 includes Keychain, which arrives in M7 and has no runner

**Location.** §12.5 "- [ ] Interop lanes 1-3 (btauth/1 both directions; Python and rtao neurons; wallets)"; §9.5 lane 3
"(3) wallets … including legacy Fernet, Ansible and macOS Keychain".

**Problem / evidence.** Keychain support is an M7 deliverable (§12.9 "macOS Keychain through `/usr/bin/security`"); the
interop lane is docker compose in `nightly.yml` on `ubuntu-24.04`, and "Docker exists only on the Ubuntu runners"
(Appendix E preface).

**Fix.** §12.5: "Interop lanes 1-3 (… wallets without Keychain …)". §12.9 add "- [ ] Keychain interop job on
`macos-14` (no docker): install the pinned Python SDK with `uv` (CI-only Python, Q2) and round-trip a coldkey password
`btcli wallet keychain save` → `rtao wallet keychain show`, and back". §9.5 lane 3: "Keychain on the macOS job (M7)".

## MC-23 (minor, PLAN.md) Q10 is answered by the source; scenario 10 need not be conditional

**Location.** §14.2 Q10 "is there a root setter that forces the queued `register_network` path on localnet
(`SubnetLimit`)? | localnet experiment | M4"; M4 exit "immediate and queued creation" (architecture §11.4 scenario 10 "if
a setter exists").

**Evidence.** `AdminUtils.sudo_set_subnet_limit(max_subnets)` (root, call_index 37,
`UP/pallets/admin-utils/src/lib.rs:1250-1258`) and `sudo_set_network_immunity_period` (call_index 35, `:1215-1228`) exist
and are in finney metadata (`SP/meta/explore/AdminUtils_calls.txt:65, 83`); at the limit with a prunable subnet
`register_network` takes the prune branch (`UP/pallets/subtensor/src/subnets/subnet.rs:204-213`) and queues
(`:233-260`, `NetworkRegistrationQueued`); immune subnets are skipped (`coinbase/root.rs:358-361`).

**Fix.** Move Q10 out of §14.2: "Resolved from source: scenario 10 sets `sudo_set_network_immunity_period(0)`, then
`sudo_set_subnet_limit(<current non-root count>)`, then registers; the call queues
(`NetworkRegistrationQueued`) and the follower waits for completion. The M4 exit criterion is unconditional." Same edit to
architecture §11.4 scenario 10 and §15.2 Q10.

## MC-24 (minor, PLAN.md) Q18's default keeps the only R3 mitigation and the Q7 answer out of reach until after 1.0

**Location.** §14.1 Q18 default "M9, as in architecture §14"; §15 "filing is in M9"; §13 R3 "certain until PR-U4".

**Problem / evidence.** With M9 filing, every subnet carries the `w3f-bls` patch through 1.0, `staged --check` cannot
fire before 1.0, and Q7 (will upstream accept U1-U15) is unanswered until all ~2k staged lines exist; PR-U5 and PR-U7 fix
confirmed bugs for the core's users (§5.8 #1, #2).

**Fix.** Q18 default until the user answers: "prepare each PR branch with its evidence when ready (§15 'Ready in'), and
file PR-U4, PR-U5 and PR-U7 as soon as their evidence exists (M2, M2, M4); the rest are filed in M9. Filing is the
user's call; the branches are ready either way." §12.11 → "PR-U1…U15 filed (those not filed earlier)".

## MC-25 (minor, PLAN.md) The licensing default omits what Apache-2.0 requires and labels copied code MIT

**Location.** §14.1 Q1 default "keep MIT; add a `LICENSE` file and a `NOTICE` with per-file attribution…"; §12.14 day 1
`[workspace.package]` license and LICENSE/NOTICE.

**Problem.** Under MIT, crates with copied or modified upstream code would declare `license = "MIT"`; Apache-2.0 §4
also requires shipping the license text and marking modified files. The copied staged code is meant for upstream PRs.

**Evidence.** `UP/LICENSE` (Apache-2.0); `UP/sdk/python/pyproject.toml:11` and `UP/sdk/bittensor-core/Cargo.toml:7`
(Apache-2.0); no `NOTICE` in `UP/`; PLAN §5.3 G9 "`keyfile_json` (line-for-line copy)"; vendored assets embedded by
`rtao-signer` (`PY/vault/static/*`, `PY/extension/static/bridge.html`) and `rtao-evm` (`PY/evm/abi/*.json`).

**Fix.** Q1 default → "keep MIT for rtao's own code, and under either answer: add `LICENSES/Apache-2.0.txt`; set
`license = "Apache-2.0"` on `rtao-core-ext` (copied upstream code destined for upstream PRs) and `license = "MIT AND
Apache-2.0"` on crates embedding vendored upstream files (`rtao-signer`, `rtao-evm`, `rtao-catalog`); start every ported
or staged file with `// Derived from RaoFoundation/subtensor <path> @ <commit> (Apache-2.0); modified by rtao.`;
`NOTICE` lists rtao's attributions (upstream ships no NOTICE to carry)." §12.14 day 1: add the same files.

## MC-26 (minor, PLAN.md) Milestone tags `rtao-v0.N.0` collide with the drift-driven minor bumps

**Location.** §12.1 "Each milestone ends with … a tag `rtao-v0.N.0`"; §10.5 "a `breaking` drift merge bumps the minor
version before 1.0".

**Problem / evidence.** Any `breaking` drift during development (possible with 9 finalized runtimes in 20 days, §10.1)
takes the next minor and breaks the milestone-number mapping; `cargo semver-checks --baseline-rev <last tag>`
(Appendix E) also fails breaking PRs unless the version was bumped after the last tag.

**Fix.** §12.1 → "a release with the next minor version (`rtao-v0.<k>.0`, in completion order; the changelog and
`compat.json` name the milestone it completes)". §10.5 append: "Before 1.0, milestone releases and `breaking`-drift
releases both take the next minor; right after each tag the workspace version moves to the next minor so
`semver-checks` admits breaking changes during the cycle."

## MC-27 (minor, PLAN.md) No supported-platform statement

**Location.** PLAN.md has none; §12.13 lead time "CI runners with docker (ubuntu-24.04, ubuntu-24.04-arm, macos-14)".

**Problem / evidence.** Users need to know where the libraries, neurons and the CLI are supported. CI checks
ubuntu-24.04, ubuntu-24.04-arm and macos-14 (Appendix E); release binaries also claim macOS x86_64 (§12.8); upstream
ships core wheels only for Linux x86_64/aarch64 and macOS arm64/x86_64 (`UP/.github/workflows/build-core-wheels.yml:34-40,
123-133`), and the v11 package requires that wheel (`UP/sdk/python/pyproject.toml:21` `bittensor-core>=0.1.6,<0.2.0`), so v11 itself does not install on Windows. macOS runners have no docker.

**Fix.** Add §9.8 "Supported platforms": "Tier 1 (`pr-gates` and release binaries): `x86_64-unknown-linux-gnu`,
`aarch64-unknown-linux-gnu`, `aarch64-apple-darwin`. Tier 2 (release binary, not tested in CI):
`x86_64-apple-darwin`, cross-built on `macos-14` as upstream does. Neuron images: linux-gnu x86_64 and aarch64.
Unsupported: Windows (v11 ships no Windows core wheels), musl (untested)." §12.13: "CI runners (ubuntu-24.04 and
ubuntu-24.04-arm with docker; macos-14 without)".
