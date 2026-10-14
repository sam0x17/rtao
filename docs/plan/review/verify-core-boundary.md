# Skeptic verification: core-boundary findings (CB-01 … CB-18)

Every finding was re-checked against the plan text and the cited evidence. Read-only commands were run against `UP/`,
`REG/` (`~/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/`) and the cargo git checkouts. Four probes were built
in my own scratch dirs, never in upstream dirs:

- `scratchpad/verify-cb/tlspanic`: rustls 0.23 with ring + aws-lc-rs.
- `scratchpad/verify-cb/clippy-probe`: copy of the reviewer's probe, run on clippy 0.1.89 and 0.1.99.
- `scratchpad/verify-cb/refmismatch`: tag vs rev for the same commit.
- `scratchpad/verify-cb/tt030`: `cargo fetch` of tokio-tungstenite 0.30.0, the version the plan pins.

| id | reviewer severity | verdict | notes |
|---|---|---|---|
| CB-01 | blocker | partially_confirmed | panic reproduced; the fix needs a process-default backstop and an owned `ClientConfig` for reqwest |
| CB-02 | major | partially_confirmed | the probe crate needs the w3f-bls patch and error classification, or the gate goes inert |
| CB-03 | major | partially_confirmed | PR-U1 and PR-U2 are real; PR-U7 and PR-U8 retire nothing on the no-host path |
| CB-04 | major | partially_confirmed | "ring is the only C" is false (blake3 SIMD); `cargo tree -e normal` cannot see `cmake` or `openssl-src` |
| CB-05 | major | confirmed | |
| CB-06 | major | partially_confirmed (minor) | lock alignment over the whole closure is infeasible; the oracle already runs on every PR |
| CB-07 | minor | partially_confirmed | inherits the CB-04 corrections |
| CB-08 | minor | confirmed | re-measured on 0.1.89 and 0.1.99 |
| CB-09 | minor | confirmed | same-commit tag vs rev re-measured: two sources |
| CB-10 | minor | confirmed | |
| CB-11 | minor | partially_confirmed | "next block" holds only for scheduled-slot skips; vectors made precise |
| CB-12 | minor | partially_confirmed | `encrypt_and_compress` is randomized (OsRng); "identical ciphertexts" needs a fixed esk and seed |
| CB-13 | minor | confirmed | |
| CB-14 | minor | partially_confirmed | `map.toml` already watches `check_mortality.rs`; the equality test conflates Python's default with the chain max |
| CB-15 | minor | confirmed | |
| CB-16 | minor | confirmed | |
| CB-17 | minor | confirmed | |
| CB-18 | minor | confirmed | |

---

## CB-01: partially_confirmed (problem fully confirmed and reproduced)

### Checked

**Core side, ring.** `UP/sdk/bittensor-core/Cargo.toml:60` declares reqwest 0.12 with `rustls-tls-native-roots`, and
`host` enables it at `:114-125`. In `REG/reqwest-0.12.28/Cargo.toml`, `rustls-tls-native-roots` enables
`__rustls-ring`, which enables `rustls?/ring`.

**rtao side, aws-lc-rs.** In `REG/reqwest-0.13.5/Cargo.toml`, `default-tls = ["rustls"]` and `rustls` enables
`__rustls-aws-lc-rs`, which enables `rustls?/aws-lc-rs`. In rustls, `aws-lc-rs` maps to `aws_lc_rs`.

**The panic.** `REG/rustls-0.23.45/src/crypto/mod.rs:243-286`: `from_crate_features()` returns `None` when both
providers are on, and `get_default_or_install_from_crate_features` then panics. Neither reqwest installs a process
default; both build with `builder_with_provider`.

**tokio-tungstenite.** I fetched 0.30.0, the version §2.7 pins. Its built-in connector still calls
`ClientConfig::builder()` (`src/tls.rs:126`), and it takes rustls with `default-features = false`.

**Reproduced.** In `verify-cb/tlspanic`, `ClientConfig::builder()` panics with "Could not automatically determine the
process-level CryptoProvider…", and `builder_with_provider(ring)` works.

**Plan text.** The plan has no TLS row. `SessionConfig` (§3.2) has no TLS field. `RES/F-rust-stack-options.md:224` had
already recommended "Pin one TLS backend (ring)", but §2.7 did not carry it over. So the `full` CLI and owner tools
panic on the first `wss://` connect. No-host builds pull `aws-lc-sys` (C, with a CMake fallback per
`aws-lc-sys-0.40.0/builder/main.rs:534-576`).

**Fix APIs exist.** `connect_async_tls_with_config(req, Option<WebSocketConfig>, bool, Option<Connector>)` is in
tokio-tungstenite 0.30.0 (`connect.rs:61`). `ClientBuilder::tls_backend_preconfigured` is in reqwest 0.13.5
(`client.rs:2209`).

### Gaps in the reviewer's fix

1. With `rustls-no-provider`, reqwest's `default_rustls_crypto_provider()` panics for any client built without a
   preconfigured config when no process default exists (`reqwest-0.13.5/src/async_impl/client.rs:2499-2507`). The fix
   adds a new panic path and no backstop.
2. `tls_backend_preconfigured` downcasts to an owned `rustls::ClientConfig` (`:2220-2228`). Passing the
   `Arc<ClientConfig>` used for tungstenite produces "Unknown TLS backend" at `build()`.

### Corrected fix

Add a row to architecture §2.7:

> | TLS | `rustls 0.23`, ring provider only: `[workspace.dependencies] rustls = { version = "0.23", default-features = false, features = ["ring", "std", "tls12", "logging"] }` | One provider for the whole graph. The core's `host` enables `rustls/ring` through its blocking reqwest 0.12 (`UP/sdk/bittensor-core/Cargo.toml:60`). reqwest 0.13's default `rustls` feature adds `aws-lc-rs`. With both, rustls has no process default and `ClientConfig::builder()` panics (`REG/rustls-0.23.45/src/crypto/mod.rs:243-286`, reproduced). tokio-tungstenite 0.30's built-in connector calls exactly that (`REG/tokio-tungstenite-0.30.0/src/tls.rs:126`). Rules: (1) `rtao-rpc` builds `ClientConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider())).with_safe_default_protocol_versions()` with native roots (`rustls-native-certs`) plus a new `SessionConfig::extra_roots`, and connects with `connect_async_tls_with_config(request, Some(ws_config), false, Some(Connector::Rustls(Arc<ClientConfig>)))`. (2) Every rtao reqwest 0.13 client is built with `ClientBuilder::tls_backend_preconfigured(config)`, where `config` is an owned `rustls::ClientConfig` with ALPN `h2` and `http/1.1`. An `Arc` is not recognized and `build()` fails. (3) `rustls-no-provider` makes reqwest panic for any client built without a preconfigured config when no process default exists (`REG/reqwest-0.13.5/src/async_impl/client.rs:2499-2507`). So `rtao-cli`'s `main`, `NeuronRuntime::run` and the template binaries first call `let _ = rustls::crypto::ring::default_provider().install_default();`. That also covers third-party code calling `ClientConfig::builder()`. (4) `deny.toml` and `deps-check` ban `aws-lc-rs`, `aws-lc-sys` and `native-tls`. Tests: `rtao-rpc` and `SignedClient` connect to a loopback `wss://` and `https://` FakeNode with a self-signed certificate (added through `extra_roots`), in an `--all-features` build and in the no-host profile. A unit test asserts that `CryptoProvider::get_default().is_some()` after `NeuronRuntime` starts. |

Change the WebSocket row to: "`tokio-tungstenite 0.30` (`rustls-tls-native-roots`; connector supplied by `rtao-rpc`, see
TLS)".

Change the HTTP row to: "`reqwest 0.13.5` (`default-features = false`, features `rustls-no-provider`, `json`, `http2`,
`charset`; preconfigured ring `ClientConfig`, see TLS)".

## CB-02: partially_confirmed

### Checked

- `CORE/lib.rs:23` declares `timelock` ungated, and `CORE/timelock/mod.rs:12` declares `pub mod epoch_schedule`.
- `EpochScheduleState` (:17), `should_run_epoch` (:45, using `MAX_TEMPO_U64` at :54) and `predict_first_reveal_block`
  (:108) are public today.
- A symbol-existence probe therefore cannot detect PR-U5's behavioral fix to existing functions.
- `keyfiles` is host-gated (`lib.rs:15-16`), and the core has `default = ["host"]` (`Cargo.toml:113`).
- Appendix E runs `staged --check` in the `--all-features` job. The plan does not say which features the probe uses.

### Gap in the reviewer's fix

A scratch crate built without rtao's `[patch.crates-io] w3f-bls` line never compiles the core (E0277; `bench/nopatch`,
`SP/design-bench/target-nopatch/.../output-lib-bittensor_core`). Because "probe does not compile" is the normal state,
xtask must classify errors. Otherwise any failure reads as "not upstream yet", and the gate goes silently inert.

`CORE/client.rs:1515-1570` shows the `Value` helpers are free functions today, so the G12 probe path must follow
PR-U15 as filed.

### Corrected fix

Replace the §1.6 intro paragraph (mirror it in PLAN.md §5.3):

> Each `rtao-core-ext` module starts with three machine-read lines: `//! UPSTREAM-TARGET: <path> (PR-Ux)`, `//! UPSTREAM-PROBE: symbol <path>` or `//! UPSTREAM-PROBE: vectors <suite>`, and `//! UPSTREAM-FEATURES: <core features>` (default none). `cargo xtask staged --check` generates one scratch crate per probe under `target/staged-probe/`. Each crate is its own workspace. It depends on the pinned core with `default-features = false` plus exactly the listed features, which is the no-host profile rtao ships. It carries rtao's `[patch.crates-io] w3f-bls` line and a copy of rtao's `Cargo.lock`, because without the patch the core never compiles (E0277). A `symbol` probe names a path that does not exist at the current `core_tag`. A `vectors` probe runs the module's own test vectors against the core's implementation. Only three outcomes count as "not upstream yet": an unresolved-name error (E0412, E0425, E0432, E0433 or E0599), a missing core feature, or failing vectors. Any other failure (E0277, resolution or network errors) fails `staged --check` with the compiler output. `staged --check` also fails when a probe passes. Probes: G1-G3 `symbol` at the ungated Appendix D targets. G4 `vectors`: the skip and tempo-reset safety-net vectors run against `bittensor_core::timelock::epoch_schedule::{should_run_epoch, predict_first_reveal_block}`, which already exist with the `MAX_TEMPO_U64` predicate. G5 `symbol timelock::generate_commit_v2_at`. G8 `symbol runtime::Runtime::metadata_ir`. G9 `symbol keyfiles::deserialize_keypair_from_keyfile_data` with `UPSTREAM-FEATURES: keyfiles-json` (today it exists only under `host`). G10 `symbol keyfiles::encode_password_for_environment` (features `host`). G11 `symbol keys::verify_strict`. G12 `symbol` at the path PR-U15 introduces, recorded when it is filed (today the helpers are free functions in `client.rs:1515-1570`). A probe that compiles only with `host`, for a module the no-host profile uses, is reported as "upstream but host-gated" and does not trigger deletion.

Playbook row (architecture §12.9 and PLAN §10.6):

> | a staged PR ships in `core_tag` | `staged --check` fails because the module's probe (symbol at the ungated target, or the vector suite) now passes | 1 h: flip the `pub use`, delete the staged module |

## CB-03: partially_confirmed

### Checked

- `CORE/lib.rs:10-11` declares `#[cfg(feature = "host")] pub mod client;`, and `:24-25` gates `transaction` the same way.
- PR-U1's target (`client/ops.rs`) and PR-U2's target (`client::ops::receipt`) would therefore compile only with `host`.
- The staged ops are on the no-host path: `ops::runtime_api` (§3.6, line 1084), `ops::receipt` (§3.9.4), `ops::fee`
  (§3.10), and the M1 and M2 rows.

So the one-line flip cannot work for PR-U1 and PR-U2.

### Overstated parts

- **PR-U7.** Appendix D gives its rtao-side effect as "none (rtao never used it)". No staged module retires through it;
  the staged constant itself is questioned in CB-14.
- **PR-U8.** Its "direct reuse" is in `tests/policy_equivalence.rs` (§4.6). That test already builds the core with
  `host` as a dev-dependency, like `keyfile_json_diff.rs`. `rtao-tx` ports `Policy` and never reuses the core's.

### Corrected fix

Appendix D (mirror in PLAN.md §15):

- PR-U1 target: "`sdk/bittensor-core/src/protocol/{metadata,storage,runtime_api,extrinsic_lookup,dispatch_error,fee}.rs`,
  a new module declared in `lib.rs` without `#[cfg(feature = "host")]`; the blocking `client.rs` calls into it".
- PR-U2 target: "`sdk/bittensor-core/src/protocol/receipt.rs` (ungated)".
- PR-U7 and PR-U8 targets stay as written. No staged module retires through them: rtao never calls `submit_shielded`,
  and PR-U8's reuse is in the host-enabled `tests/policy_equivalence.rs`.
- Add under the table: "A PR whose symbols land only behind `host` does not retire a staged module that the no-host
  profile uses; `staged --check` probes without `host` (§1.6)."

## CB-04: partially_confirmed

### Checked

- ring compiles C (`REG/ring-0.17.14/build.rs:53-70`, build-dependency `cc`).
- aws-lc-sys has build-dependencies `cc` and `cmake`, with a CMake fallback.
- reqwest 0.13's default TLS is aws-lc-rs.
- Neuron profiles need TLS: finney's default endpoint is `wss://` (§3.3), and `neuron` pulls `btauth-client`, which
  pulls reqwest.
- `sp-core/std` enables the optional `secp256k1` dependency (polkadot-sdk `cacb431` `primitives/core/Cargo.toml:66-67`,
  `117-118`).
- CI feature sets: PLAN §5.7 builds `validator,miner` and Appendix E builds `validator,miner,prometheus`. `miner,redis`
  is never checked.

So the "no C compiler / no C toolchain" claims and the M2 exit criterion are unattainable.

### Problems in the reviewer's fix

1. "ring is the only C component" is false. `blake3` is a normal dependency of `merkleized-metadata` in the core's
   no-host graph (`design-tagpin/Cargo.lock`), and it compiles SIMD assembly or C intrinsics through `cc` whenever a C
   compiler is present (`REG/blake3-1.8.7/build.rs:343-360`).
2. `cargo tree -e normal` hides build-dependency edges, so `cmake` and `openssl-src` (a build-dependency of
   openssl-sys 0.9.117, `Cargo.toml:78`) can never appear. Measured in `verify-cb/tlspanic`: `-e normal` shows no
   `cmake` or `cc`, while `-e normal,build` shows both.
3. The `aws-lc-*` bans hold only with CB-01's ring-only decision.

### Corrected fix (PLAN.md)

- §1.4 row label: "weight-setting validator, no Perl/OpenSSL/libsodium".
- §1.5 #5 title: "Operator-first delivery with libsodium- and OpenSSL-free neuron builds".
- §1.5 #5 body and §6.1 #5: "Neuron binaries build without `bittensor-core/host`, so their images need no libsodium,
  OpenSSL, `secp256k1-sys`, Perl or CMake. The only native code they compile is `ring` (TLS) and `blake3`'s SIMD
  kernels (through `merkleized-metadata`). Both are built by the C compiler that rustc already uses as its linker on
  Linux."
- §7.1 comments: "hotkey only; no Perl, OpenSSL or libsodium" (validator), and the same for the miner.
- §7.5: "neuron images built with `-p <binary>` build in `rust:1.89-slim` as shipped (gcc only); the owner image adds
  `perl` and `make`".
- §12.4 M2 exit: "the no-host validator image builds FROM `rust:1.89-slim-bookworm` without installing perl, make,
  cmake, pkg-config, libssl-dev or libsodium-dev".
- §17: "no-host profile | neuron builds without `bittensor-core/host`: no libsodium, OpenSSL, `secp256k1-sys`, Perl or
  CMake; native code limited to `ring` and `blake3` SIMD".
- §5.7 CI bullet: "The CI `no-host` job builds `-p rtao --no-default-features --features validator,prometheus` and,
  separately, `--features miner,redis`. It asserts with `cargo tree -e normal,build` that none of `sodiumoxide`,
  `libsodium-sys`, `openssl`, `openssl-sys`, `openssl-src`, `native-tls`, `fernet`, `ansible-vault`, `secp256k1-sys`,
  `aws-lc-rs`, `aws-lc-sys`, `cmake`, `hidapi`, `libloading` or `rayon` is present. Including build edges makes C
  builders such as `cmake` and `openssl-src` visible. It also asserts that `reqwest` lacks the `blocking` feature and
  that `sp-core` is built without `std` (`cargo tree -e features -i sp-core`). The `aws-lc-*` entries assume the
  ring-only TLS decision (architecture §2.7)."

## CB-05: confirmed

- `CORE/codec/value.rs:58-77` falls back to `Number::from(0)` when `from_i128` or `from_u128` returns `None`.
- `REG/serde_json-1.0.154/src/number.rs:264-279`: `from_u128` returns `None` above `u64::MAX` without
  `arbitrary_precision`.
- `Number::as_str` exists only with the feature (`:306-310`).
- `to_corpus_json` is rtao's JSON path: `DynValue` `Display` (architecture §1.5) and the CLI and agent JSON (PLAN
  §5.1).
- An f64 comparison is exact only up to 2^53, and rao totals exceed that.

The proposed decision is consistent and implementable.

## CB-06: partially_confirmed (severity minor)

### Checked

- `sdk/bittensor-core-py` is a workspace member (`UP/Cargo.toml:35`), so the oracle builds with `UP/Cargo.lock`.
- 88 of the 241 packages in the core's no-host graph (`design-tagpin/Cargo.lock`) resolve to versions absent from
  `UP/Cargo.lock`. That includes all of the reviewer's examples.

### Why only partially

- The plan's sentence is about core sources and is literally true. The oracle remains the specification of Python's
  behavior.
- The offline oracle comparison runs on every PR (§4.9 layer 7, §11.5), so behavior drift fails CI. The risk is
  misattributed mismatches, not silent errors.
- The proposed alignment of the whole normal-dependency closure fails on day one. `UP/Cargo.lock` has tracing 0.1.41
  and tracing-core 0.1.34, while the planned tracing-subscriber 0.3.23 needs tracing-core ≥ 0.1.35 and tracing ≥ 0.1.43
  (`REG/tracing-subscriber-0.3.23/Cargo.toml`), and §2.7 pins tracing 0.1.44.
- Aligning rtao's CI lock says nothing about subnet builds.

### Corrected fix

Replace the §1.2 paragraph (mirror in PLAN.md §5.5, last bullet):

> The oracle (§11.5) builds `bittensor-core-py` from `parity.tag`. When the fingerprints match, its core sources are byte-identical to `core.tag`'s, so the two-tag scheme does not invalidate the oracle. Third-party versions are a separate matter. `bittensor-core-py` is a member of the monorepo workspace (`UP/Cargo.toml:35`) and builds with `UP/Cargo.lock`, while rtao and every subnet workspace resolve their own locks. At v477, 88 of the 241 packages in the core's no-host graph resolve to versions absent from `UP/Cargo.lock`, for example ml-kem 0.2.2/0.2.3, ed25519-zebra 4.1.0/4.2.0, scale-decode 0.16.0/0.16.2 and serde_json 1.0.145/1.0.154. `xtask pin` records `UP/Cargo.lock`'s versions of the byte-defining crates in the core's graph in `vendor/oracle/lock.json`: `parity-scale-codec`, `scale-info`, `scale-decode`, `frame-metadata`, `merkleized-metadata`, `schnorrkel`, `ed25519-zebra`, `bip39`, `sha2`, `sha3`, `blake2`, `twox-hash`, `ml-kem`, `chacha20poly1305`, `tle`, `ark-*` and `serde_json`. `deps-check` prints rtao's differences from that set as a warning. The oracle comparison runs offline on every PR (§4.9 layer 7), so a dependency bump that changes behavior fails CI, and such a mismatch is checked against `lock.json` before anything is ported.

## CB-07: partially_confirmed

### Checked

- hidapi is gated by `ledger`, not `host` (`UP/sdk/bittensor-core/Cargo.toml:86-94, 126`).
- The ban lists differ. One correction to the finding: §2.3 rule 5 does include blocking `reqwest`.
- The "no C toolchain" wording is still in architecture §0.2, §2.6, §9.11, §14 and §16.

The fix inherits CB-04's two errors.

### Corrected fix

- §1.7 first sentences: "Every proposal enables `bittensor-core/host` for neuron binaries. That means libsodium,
  vendored OpenSSL (which needs Perl `IPC::Cmd` and `make`) and `secp256k1-sys` (through `sp-core/std`), 330 crates
  instead of 207. hidapi is gated by the separate `ledger` feature and uses the pure-Rust hidraw backend on Linux."
- Replace the §1.7 CI bullet, §2.3 rule 5 and the Appendix E `no-host` job with the CB-04 corrected list
  (`cargo tree -e normal,build`; builds `validator,prometheus` and `miner,redis`).
- R4: "(libsodium, vendored OpenSSL needing Perl, secp256k1-sys)".
- In §0.2, §2.6 (both manifest comments and the Dockerfile sentence), §9.11, §14 M2 and §16 #5, replace "without the C
  toolchain" and "no C toolchain" with "without Perl, OpenSSL, libsodium or CMake (native code limited to ring and
  blake3 SIMD, built by the C compiler rustc already uses as its linker)".

## CB-08: confirmed

I re-ran the probe on clippy 0.1.89 (2025-08-04) and 0.1.99:

- Without the feature, the unresolvable path prints the warning and `-D warnings` still exits 0.
- With `allow-invalid = true`, there is no warning and exit is 0.
- With the feature and a call, the ban still fires (exit 101).
- The seven host-gated entries are confirmed: `timelock/mod.rs` gates the drand fetches (`:241`, `:275`, `:297`), and
  `generate_commit_v2` is ungated.

## CB-09: confirmed

- §10.4 compares against the doctor's own tag, and §1.2 step 3 rewrites only the root `Cargo.toml`.
- The template and the quickstart carry the line (§1.3 item 3; §13 line 3673).
- Re-measured in `verify-cb/refmismatch`: the core at `tag = "v477"` with the patch at
  `rev = "5c6e83e2…"`, the same commit, makes `cargo generate-lockfile --offline` lock two `w3f-bls 0.1.3` sources
  (`?tag=v477#5c6e83e2` and `?rev=5c6e83e2#5c6e83e2`). The reviewer's own probe used two different commits; this
  closes that gap.

## CB-10: confirmed

- The core declares `codec` directly (`Cargo.toml:55`).
- `scale-info` and `subtensor-macros` are inherited (`:79-80`), and `subtensor-macros` resolves to
  `path = "support/macros"` (`UP/Cargo.toml:74`).
- `support/macros` inherits `syn`, `quote`, `proc-macro2`, `ahash` and `edition`.
- The root `[patch.crates-io]` (`:334-337`) includes `zstd-sys` and `zstd-safe`, which are not in the core's graph.
- `subtensor-macros` is in the tag-pin lock.

## CB-11: partially_confirmed

### Checked

- Deferral sets `PendingEpochAt` (`run_coinbase.rs:422-431`), which both predicates honor (core `:50-53`, pallet
  `:1228-1230`).
- A skip advances `LastEpochBlock` without resetting the counter (`:434-438`, `:472-483`).
- `do_set_tempo` (`tempo_control.rs:38`) calls `apply_tempo_with_cycle_reset` (`misc.rs:119-125`), which resets
  `LastEpochBlock` only.
- §9.3 step 9 contradicts step 8.

### Overstated part

"Fires on the next block" holds for a skipped scheduled slot (counter = tempo) and for a lowered tempo. For a skipped
owner-triggered slot (`PendingEpochAt`, counter c < tempo) the pallet fires at b + tempo + 1 − c. While the input
state stays inconsistent, it fires on every block.

### Corrected fix

In PLAN §5.8 #2, PLAN §6.1 #20, PLAN §15 PR-U5, architecture §9.3 step 8 and §16 #20, replace "After a deferred or
skipped slot the core predicts the first reveal one epoch late" with:

> After an `EpochSkipped` slot (`run_coinbase.rs:472-483`: `LastEpochBlock` advances but `BlocksSinceLastStep` is not reset), or after `sudo_set_tempo` lowers the tempo below the counter (`tempo_control.rs:38` → `utils/misc.rs:119-125`), the pallet fires the safety net as soon as the counter exceeds `tempo`. That is the very next block after a skipped scheduled slot or a lowered tempo, and every block while the input state stays inconsistent. The core waits for `LastEpochBlock + tempo`, so it predicts the first reveal at least one epoch late.

Keep the existing consequence clause. Add: "Deferral sets `PendingEpochAt` (`run_coinbase.rs:422-431`), which both
predicates honor, and does not diverge."

Add to PLAN §12.4 (M2): "safety-net vectors: skip at a scheduled slot b ⇒ epoch at b+1; skip at an owner-triggered slot
b with counter c ⇒ epoch at b + tempo + 1 − c; tempo lowered below the counter in block t ⇒ epoch at t+1".

## CB-12: partially_confirmed

### Checked

- The root patch makes a crates.io `w3f-bls` impossible inside rtao's workspace.
- `diff -rq` between crates.io `w3f-bls` and `vendor/w3f-bls` shows four files differ, as stated.
- `single.rs:384` is `impl Valid for SecretKey::check`.
- `tle` uses only `EngineBLS` and `Message`, and `CORE/timelock` uses only `EngineBLS`.
- `vendor/w3f-bls/Cargo.toml:1-6` describes one patch.

### Flaw in the fix

"Identical ciphertexts are expected" is wrong as written. `encrypt_and_compress` draws both the ephemeral key and the
RNG from `OsRng` (`CORE/timelock/mod.rs:124-130`).

### Corrected fix

Scenario 16 becomes:

> **PR-U4 evidence**: `ci/pr-u4-probe/` is a separate Cargo workspace with its own `[workspace]` table, no `[patch]` and its own lock. It depends on `tle` at the monorepo pin (`ideal-lab5/timelock` rev `5416406c`, `features = ["std"]`) and on crates.io `w3f-bls = "=0.1.3"` (std). It reproduces `WeightsTlockPayload` and `encrypt_and_compress` (`CORE/timelock/mod.rs:53-58, 104-140`) with an injectable ephemeral key and RNG, and writes commits for chosen past and future rounds to `vendor/recorded/pr-u4/`. (a) Offline, rtao's workspace decrypts the past-round commits through `decrypt_with_signature` with committed quicknet signatures, and checks byte-equality against the vendored path under the same fixed key and seeded RNG. (b) On localnet, pallet-drand reveals the future-round commits.

Add to §1.3 item 4:

> crates.io `w3f-bls` 0.1.3 and `vendor/w3f-bls` differ in four files. `double_pop.rs` is a no-std rewrite with the same `BLS_POP_` tag and seed order. `lib.rs` drops the serde `extern crate`. `verifiers.rs:271` changes ownership only. In `single.rs:384`, `SecretKey::check` indexes `key[0], key[1]` instead of `key[1], key[2]`. Neither `tle` nor `CORE/timelock` touches `SecretKey`. So the two builds should produce byte-identical ciphertexts for the same ephemeral key and RNG seed, and each should decrypt the other's output. The probe confirms this.

## CB-13: confirmed

The architecture §2.6 owner-tool manifest (lines 689-692) has only `[dependencies]`. PLAN §7.1 already has the patch
line.

## CB-14: partially_confirmed

### Checked

- `check_mortality.rs:19` is a private `const` and is absent from metadata.
- `PY/settings.py:24-29` defines the Python value as a default that "must not exceed 8".
- The G6 staged constant has no named consumer, and Appendix D gives PR-U7 "none" as its rtao effect, which is
  inconsistent with G6.

### Overstated parts

- "No gate" is false. `runtime/src/check_mortality.rs` is in `map.toml`'s watched roots (line 1947), so a blob change
  fails the file layer.
- An equality between `py` and `rt` conflates Python's default with the chain maximum.

### Corrected fix

- G6 cell: "none (rtao never calls `Client::submit_shielded`)".
- Remove `shield::MAX_SHIELD_ERA_PERIOD` from §2.2 and from PLAN.md §4.3 and §5.3. The G6 row there becomes: "none;
  rtao never calls `submit_shielded`; the bound is extracted".
- Add to the §4.9 chain-source layer: "`MAX_SHIELD_ERA_PERIOD` from `runtime/src/check_mortality.rs:19` (a private
  runtime constant, absent from metadata) into `rtao_catalog::rt::MAX_SHIELD_ERA_PERIOD`. Tests assert
  `ShieldPeriod::MAX.blocks() == rt::MAX_SHIELD_ERA_PERIOD` and `py::MEV_SHIELD_ERA_PERIOD <= rt::MAX_SHIELD_ERA_PERIOD`
  (`PY/settings.py:24-29`: a default that must not exceed the runtime bound)."

## CB-15: confirmed

- `Runtime::resolve` returns `&scale_info::Type<PortableForm>` (`CORE/runtime/mod.rs:257`), and the core re-exports no
  `scale_info`.
- The binding's IR matches `scale_info::TypeDef::Variant` (`bittensor-core-py/src/runtime.rs:598`).
- G11's own text names schnorrkel and ed25519.
- `RequireFreezeStruct` flags any Encode/Decode struct (`support/linting/src/require_freeze_struct.rs`).
- `sp-crypto-hashing` in the core's graph comes from the polkadot-sdk git fork.
- `hash_param` supports `Blake2_256` and `Twox128`, and `mlkem::twox_128` is public.

Listing `frame-metadata` is optional, since the plan keeps V16 in xtask, but it is harmless.

## CB-16: confirmed

- The watched roots omit `sdk/bittensor-core-py/**`.
- `git diff --name-only v473 v475 -- sdk/bittensor-core-py` lists `pyproject.toml`, `src/lib.rs` and `src/pow.rs`.
- The binding's `src/*.rs` total 1,865 lines.

## CB-17: confirmed

§1.5 (line 350) says "lazily decoded V16 view". That contradicts §3.4 (line 993, "V16 is never parsed at run time"),
§17 #5 and PLAN §16.

## CB-18: confirmed

- `~/.cargo/git/db`: subtensor 293M, polkadot-sdk 480M.
- `tagpin.log` shows "Updating git repository" for all three sources, yet the whole command took 13.2 s, so the git
  dbs were already warm.
- §2.8 and the template Dockerfile text (§2.6, §9.11) say nothing about the cold fetch. Only rtao's own CI caches
  `~/.cargo/git` (Appendix E).
