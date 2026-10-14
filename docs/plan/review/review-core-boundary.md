# Review: bittensor-core boundary and dependency mechanics

Scope: PLAN.md (full), architecture.md §1, §2, §6.1, Appendix D (focus), PLAN.md §5, §14.3, §15, draft-notes.md items in
this lens. All claims re-checked against `UP/` at `850f2755` (= `v477` for `sdk/`, `vendor/w3f-bls`, `support/macros`,
root `Cargo.toml`; `git diff v477 850f2755 -- <paths>` empty), the cargo checkouts (identical), `REG/` crates in
`~/.cargo/registry`, and three local probes in `scratchpad/review-work/` (no cargo run in upstream dirs).

Verified correct (no finding): `Runtime::parse` V14/V15/V16 handling (`CORE/runtime/mod.rs:163-198`), V14 `apis = []`;
`compose_call` signature and `Value` model (`codec/encode.rs:532`, `codec/value.rs:12`); `TxParams`,
`signature_payload_parts`/`signature_payload`/`encode_signed_extrinsic` (v4 only, `extrinsic.rs:260-273`), metadata-order
extension handling with fail-closed unknown non-zero-sized extensions (`:89-118`), era encoding and `era_birth`; every
§1.4 line number; keyfile formats and KDFs (`$NACL` argon2i13 SENSITIVE with fixed salt, Ansible, Fernet PBKDF2 10M,
`keyfiles/mod.rs:22-177`); keyfile JSON quirks (`:211-374`) use only non-host constructors; `host` gating
(`lib.rs:10-31`, `Cargo.toml:105-126`); `ledger` independent of `host`; timelock API and the reveal-round formula
(`timelock/mod.rs:104-195`), drand fetches host-only; `mlkem::seal`; digest; pow; both confirmed core defects
(`client.rs:32, 622-654` + `runtime/src/check_mortality.rs:19, 91-100`, also reached via `Executor::submit_shielded`,
`transaction.rs:1103-1115`; `epoch_schedule.rs:54` vs `run_coinbase.rs:1232` and cap `:412-416`); 22 constructors / 4
without catalog op; the ten `disallowed-methods` paths exist; w3f-bls double-source E0277 (`bench/nopatch`); tag-pin
plus replicated patch builds and `cargo update -p bittensor-core -p w3f-bls` re-unifies after a tag rewrite (probe); the
no-host core graph itself has no hard C build (only `cc` for `blake3`, which falls back to pure Rust,
`REG/blake3-1.8.7/build.rs:343-351`); upstream CI guards the no-host core (`.github/workflows/check-rust.yml:298`).

---

## CB-01 (blocker) architecture.md §2.7: no rustls crypto-provider decision; `host` builds panic on `wss://`

Location: §2.7 rows "WebSocket | `tokio-tungstenite 0.30` (`rustls-tls-native-roots`)" and "HTTP | … `reqwest 0.13.5`
(rustls)". No section chooses a TLS provider (`grep -n 'CryptoProvider\|install_default\|Connector::'` over PLAN.md and
architecture.md: no hits).

Problem: in every build with the core's `host` feature (owner profile, `wallet-encrypted`, `pow`, `full` = the `rtao`
CLI, the `core-oracle` lane) two rustls providers are unified: the core's blocking reqwest 0.12 enables `rustls/ring`,
rtao's reqwest 0.13 (`rustls`, also its default TLS) enables `rustls/aws-lc-rs`. With both, rustls cannot pick a process
default provider, and tokio-tungstenite's built-in rustls connector calls `ClientConfig::builder()`, which panics. The
CLI and owner tools therefore crash on their first `wss://` connect (finney's default endpoint). FakeNode, localnet,
e2e and interop lanes all use `ws://`, so no planned test sees it. In no-host builds the only provider is aws-lc-rs,
which brings `aws-lc-sys` (C, optionally CMake) into neuron images (see CB-04).

Evidence: `UP/sdk/bittensor-core/Cargo.toml:60` (reqwest 0.12 `rustls-tls-native-roots`, enabled by `host` :114-125);
`REG/reqwest-0.12.28/Cargo.toml:154` (`rustls-tls-native-roots` → `__rustls-ring`), :74-77 (`rustls?/ring`);
`REG/reqwest-0.13.5/Cargo.toml:119` (`default-tls = ["rustls"]`), :169 (`rustls` → `__rustls-aws-lc-rs`), :86-89
(`rustls?/aws-lc-rs`); `REG/rustls-0.23.45/src/crypto/mod.rs:243-253` (`expect("Could not automatically determine the
process-level CryptoProvider …")`), :259-286 (`from_crate_features()` is `None` when both `ring` and `aws_lc_rs` are on);
`REG/rustls-0.23.45/src/client/client_conn.rs:315` (`builder()` uses the process default);
`REG/tokio-tungstenite-0.28.0/src/tls.rs:126` (default connector = `ClientConfig::builder()`, same in 0.26.2),
`Cargo.toml:157-160, 179-182` (rustls and tokio-rustls without default features, so no provider of its own).

Fix (add a row to §2.7 and amend two):
"| TLS | `rustls 0.23`, **ring only**: `[workspace.dependencies] rustls = { version = "0.23", default-features = false,
features = ["ring", "std", "tls12", "logging"] }` | One provider for the whole graph: the core's `host` already turns on
`rustls/ring` (blocking reqwest 0.12); reqwest 0.13's default TLS would add `aws-lc-rs`, after which rustls has no
default provider and tokio-tungstenite's built-in connector panics (`REG/rustls-0.23.45/src/crypto/mod.rs:243-286`,
`REG/tokio-tungstenite-0.28.0/src/tls.rs:126`). rtao never relies on the process default: `rtao-rpc` builds
`ClientConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))` with native roots and
connects with `connect_async_tls_with_config(request, ws_config, false, Some(Connector::Rustls(tls_config)))`; every reqwest client uses
`ClientBuilder::tls_backend_preconfigured(tls_config)`. `aws-lc-rs`, `aws-lc-sys`, `native-tls` are banned in `deny.toml` and
`deps-check`. Test: `rtao-rpc` connects to a loopback `wss://` FakeNode (self-signed cert) both in an `--all-features`
build and in the no-host profile. |"
WebSocket row: "`tokio-tungstenite 0.30` (`rustls-tls-native-roots`; connector supplied by rtao, see TLS)". HTTP row:
"`reqwest 0.13.5` (`default-features = false`, features `rustls-no-provider`, `json`, `http2`, `charset`; preconfigured
ring `ClientConfig`)".

## CB-02 (major) architecture.md §1.6: `staged --check` deletion trigger is unsound for G4 and feature-blind for G9

Location: §1.6 intro ("has the deletion trigger "pinned core exports `<symbol>`" … fails when one exists"); same rule
in PLAN.md §5.3 and the §10.6/§12.9 playbook row "a staged PR ships in `core_tag` | `staged --check` fails | flip the
`pub use`, delete the staged module".

Problem: (1) G4: the staged `epoch_schedule` re-exports `EpochScheduleState`, `predict_first_reveal_block`,
`should_run_epoch` (§1.1 code block). These symbols already exist in the pinned core, without `host`, with the buggy
predicate. A symbol-existence probe fires on the day the module lands (M2); following the playbook deletes the fix and
re-adopts confirmed defect #2. (2) G9: `keyfile_json`'s target symbols exist today behind `host`; the core's
`default = ["host"]`, and CI runs everything `--all-features`, so a naive probe fires immediately and deleting the module
breaks the no-host profile. The trigger must be per module, feature-aware, and behavioral where upstream fixes an
existing function.

Evidence: `CORE/timelock/epoch_schedule.rs:17, 45, 108` (public today), `:54` (`> MAX_TEMPO_U64`); `CORE/lib.rs:15-16`
(`keyfiles` host-gated), `CORE/keyfiles/mod.rs:41, 179, 287`; `UP/sdk/bittensor-core/Cargo.toml:113`
(`default = ["host"]`); Appendix E `check` job (`--all-features`); architecture §12.9 playbook row.

Fix (replace the §1.6 intro paragraph; mirror in PLAN.md §5.3 and the playbook row):
"Each `rtao-core-ext` module starts with three machine-read lines: `//! UPSTREAM-TARGET: <path> (PR-Ux)`,
`//! UPSTREAM-PROBE: symbol <path>` or `//! UPSTREAM-PROBE: vectors <suite>`, and `//! UPSTREAM-FEATURES: <core
features>` (default none). `cargo xtask staged --check` compiles each probe in a scratch crate that depends on the
pinned core with `default-features = false` plus exactly the listed features (the no-host profile rtao ships) and fails
only when the probe passes. A `symbol` probe names a path that does not exist at the current `core_tag`; a `vectors`
probe runs the module's own test vectors against the core's implementation. Per module: G1-G3 `symbol` at the ungated
targets of Appendix D; G4 `vectors` (the skip and tempo-reset safety-net vectors run against
`bittensor_core::timelock::epoch_schedule::{should_run_epoch, predict_first_reveal_block}`, which already exist with
the `MAX_TEMPO_U64` predicate); G5 `symbol timelock::generate_commit_v2_at`; G8 `symbol runtime::Runtime::metadata_ir`;
G9 `symbol keyfiles::deserialize_keypair_from_keyfile_data` with `UPSTREAM-FEATURES: keyfiles-json` (it exists today
only under `host`); G10 `symbol keyfiles::encode_password_for_environment` (features `host`); G11 `symbol
keys::verify_strict`; G12 `symbol codec::value::Value::field`. A probe that compiles only with `host` for a module the
no-host profile uses is reported as "upstream but host-gated" and does not trigger deletion."
Playbook row: "`staged --check` fails because the module's probe (symbol at the ungated target, or the vector suite)
now passes | 1 h: flip the `pub use`, delete the staged module".

## CB-03 (major) architecture.md Appendix D: PR-U1, PR-U2, PR-U7, PR-U8 target host-gated modules

Location: Appendix D rows PR-U1 (`sdk/bittensor-core/src/client/ops.rs`), PR-U2 (`client::ops::receipt`), PR-U7
(`client.rs` `submit_shielded`, `MAX_SHIELD_ERA_PERIOD`), PR-U8 (`transaction.rs` public `PolicySubject`/`PolicyInput`,
"the equivalence test becomes direct reuse"); same targets in PLAN.md §15.

Problem: `client` and `transaction` are compiled only with `host`. The staged `ops::{metadata, storage, runtime_api,
extrinsic_lookup, dispatch_error, fee, receipt}` are used by the no-host validator/miner (submission pipeline, receipts,
fee probe in `rtao-client`, M2). If the PRs land as written, the promised one-line `pub use` flip ("no other crate
changes", §1.1) compiles only with `host`, so it either breaks the no-host profile or the staged copies can never be
deleted. The same holds for PR-U8's "direct reuse" of `Policy` in `rtao-tx`.

Evidence: `CORE/lib.rs:10-11` (`#[cfg(feature = "host")] pub mod client;`), `:24-25` (`transaction`); §3.9.4, §3.10
(staged ops on the no-host submission path); §14 M2 row.

Fix (Appendix D, and PLAN.md §15):
- PR-U1 target: "`sdk/bittensor-core/src/protocol/{metadata,storage,runtime_api,extrinsic_lookup,dispatch_error,fee}.rs`,
  a new module declared in `lib.rs` without `#[cfg(feature = "host")]`; the blocking `client.rs` calls into it".
- PR-U2 target: "`sdk/bittensor-core/src/protocol/receipt.rs` (ungated)".
- PR-U7: "`Client::submit_shielded` (and `Executor::submit_shielded`, `transaction.rs:1103-1115`, which delegates to it)
  sign carrier and inner with `protocol::shield::MAX_SHIELD_ERA_PERIOD` (ungated) and resolve the inner receipt".
- PR-U8: "move `Policy`, `Spend` and new public `PolicySubject`/`PolicyInput` from host-gated `transaction.rs` to an
  ungated `policy.rs`, re-exported from `transaction`".
- Add under the table: "A PR whose symbols land only behind `host` does not retire a staged module used by the no-host
  profile; `staged --check` probes without `host` (§1.6)."

## CB-04 (major) PLAN.md: "no C compiler / no C toolchain" claims and the M2 exit criterion are unattainable

Location: §1.4 table ("weight-setting validator, no C toolchain"), §1.5 #5 ("their images need no libsodium, OpenSSL,
Perl or C compiler"), §6.1 #5, §7.1 comments ("no C toolchain"), §7.5 ("neuron images built with `-p <binary>` need no
C toolchain"), §5.7 CI bullet, §12.4 M2 exit ("the no-host validator image builds without a C toolchain"), §17 glossary.

Problem: neuron profiles need TLS (`wss://` endpoints; `neuron` → `btauth-client` → reqwest), and every rustls provider
compiles C: `ring` builds C sources through `cc`; `aws-lc-sys` (reqwest 0.13's default) needs a C compiler and may need
CMake. rustc on Linux also uses `cc` as its linker driver. The M2 exit criterion cannot pass as worded. Meanwhile the
`no-host` ban list misses the C crates that matter (`secp256k1-sys`, which returns whenever anything enables
`sp-core/std`; `libsodium-sys`; `aws-lc-sys`; `cmake`; `native-tls`), and CI builds `validator,miner` (§5.7) or
`validator,miner,prometheus` (Appendix E) instead of the documented profiles (`validator`+`prometheus`, `miner`+`redis`).

Evidence: `REG/ring-0.17.14/build.rs:55-67` (C files), `Cargo.toml:252-254` (build-dep `cc`);
`REG/aws-lc-sys-0.40.0/Cargo.toml` build-deps `cc`, `cmake`, `builder/main.rs:534-576`; `REG/rustls-0.23.45/Cargo.toml:70`
(`default` includes `aws_lc_rs`); `REG/reqwest-0.13.5/Cargo.toml:119, 169`; `UP/sdk/bittensor-core/Cargo.toml:45-48, 124`
and `~/.cargo/git/checkouts/polkadot-sdk-778d4d407c7beeae/cacb431/substrate/primitives/core/Cargo.toml:66-67, 117-118`
(`sp-core/std` → C `secp256k1`); `SP/design-tagpin/Cargo.lock` (core no-host graph: only `cc` via `blake3`).

Fix (replacement text):
- §1.4 row label: "weight-setting validator, no Perl/OpenSSL/libsodium".
- §1.5 #5 and §6.1 #5: "Neuron binaries build without `bittensor-core/host`, so their images need no libsodium,
  OpenSSL, `secp256k1-sys`, Perl or CMake; the only native code is `ring` (TLS), built by the C compiler rustc already
  uses as its linker on Linux."
- §7.1: "# validator/Cargo.toml: hotkey only; no Perl, OpenSSL or libsodium" (same for the miner).
- §7.5: "neuron images built with `-p <binary>` build in `rust:1.89-slim` as shipped (gcc only); the owner image adds
  `perl` and `make`".
- §12.4 M2 exit: "the no-host validator image builds FROM `rust:1.89-slim-bookworm` without installing perl, make,
  cmake, pkg-config, libssl-dev or libsodium-dev".
- §17: "no-host profile | neuron builds without `bittensor-core/host`: no libsodium, OpenSSL, secp256k1-sys, Perl or
  CMake; `ring` is the only C component".
- §5.7 CI bullet: "The CI `no-host` job builds `-p rtao --no-default-features --features validator,prometheus` and
  `--features miner,redis` and asserts with `cargo tree -e normal` that none of `sodiumoxide`, `libsodium-sys`,
  `openssl`, `openssl-sys`, `openssl-src`, `native-tls`, `fernet`, `ansible-vault`, `secp256k1-sys`, `aws-lc-rs`,
  `aws-lc-sys`, `cmake`, `hidapi`, `libloading`, `rayon` is present, that `reqwest` lacks the `blocking` feature, and
  that `sp-core` is built without `std` (`cargo tree -e features -i sp-core`)."

## CB-05 (major) PLAN.md §14.3 #1: `arbitrary_precision` default is wrong; decide it

Location: §14.3 row 1 default "pin with a test; custom `Deserialize` …; compare numbers as f64; document; ask upstream
whether the feature can move behind a corpus/binding feature"; §13 R19 "compare oracle JSON numerically"; §5.8 #10.

Problem: (1) rtao itself depends on the feature: `DynValue` `Display` and the CLI/agent JSON go through the core's
`to_corpus_json` (architecture §1.5; PLAN §5.1), which renders u128 above u64::MAX, i128 outside i64/u64 and U256 via
`Number::from_u128`/`from_i128`/`parse`. Without the feature these return `None` and the core substitutes `0`
silently (U256 parses lossily). Every U64F64 value ≥ 1.0 has bits ≥ 2^64. Asking upstream to move the feature behind an
optional flag would zero rtao's JSON. (2) "compare numbers as f64" is lossy above 2^53 = 9,007,199,254,740,992: any rao
amount above ~9.007M TAO (total issuance is ~1e16 rao) and every fixed-point bit pattern; the read oracle would accept
wrong integers.

Evidence: `CORE/codec/value.rs:58-77` (`from_i128`/`from_u128` `unwrap_or_else(|| Number::from(0))`; comment "serde_json's
arbitrary_precision feature lets the comparison carry it digit-exact"); `REG/serde_json-1.0.154/src/number.rs:264-279`
(`from_u128` is `None` above u64 without the feature); `UP/sdk/bittensor-core/Cargo.toml:40-42`; architecture
§4.8 ("parsed f64 values compare exactly").

Fix (replace row 1's "Resolve in"/"Default" with a decision; update R19 and §5.8 #10 to match):
"Decided (M0): keep `arbitrary_precision` and depend on it directly: `[workspace.dependencies] serde_json = { version =
"1.0.132", features = ["arbitrary_precision"] }`, used by `rtao-core`, so `DynValue` JSON stays digit-exact whatever
upstream does. Rules: (a) no `#[serde(untagged)]`/`#[serde(flatten)]` over numeric fields in rtao input types;
`WeightsArg`, `Amount<U>` and `Ratio` deserialize through `serde_json::Value` and convert from `Number::as_str()` (the
exact decimal text, available under the feature, which also gives `Ratio` its exact-decimal input); (b) the oracle
comparator walks both trees and compares integers exactly (normalized decimal strings) and only non-integers as f64,
never `serde_json::Value ==`; (c) an M0 test pins the measured behavior plus a u128/U256 `to_corpus_json` round trip;
(d) the template documents that request types must avoid untagged/flatten over floats or go through
`serde_json::Value`; (e) upstream: ask that the feature stay a documented contract of `to_corpus_json`."

## CB-06 (major) architecture.md §1.2: the oracle is not built with the same third-party versions as rtao

Location: §1.2 last paragraph ("The oracle (§11.5) builds `bittensor-core-py` from `parity.tag`. That is byte-identical
to `core.tag`'s core whenever the fingerprints match … So the oracle stays a valid specification for rtao's build");
PLAN.md §5.5 last bullet.

Problem: equal core sources are not equal binaries. The oracle's PyO3 module is built with the monorepo's `Cargo.lock`;
rtao resolves its own versions. Today they differ for crates that define bytes or JSON, and Renovate (§12.4) widens the
gap weekly.

Evidence: `UP/Cargo.toml:27-38` (`sdk/bittensor-core-py` is a workspace member at :35, so maturin uses `UP/Cargo.lock`); version
diff of `UP/Cargo.lock` against the plan's own v477 tag-pin lock `SP/design-tagpin/Cargo.lock`: ml-kem 0.2.2 vs 0.2.3,
bip39 2.2.0 vs 2.2.2, ed25519-zebra 4.1.0 vs 4.2.0, frame-metadata 23.0.0 vs 23.0.1, twox-hash 2.1.2 vs 2.1.5,
blake2b_simd 1.0.3 vs 1.0.5, sha3 0.10.8 vs 0.10.9, serde_json 1.0.145 vs 1.0.154, scale-decode 0.16.0 vs 0.16.2,
bitvec 1.0.1 vs 1.1.1, uint 0.10.0 vs 0.10.2.

Fix (replace the paragraph; mirror in PLAN.md §5.5):
"Equal core sources do not make equal binaries: the oracle's `bittensor-core-py` is built with `UP/Cargo.lock`
(`sdk/bittensor-core-py` is a workspace member), rtao with its own lock. `xtask pin` therefore aligns rtao's
`Cargo.lock` with `UP/Cargo.lock` at `parity.commit` for every crates.io package in bittensor-core's normal-dependency
closure (`cargo update -p <name>@<cur> --precise <mono>`); Renovate excludes that set; `deps-check` rule 7 fails on any
difference not listed with a reason in `parity/lock-exceptions.toml`. rtao's own requirements on shared crates use the
monorepo floor (`scale-decode = "0.16"`, `frame-metadata = "23.0"`). Subnet workspaces resolve their own versions;
`rtao doctor` warns when theirs differ from the set recorded in `compat.json`."

## CB-07 (minor) architecture.md §1.7, §2.3 rule 5, Appendix E, §15.1 R4, §0.2, §2.6, §11.x, §14, §16: stale no-host text

Location: §1.7 ("libsodium, vendored OpenSSL …, `secp256k1-sys` and hidapi C builds, 330 crates instead of 207";
CI bullet), §2.3 rule 5, Appendix E `no-host` job comment, §15.1 R4 ("… secp256k1-sys, hidapi"), §0.2 ("Neuron binaries
build without the C toolchain"), §2.6 manifest comments, §9.11 Dockerfile bullet (lines 3096-3097), §14 M2 exit, §16 #5.

Problem: draft-notes Plan-step #4 and #5 are not applied: hidapi is gated by `ledger`, not `host`; the three ban lists
differ (§1.7 has `openssl-src` and blocking `reqwest` but not `libloading`; rule 5 and Appendix E the reverse); and
the "no C toolchain" wording has the same defect as CB-04.

Evidence: `UP/sdk/bittensor-core/Cargo.toml:86-94, 113-126`; architecture lines 384-386, 404-406, 578-579, 4124, 3755,
119, 658, 678, 3096, 3721, 3815; draft-notes.md "Plan step" #4, #5.

Fix: §1.7 first sentence: "Every proposal enables `bittensor-core/host` for neuron binaries. That means libsodium,
vendored OpenSSL (which needs Perl `IPC::Cmd` and `make`) and `secp256k1-sys` (through `sp-core/std`), 330 crates
instead of 207. (hidapi is gated by the separate `ledger` feature and uses the pure-Rust hidraw backend on Linux.)"
Use the CB-04 ban list verbatim in §1.7, §2.3 rule 5 and the Appendix E job (which builds `validator,prometheus` and
`miner,redis`). R4: "(libsodium, vendored OpenSSL needing Perl, secp256k1-sys)". §0.2, §2.6, §9.11, §14 M2, §16 #5:
replace "without the C toolchain"/"no C toolchain" with "without Perl, OpenSSL, libsodium or CMake (ring is the only C
component)".

## CB-08 (minor) PLAN.md §14.3 #11: decide the host-gated `disallowed-methods` question (measured)

Location: §14.3 row 11 ("recent clippy versions warn about unresolvable configured paths [I] … check a no-host clippy
run"); architecture §1.8 `clippy.toml`.

Problem: left open, and there is a silent failure mode the plan does not cover: a configured path that does not resolve
only warns, and `-D warnings` does not turn it into an error. A rename upstream therefore disables the ban silently,
even in the `--all-features` run.

Evidence: probe `scratchpad/review-work/clippy-probe` (workspace with `dep` gating `client::Client::connect` behind
`host`): on clippy 1.89.0 and 1.99.0, `cargo clippy -p app -- -D warnings` without the feature prints "warning:
`dep::client::Client::connect` does not refer to a reachable function … help: add `allow-invalid = true`" and exits 0;
with `allow-invalid = true` the warning disappears on both; a crate with no path to `dep` gets no warning.

Fix (replace row 11 with a decision; edit architecture §1.8): "Decided (measured on clippy 1.89 and 1.99): add
`allow-invalid = true` to the seven host-gated entries (`Client::connect`, `Executor::new`, `Executor::with_policy`,
`timelock::{decrypt, get_round_info, get_reveal_round_signature}`, `keyfiles::save_password_to_environment`); the
enforcing run stays `cargo clippy --workspace --all-targets --all-features -- -D warnings`; `xtask core-surface` emits,
for each of the ten entries, `#[allow(clippy::disallowed_methods)] const _: <signature> = <path>;` (host-gated ones
under `#[cfg(feature = "host")]`) into `crates/rtao-core/tests/core_surface.rs`, so a renamed or removed path fails the
build instead of silently disabling the ban."

## CB-09 (minor) architecture.md §10.4 and §1.2 step 3: `rtao doctor` uses the wrong expected tag; patch line not propagated

Location: §10.4 ("exactly one `w3f-bls`, at the same tag and commit as `bittensor-core`, and that tag equal to rtao's
`BITTENSOR_CORE_TAG`"); §1.2 bump step 3 ("rewrite both `tag =` lines in `Cargo.toml`"); PLAN.md §5.6 items 2-3.

Problem: (a) a doctor binary from release X run on a workspace that uses rtao release Y with another `core_tag` prints a
patch line at X's tag, which locks a second `w3f-bls` (E0277). (b) The patch must reproduce the core's git reference
exactly: a `rev` of the very same commit is a different Cargo source and locks a second copy. (c) The bump rewrites only
the root `Cargo.toml`, but the line also lives in `templates/subnet/` and the README/book quickstart, so a core bump
ships a template that fails with E0277.

Evidence: probe `scratchpad/review-work/refmismatch`: core at `rev = "850f2755…"` plus patch at `tag = "v477"` (same
`sdk/` tree) → `cargo generate-lockfile --offline` locks two `w3f-bls 0.1.3` (`?tag=v477#5c6e83e2` and
`?rev=850f2755…`); rewriting both to the same `rev` and running `cargo update -p bittensor-core -p w3f-bls` leaves one;
two copies give E0277 (`SP/design-bench/target-nopatch/.../output-lib-bittensor_core`).

Fix: §10.4 bullet: "`Cargo.lock` of a downstream workspace: read the expected reference from the workspace's own
`bittensor-core` entry (`source = "git+https://github.com/RaoFoundation/subtensor.git?tag=<T>#<sha>"`); require
exactly one `w3f-bls` with the identical `source` (same reference kind; a `rev` of the same commit is a different
source); print `w3f-bls = { git = "https://github.com/RaoFoundation/subtensor.git", tag = "<T>" }`; warn, not fail,
when `<T>` differs from the doctor's own `BITTENSOR_CORE_TAG`." §1.2 step 3: "… rewrite every reference generated from
`[core]` (root `Cargo.toml`, `templates/subnet/Cargo.toml`, README and book quickstart snippets); a test asserts that
every `w3f-bls = { git = …, tag = "…" }` in the repository equals `[core].tag`."

## CB-10 (minor) architecture.md §1.2 "Core-input fingerprint": misses a path dependency, mislists `codec`, over-includes patches

Location: §1.2 items 1-4 and "Anything else in the monorepo … cannot change what Cargo builds for rtao"; PLAN.md §5.5,
§17 glossary.

Problem: (a) the core's path dependency `subtensor-macros` (`support/macros`, built for rtao) and its inherited
workspace entries are not hashed; (b) `codec` is named as inherited, but the core declares it directly; `scale-info` and
`subtensor-macros` are the inherited ones; (c) the whole root `[patch.crates-io]` table is hashed although downstream
Cargo ignores it, and its `zstd-sys`/`zstd-safe` entries (not in the core's graph) would move `core_tag` and force a
downstream patch edit with no build change.

Evidence: `UP/sdk/bittensor-core/Cargo.toml:55` (codec direct), :79-80 (`scale-info`, `subtensor-macros` with
`workspace = true`); `UP/Cargo.toml:74` (`subtensor-macros = { path = "support/macros" }`), :334-337;
`UP/support/macros/Cargo.toml` (inherits `syn`, `quote`, `proc-macro2`, `ahash`, `edition.workspace`); probe
`cargo update` output "Adding subtensor-macros v0.1.0 (https://github.com/RaoFoundation/subtensor.git?rev=…)";
`SP/design-tagpin/Cargo.lock` has no zstd.

Fix (items 1-4): "1. for each package in the path-dependency closure of `sdk/bittensor-core/Cargo.toml` (today
`sdk/bittensor-core`, `support/macros`, `vendor/w3f-bls`), the tree SHA of `src/` and the blob SHAs of `Cargo.toml`
(and `build.rs`), excluding `tests/`, `benches/`, `fixtures/`; 2. the TOML-normalized `[workspace.dependencies]`
entries those packages inherit (today `sp-core`, `tle`, `w3f-bls`, `frame-metadata`, `scale-info`, `subtensor-macros`,
`syn`, `quote`, `proc-macro2`, `ahash`) and the `[workspace.package]` keys they inherit (`edition`); 3. the root
`[patch.crates-io]` entries for crates in the core's resolved graph only (today `w3f-bls`). The closure is derived from
the manifests, so a new path dependency is picked up automatically." Next sentence: "Everything else (pallets,
runtime, Python, docs, patches for crates outside the core's graph) cannot change what Cargo builds for rtao."

## CB-11 (minor) PLAN.md §5.8 #2: the epoch defect is triggered by skips and tempo resets, not by deferral

Location: §5.8 #2 ("After a deferred or skipped slot the core predicts the first reveal one epoch late"); same phrase in
§6.1 #20, §15 PR-U5, architecture §9.3 step 8 and §16 #20.

Problem: deferral sets `PendingEpochAt`, which both predicates honor, so it does not diverge (and §9.3 step 9 calls
deferral "the safe direction", a contradiction). The divergence needs `BlocksSinceLastStep > tempo` while
`LastEpochBlock` is recent: after an `EpochSkipped` slot (`LastEpochBlock` advances, counter not reset), and after
`sudo_set_tempo` lowers the tempo below the counter (`LastEpochBlock` reset only). The safety-net vectors must cover
exactly these two cases.

Evidence: `UP/pallets/subtensor/src/coinbase/run_coinbase.rs:422-431` (defer → `PendingEpochAt`), :434-438 (reset only
on a consistent run), :472-483 (skip still advances `LastEpochBlock`), :1228-1233; `CORE/timelock/epoch_schedule.rs:50-56`;
`UP/pallets/subtensor/src/utils/misc.rs:119-125` (`apply_tempo_with_cycle_reset`).

Fix: "After an `EpochSkipped` slot (`run_coinbase.rs:472-483`: `LastEpochBlock` advances but `BlocksSinceLastStep` is
not reset) or after `sudo_set_tempo` lowers the tempo below the counter (`utils/misc.rs:119-125`), the pallet fires the
safety net on the next block while the core predicts the next epoch a full tempo later; the first reveal is predicted
one epoch late … Deferral sets `PendingEpochAt`, which both predicates honor, and does not diverge." Add to §12.4
(M2): "safety-net vectors: skip at slot b ⇒ epoch at b+1; tempo lowered below the counter ⇒ epoch next block".

## CB-12 (minor) architecture.md §11.4 scenario 16 / §1.3 item 4 / Appendix D PR-U4: the evidence build is not buildable as described

Location: §11.4 #16 ("a build of a timelock commit with crates.io `w3f-bls` (std)"), §1.3 item 4, Appendix D PR-U4,
§15.2 Q8.

Problem: inside rtao's workspace the root patch redirects every crates.io `w3f-bls` to the fork, and a patch-free build
of the core fails with E0277, so "a crates.io-w3f-bls build" needs its own workspace. The evidence also omits the source
diff, which already predicts the answer: crates.io 0.1.3 and `vendor/w3f-bls` differ in four files; the only behavioral
change is `SecretKey::check`, which neither `tle` nor the core's timelock reaches.

Evidence: `diff -r REG/w3f-bls-0.1.3/src UP/vendor/w3f-bls/src`: `double_pop.rs` (no-std rewrite; same `BLS_POP_` tag
and seed order), `lib.rs` (serde extern removed), `verifiers.rs:271` (ownership only), `single.rs:384` (`key[1], key[2]`
→ `key[0], key[1]`); `grep -rn SecretKey` in `tle/src` and `CORE/timelock`: no w3f-bls `SecretKey` use;
`tle/Cargo.toml` `w3f-bls = { version = "=0.1.3" }`; `UP/vendor/w3f-bls/Cargo.toml:1-6` understates the delta.

Fix: scenario 16: "**PR-U4 evidence**: `ci/pr-u4-probe/`, a separate Cargo workspace with no `[patch]` and its own lock,
depends on `tle` at the monorepo pin (`ideal-lab5/timelock` rev `5416406c`, `features = ["std"]`) and crates.io
`w3f-bls = "=0.1.3"` (std), reproduces `WeightsTlockPayload` and `encrypt_and_compress` (`CORE/timelock/mod.rs:53-58,
104-140`) and writes commits for chosen past and future rounds to `vendor/recorded/pr-u4/`. (a) Offline, rtao's
workspace decrypts the past-round commits with the core's vendored path (`decrypt_with_signature`) and committed
quicknet signatures; (b) on localnet, pallet-drand reveals the future-round commits." Add to §1.3 item 4 the four-file
diff and "neither `tle` nor `CORE/timelock` uses `SecretKey`, so identical ciphertexts are expected; the probe confirms".

## CB-13 (minor) architecture.md §2.6: owner-tool manifest lacks the mandatory patch line

Location: §2.6 third manifest (`# owner-tool/Cargo.toml — coldkey operations; builds the core with host`); draft-notes
Plan-step #14 (fixed in PLAN.md §7.1, not here).

Evidence: architecture lines 689-692 (only `[dependencies]`); §1.3 ("every final workspace must repeat this");
`bench/nopatch` E0277.

Fix: append to that manifest
`[patch.crates-io]` /
`w3f-bls = { git = "https://github.com/RaoFoundation/subtensor.git", tag = "v477" }   # = rtao::core::BITTENSOR_CORE_TAG`
and after the block: "In a workspace holding all three crates the line appears once, in the workspace root."

## CB-14 (minor) architecture.md §1.6 G6: the staged `shield::MAX_SHIELD_ERA_PERIOD` is a hand copy of a runtime constant

Location: §1.6 G6 module cell, §2.2 and PLAN.md §4.3 `rtao-core-ext` items, §3.11 `ShieldPeriod { P4, P8 }`.

Problem: three unlinked sources for one bound: a staged constant, `ShieldPeriod`, and the extracted Python
`MEV_SHIELD_ERA_PERIOD` ("shield bounds", §4.1 "No Python constant is hand-copied"). A runtime change to the bound would
trip no gate, and rtao never calls `submit_shielded`, so the staged constant has no consumer.

Evidence: `UP/runtime/src/check_mortality.rs:19` (private runtime const, not in metadata), `PY/settings.py:29`;
architecture line 1563; §4.9 chain-source layer list (no entry); map.toml already watches `check_mortality.rs`.

Fix: G6 cell: "none (rtao never calls `Client::submit_shielded`). The bound is extracted: the chain-source layer reads
`MAX_SHIELD_ERA_PERIOD` from `runtime/src/check_mortality.rs:19` with `syn` into `rtao_catalog::rt`, and a test
asserts it equals `py::MEV_SHIELD_ERA_PERIOD` and `ShieldPeriod::MAX.blocks()`." Remove `shield::MAX_SHIELD_ERA_PERIOD`
from §2.2 and PLAN.md §4.3; add the item to the §4.9 chain-source layer.

## CB-15 (minor) architecture.md §1.1: `rtao-core-ext` dependency list is incomplete and one entry duplicates a core crate

Location: §1.1 table, `rtao-core-ext` "May depend on" (`bittensor-core`, `serde`, `serde_json`, `parity-scale-codec`,
`hex`, `sp-crypto-hashing`).

Problem: staged modules need crates the list omits: `scale-info` (the IR walks `Runtime.types: PortableRegistry`; the
core does not re-export it), `frame-metadata` (runtime-API versions and V16 data for the IR), `schnorrkel` and
`ed25519-zebra` (`verify_strict`), `subtensor-macros` (`#[freeze_struct]`, which the upstream lint requires on SCALE
structs). Meanwhile `sp-crypto-hashing` from crates.io would be a second copy beside the fork's git copy, while the
core already exports the hashers.

Evidence: `CORE/runtime/mod.rs:145` (`pub types: PortableRegistry`); `CORE/codec/storage.rs:21-43` (`hash_param` incl. `Blake2_256`, `Twox128`);
`CORE/mlkem/mod.rs:37`; `SP/design-tagpin/Cargo.lock` (`sp-crypto-hashing` from `RaoFoundation/polkadot-sdk.git?rev=
cacb4310`); `UP/build.rs:72` (`RequireFreezeStruct` runs on every file); `UP/sdk/bittensor-core/Cargo.toml:37` (schnorrkel 0.11.5).

Fix: "`bittensor-core` (no default features); `serde`, `serde_json`, `parity-scale-codec`, `hex`; `scale-info`,
`frame-metadata` (IR); `schnorrkel`, `ed25519-zebra` (`verify_strict`); `subtensor-macros` (git, `core_tag`) — each
at the version and source already in the core's graph (`deps-check` asserts no second copy); hashing only through
`bittensor_core::codec::storage::hash_param` and `mlkem::twox_128`; **no tokio**".

## CB-16 (minor) architecture.md §4.9: `map.toml` does not watch the PyO3 binding

Location: §4.9 `map.toml` "Watched roots".

Problem: Python reaches the core only through `sdk/bittensor-core-py` (1,865 lines of glue: argument mapping, `Value`
materialization, the metadata IR). A binding-only change alters Python behavior while no watched file and no core
input changes; it changed in v473→v475.

Evidence: `PY/intents/weights.py:308` → `UP/sdk/bittensor-core-py/src/timelock.rs:61`; `src/values.rs:1-3` ("Value <->
Python materialization"); `git diff --name-only v473 v475 -- sdk/bittensor-core-py` → `pyproject.toml`, `src/lib.rs`,
`src/pow.rs`.

Fix: add `sdk/bittensor-core-py/**` to the watched roots, kind `oracle-only` (rows for `values.rs`, `runtime.rs`,
`timelock.rs`, `keys.rs` name the rtao code that mirrors their shapes), so a binding change appears in the porting
report.

## CB-17 (minor) architecture.md §1.5: "lazily decoded V16 view" contradicts §3.4 and §17

Location: §1.5 `Runtime` row ("parsed V15 plus a lazily decoded V16 view for tooling").

Evidence: §3.4 ("V16 is never parsed at run time. `xtask` reads it with `frame-metadata` 23"); PLAN.md §16 non-goal
"parsing V16 at run time"; `CORE/runtime/mod.rs:193-198`.

Fix: "`rtao_client::RuntimeHandle(Arc<RuntimeEntry>)`: parsed V15, `MetadataLevel`, the surface report, the shape
cache, name→index caches (V16 is decoded only by `xtask`, §3.4)".

## CB-18 (minor) architecture.md §2.8: build weight ignores the cold git fetch of the pinned sources

Location: §2.8 (and PLAN.md §2 "Highly performant": "builds in 12.9 s"); §2.6/§9.11 Dockerfile text.

Problem: 12.9 s is a warm build. A cold build (a neuron Docker image, subnet CI without cache) first clones the full
history of the monorepo and of the polkadot-sdk fork.

Evidence: `du -sh ~/.cargo/git/db/*` after the v477 builds: subtensor 293M, polkadot-sdk 480M, timelock 956K;
`bench/tagpin/tagpin.log` ("Updating git repository" for all three).

Fix: add to §2.8: "Cold fetch: about 293 MB (`RaoFoundation/subtensor`) and 480 MB (`RaoFoundation/polkadot-sdk`) of git
objects (measured). The template Dockerfile builds with `RUN --mount=type=cache,target=/usr/local/cargo/git
--mount=type=cache,target=/usr/local/cargo/registry cargo build --release --locked -p <binary>`, and the template CI
caches `~/.cargo/git/db`."
