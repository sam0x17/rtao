# J: Ledger signing in CI, an emulator shim (2026-10-13)

**Question.** The user accepted a manual Ledger device run per release and asked whether "some shim" could automate
Ledger testing (PLAN.md Q17). This report consolidates three research runs of 2026-10-13: the protocol and the core's
integration points, the tooling landscape with a local emulator run, and a hands-on run with a Rust probe on
`bittensor-core`. The decision it supports is recorded in [architecture §11.9](../architecture.md#119-ledger-emulator-lane-speculos)
and [PLAN.md §14.1](../../../PLAN.md#141-decisions-answered-2026-10-13) (Q17). After this research the user decided to
carry PR-U17 as a Cargo patch of the core in rtao's own workspace until it ships (Q20,
[architecture §1.11](../architecture.md#111-the-pr-u17-core-patch)); §0, §3 and §4 below are revised for that decision,
the evidence in §1 and §2 is unchanged, and §8 records a prototype of the core patch, run in Docker after the decision.

**Conventions.** `UP/`, `CORE/`, `PY/`, `PYT/` and `REG/` are the prefixes of
[architecture, Evidence conventions](../architecture.md#evidence-conventions) (`RaoFoundation/subtensor` at `850f2755`;
`sdk/bittensor-core` there is byte-identical to tag `v477`). `DOTAPP/` is the Polkadot generic Ledger app,
`Zondax/ledger-polkadot` at `644ec63851072f6eac9c3911ea23a811f3a1b15c` (tag `v100.0.29`,
<https://github.com/Zondax/ledger-polkadot/tree/644ec63851072f6eac9c3911ea23a811f3a1b15c>). Speculos is cited by URL at
`LedgerHQ/speculos` commit `abc6b7d93639c58e9309c4d05c3966e554ba6a52`. Markers: **[E]** executed in this planning
session (Docker 29.7.2 on macOS arm64, linux/arm64 containers); **[V]** read in the cited source or document, and
re-checked on disk where a local prefix is given; **[I]** inference, not executed. Durations quoted below are measurements
of emulator operations on that host, not estimates. The session's scratch workspace (driver scripts, logs, ELFs, the
probe crate) was not retained; the transcripts that matter are quoted here.

## 0. Answer

1. **An emulator lane works.** Speculos, Ledger's official device emulator, ran the Polkadot generic app headless and
   clear-signed requests built by `bittensor-core`: the core's own `ledger_proof_vector.json` and five more Bittensor
   calls, including a payload above 256 bytes. Every signature verified, and on-device rejection and tampering produced
   the expected status words. Two independent runs, three device models [E] (§1).
2. **rtao's production path cannot reach it unchanged.** The core's `LedgerDevice` can only open USB HID: its transport
   is a concrete type in a private module and `open()` is its only constructor (§3). Speculos serves APDUs over TCP and
   HTTP and has no USB HID ([speculos#40](https://github.com/LedgerHQ/speculos/issues/40)).
3. **Decision** (§4):
   - file **PR-U17** (an injectable APDU transport in the core) now; its evidence exists;
   - until it ships in `core_tag`, rtao's workspace builds the core through a Cargo `[patch]` that adds PR-U17 (Q20), so
     the emulator lane runs the core's own `ledger.rs` from its first run; a prototype of PR-U17 and of the patch did
     exactly that against the app, and showed that the patch needs a second table (§8); a test-only transcription of
     the core's APDU code in `rtao-testkit`, this report's first proposal, remains only the fallback if upstream
     declines PR-U17;
   - Speculos runs Python inside its image, so under Q2 the live lane is CI-only: nightly, on pushes that change the
     Ledger paths or the pins (labelled PRs after the goal loop of Q21), and when recordings are refreshed. Rust-only
     `pr-gates` replay the recorded transcripts, and the Python oracle pins what Python's `LedgerSigner` sends to the
     device;
   - a finney live probe checks that the runtime carries the compiled metadata hash, the failure behind
     [subtensor#3159](https://github.com/RaoFoundation/subtensor/issues/3159);
   - the manual device run per release stays as the final release check, for what the emulator cannot cover (§4.5).
4. **Nothing to depend on.** No Substrate or Bittensor project was found running Ledger emulator tests, and no Rust crate
   fits as a dependency. stellar-cli, starknet-rs and LedgerHQ's Bitcoin client are the patterns to copy (§6).

## 1. Executed in this session

### 1.1 Pins

| Item | Value |
|---|---|
| Speculos | `ghcr.io/ledgerhq/speculos` `v0.27.1`, index digest `sha256:028d1e368244631578537e7ca50b6f13ca2932b5f3fd220627289b559c07a886` (linux/amd64 and linux/arm64; `speculos 0.27.1` on Python 3.10.21 inside the image) [E] |
| App, run A | `LedgerHQ/app-polkadot` `v100.0.30` (`e36ced75`, Ledger's release fork of Zondax's app), ELFs from LedgerHQ's `compiled_app_binaries` CI artifact ([run 36904163258](https://github.com/LedgerHQ/app-polkadot/actions/runs/36904163258), retained 90 days, sign-in required): Nano S+ `deba482c73a5a5cdb343304b5989e6a9e847ca5c8d313c138ac7177367bfd8ff`, Nano X `6cf440dec1e155d1c031b0eff28aa69b1d851b3c7d478dea5708b1beb173a8ba`, Flex `6938d710d511ac550633e675b35b2f41cc4e56c3f5c594ae09c0de08fe708dd3`; API level 27 [E] |
| App, run B | `Zondax/ledger-polkadot` `v100.0.29` (`644ec638`) built from source in `ghcr.io/ledgerhq/ledger-app-builder/ledger-app-builder-lite@sha256:9f6f8779096e551dac8ba080227ec21287f286d1866f2dee0929e21dbb480dd0` (5.4.9, SDK v27.1.2): Nano S+ `a8bd35fc2e4d2ec97049a9cab518e4d2503cdd0469a53946393eeba81d2a43f8`, identical on a clean rebuild, and Flex `989525837ff3fbdb66613a176136d468a1a1e66943e52fcf8c7e143f33de7dc5`; API level 27 [E] |
| Probe (run B) | a Rust crate on `bittensor-core` tag `v477` (`default-features = false, features = ["ledger"]`, with the `w3f-bls` patch line), built in `rust:1.89-bookworm` with no system packages, because hidapi's Linux backend is pure Rust [E]. It reused `digest::{metadata_digest, generate_extrinsic_proof}`, `keys::{ss58_from_public, verify}` and hand-built only the APDU framing (§3) |
| Seed | Zondax's public test mnemonic `equip will roof matter pink blind book anxiety banner elbow sun young` (`DOTAPP/tests_zemu/tests/common.ts:4`) |

Run A drove Speculos with stdlib Python scripts over its REST API; run B used curl over REST and the Rust probe over the
raw APDU TCP port.

### 1.2 Results

| Check | Result |
|---|---|
| `GET_VERSION` | `(100, 0, 29)` (B) and `(100, 0, 30)` (A); target id `0x33100004` on Nano S+, `0x33300004` on Flex [E] |
| `GET_ADDR` at `m/44'/354'/0'/0'/0'` | public key `e1b4d72d27b3e91b9b6116555b4ea17138ddc12ca7cdbab30e2e0509bd848419`; at SS58 0 `166wVhuQsKFeb7bd1faydHgVvX1bZU2rUuY7FJmWApNz2fQY`, which is Zondax's published vector ([standard.test.ts:22-23](https://github.com/Zondax/ledger-polkadot/blob/644ec63851072f6eac9c3911ea23a811f3a1b15c/tests_zemu/tests/standard.test.ts#L22-L23)); at SS58 42 `5HAeMNeM1XzB9ab742XyV8rM4u1wsAUiQQod61n9cjMTrKbn`, equal to the core's `keys::ss58_from_public(pk, 42)`; account 1 gives `91faaf4e…53bee` [E] |
| `GET_ADDR` with display (P1 = 1) | address and public-key screens, then `APPROVE`; the same answer as the silent call [E] |
| Wrong CLA, unknown INS, coin type 60 | `0x6E00`, `0x6D00`, `0x6984` [E] |
| Clear-sign the core's vector (`UP/sdk/bittensor-core/fixtures/ledger_proof_vector.json`: `Balances.transfer_keep_alive`, spec 461, payload 146 B, the core's proof 3,470 B; polkadot-js's proof 2,511 B in run A) | screens `Please review`, `Chain node-subtensor`, `Balances transfer_keep_alive`, `dest Id`, `dest 5FHneW46…694ty`, `value TAO 0.000012345`, `APPROVE`; reply `00bc772a…d80e` with `0x9000`; the signature verifies, and every run returned the same bytes (RFC 8032 ed25519 is deterministic). Nano S+, Nano X and Flex (A); Nano S+ and Flex (B) [E] |
| Core-built requests (B; the core's proof for the vector was byte-equal to the fixture's `rust_proof_hex`) | `transfer_keep_alive` of 1 TAO at nonce 5 (payload 148 B, proof 3,470 B), `SubtensorModule.add_stake` (153/3,669), `set_weights` (131/3,692), `burned_register` (145/3,618): `0x9000`, verified with the core's `keys::verify` and with ed25519-dalek, and equal to an offline RFC 8032 signature [E] |
| Payload above 256 B (B) | `Utility.batch` of five transfers (312/3,841 B): the signature verifies over `blake2_256(payload)` and not over the raw payload [E] |
| Reject on the device (Nano S+) | `0x6986` with no data, the core's `RETCODE_USER_REFUSED` (`CORE/signers/ledger.rs:42, 171-173`) [E] |
| Tampering | a flipped proof byte: `0x6984` with `wrong metadata digest` (A) or `Value out of range` (B, a byte that broke parsing); a flipped metadata hash in the signed data: `0x6984` with `wrong metadata digest` (A, B); no review screen in any case [E] |
| Flex (touch) | swipes or a tap on the page arrow, then a hold on `Hold to sign`; the same signature [E] |
| Speculos `--automation` rules (B) | approved hands-free, but Speculos logs that the flag is deprecated, and a catch-all rule double-presses on multi-line screens [E] |
| Substrate dev phrase as the seed (B) | `m/44'/354'/0'/0'/0'` derives `5EvfzWBXDP7FaENtoRNaaD3UkiWEgpGj2Zjz6T4TCPnxamNH`: a Ledger key is not `//Alice`, so localnet tests must fund the emulator's address [E] |
| Docker Desktop's kernel (`7.0.12-linuxkit`) | `CONFIG_UHID` and `CONFIG_HIDRAW` unset (`/proc/config.gz`), so a uhid bridge cannot run there [E] |
| Measured on this host (B) | first APDU answered 0.52-0.56 s after `docker run` in five runs; `GET_VERSION` p50 0.24 ms over TCP; `GET_ADDR` p50 1.05-1.08 s, CPU-bound in `qemu-arm-static` (each key derivation is emulated); a Nano S+ signing flow with REST navigation 2.7-3.7 s for 6-10 screens and 7.1 s for the 24-screen batch; Flex 8.1 s including a 3 s hold [E] |
| ELF provenance | a source build is deterministic for a fixed tag and builder digest, but it does not reproduce the shipped release: release assets are installer scripts carrying API-level-26 Intel HEX ([installer_nanos_plus.sh](https://github.com/Zondax/ledger-polkadot/releases/download/v100.0.29/installer_nanos_plus.sh)), and neither builder reproduced those bytes [E] |

### 1.3 Transcripts

Ledger's `=>`/`<=` format (run B, Nano S+, app 100.0.29):

```
=> f900000000                                                GET_VERSION
<= 0000640000001d0033100004 9000                             test mode 0, 100.0.29, unlocked, target 0x33100004
=> f9010000162c000080620100800000008000000080000000802a00    GET_ADDR, silent, path 44'/354'/0'/0'/0', ss58 42
<= e1b4d72d27b3e91b9b6116555b4ea17138ddc12ca7cdbab30e2e0509bd848419 ‖ ASCII "5HAeMNeM1XzB9ab742XyV8rM4u1wsAUiQQod61n9cjMTrKbn" 9000
=> f9020000162c000080620100800000008000000080000000809200    SIGN INIT: path ‖ payload length 146 (u16 LE)
<= 9000
=> f9020100fa0503008eaf0415…                                 SIGN ADD, 14 chunks of 250 B of payload ‖ proof
<= 9000
=> f902020074…                                               SIGN LAST (116 B); review shown, approved over REST
<= 00bc772a98fcdc822b7fdc0bea4c0fed1040c0a6dd226b89db04bb12a65bc49ccf94117e80ddbf063138852d89e8eb3bb822b6f57710dd7b363ec77e507ddcd80e 9000
```

Run A, app 100.0.30: `=> f900000000` / `<= 0000640000001e0033100004 9000`; a tampered request's last chunk answered
`<= 77726f6e67206d6574616461746120646967657374 6984` (ASCII `wrong metadata digest`).

### 1.4 Not executed

A uhid virtual-HID bridge; GitHub-hosted runners; the ELF hash of an amd64 build; `LedgerHQ/app-polkadot` `v100.0.30`
built from source; Stax and Apex+; payload ‖ proof above 16 KiB; `CheckMetadataHash` mode Disabled; submitting a
Ledger-signed extrinsic to a localnet; a shielded submission with two approvals; Python's `LedgerSigner` against the
emulator.

## 2. The protocol and what the device checks [V]

- **App and APDUs.** The device runs Zondax's "Polkadot" generic app, version 100.0.5 or later
  (`UP/docs/guides/ledger.mdx:25-27`). CLA is `0xF9` (`DOTAPP/app/src/coin.h:23`); the core sends only INS `0x00`
  `GET_VERSION`, `0x01` `GET_ADDR` (path ‖ SS58 prefix u16 LE) and `0x02` `SIGN`, with P2 = 0 for ed25519
  (`CORE/signers/ledger.rs:24-36, 83-116`; `DOTAPP/app/src/apdu_handler.c:64-66`). The non-migration app accepts only
  `44'/354'` paths (`apdu_handler.c:52-58`).
- **Keys.** Ledger BIP32-Ed25519 (`os_derive_bip32_with_seed_no_throw(HDW_NORMAL, CX_CURVE_Ed25519, …)`,
  `DOTAPP/app/src/crypto.c:129-132`) at `m/44'/354'/account'/0'/index'`, five hardened u32 LE words
  (`CORE/signers/ledger.rs:179-194`). The app has no sr25519 (`CURVE_APP_LOAD_PARAMS = ed25519 secp256k1`,
  `DOTAPP/app/Makefile:111-115`). `Keypair::from_mnemonic` (Substrate SURI derivation) cannot reproduce a Ledger
  address, so tests pin the key the emulator derives for its seed.
- **Signing.** INIT carries path ‖ payload length (u16 LE); then payload ‖ proof travel in 250-byte ADD/LAST chunks; the
  reply is `0x00 ‖ sig64` (`CORE/signers/ledger.rs:124-156`; `DOTAPP/app/src/apdu_handler.c:74-106`; `crypto.c:135-136`).
  The payload is `call ‖ included_in_extrinsic ‖ included_in_signed_data` **unhashed** (`CORE/codec/extrinsic.rs:213-236`;
  `PY/ledger.py:155-157`), not `signature_payload`, which already hashes above 256 bytes (`extrinsic.rs:238-253`). The
  app itself hashes a blob above 256 bytes with blake2b-256 before signing (`crypto.c:119-126`). The proof is the SCALE
  `MetadataProof { proof, extrinsic, extra_info }` (`CORE/digest/mod.rs:117-142`).
- **Device checks.** The `CheckMetadataHash` mode byte must be `0x01` (Enabled), otherwise `wrong metadata digest`
  (`DOTAPP/app/src/parser_impl.c:91-110`); the app recomputes the RFC-0078 digest from the proof and compares it with the
  hash in the signed data (`parser_impl.c:183-196`).
- **Limits and status words.** The 16 KiB transaction buffer must hold payload ‖ proof
  (`DOTAPP/app/src/common/tx.c:36-37`); overflow answers `0x6983` (`apdu_handler.c:91-95`). `0x6986` is the user's
  rejection and also the answer to any APDU while a review is pending (`apdu_handler.c:268-270`); any `GET_ADDR` resets a
  signing in progress (`apdu_handler.c:43-44`). The core locks per APDU (`CORE/signers/hid.rs:36-43, 74-83`), while
  `sign` is several exchanges with no lock across them (`ledger.rs:124-156`), so a caller must serialize whole
  operations. `0x6984` carries an ASCII reason, which the core surfaces (`ledger.rs:198-219`).
- **Raw signing.** The app's `SIGN_RAW` (INS `0x03`, `coin.h:26`) signs only `<Bytes>…</Bytes>`-wrapped data
  (`DOTAPP/app/src/common/tx.c:74-104`); the core does not implement it, and Python refuses raw signing
  (`PY/ledger.py:165-175`). rtao never produces the wrapped form (Q4).
- **Chain constants.** The runtime compiles the metadata hash only with feature `metadata-hash`, as
  `enable_metadata_hash("TAO", 9)` (`UP/runtime/build.rs:10-18`); localnet builds enable it (`UP/scripts/localnet.sh:40,
  47`), as does the production image (`UP/Dockerfile:40`). A runtime built without it rejects a mode-Enabled extrinsic
  with `UnknownTransaction::CannotLookup` (polkadot-sdk fork `cacb431`,
  `substrate/frame/metadata-hash-extension/src/lib.rs:150-155`), which is how every Ledger transaction failed on finney
  runtimes 456 and 457 ([subtensor#3159](https://github.com/RaoFoundation/subtensor/issues/3159)).
- **Python parity.** Python takes `spec_name` from `state_getRuntimeVersion` (`PY/_transport/contract.py:60`), the SS58
  format from its codec, and decimals and symbol from `LedgerSigner` arguments that default to 9 and `"TAO"`
  (`PY/ledger.py:37-41, 68-81, 115-128`). Architecture §3.9.2 had hard-coded `"node-subtensor"` and 42; it now takes the
  runtime's values. A shielded submission needs two device approvals (`PY/cli/context.py:316-320`;
  `PY/executor.py:124-130`).

## 3. The blocker and the options

**Facts [V].** `mod hid;` is private (`CORE/signers/mod.rs:6-7`); `LedgerDevice { transport: Transport }` holds the
concrete HID type (`ledger.rs:52-54`); `open()` is the only constructor (`:61-65`) and takes the first HID device with
VID `0x2c97` and usage page `0xFFA0` or interface 0 (`hid.rs:47-72`); APDU building and parsing are private
(`ledger.rs:158-219`). Python's tests replace the whole device class (`PYT/unit/test_ledger_signer.py:34-54`). No
upstream test or workflow sends an APDU to the app; Ledger appears in CI only as a wheel feature
(`UP/.github/workflows/build-core-wheels.yml:89-91`). The WASM binding keeps a `LedgerError` name "for when a WebHID
signer backend lands" (`UP/sdk/bittensor-core-wasm/src/errors.rs:15-18`).

| Option | What meets the app | Status | Verdict |
|---|---|---|---|
| A. **PR-U17**: `ApduTransport` trait plus `LedgerDevice::with_transport` in the core | the core's `ledger.rs` (not `hid.rs`), on any OS with Docker | evidence exists; not filed | **adopt** as the end state; file in M0 |
| B. Linux **uhid** virtual Ledger (VID `0x2c97`, usage page `0xFFA0`, 64-byte reports) bridged to Speculos's TCP port | the core's unmodified `open()`, `hid.rs` and `ledger.rs`, and Python's `LedgerSigner` unchanged | unverified: needs the `uhid` module and write access to `/dev/uhid` and the new hidraw node ([kernel docs](https://docs.kernel.org/hid/uhid.html)); impossible on Docker Desktop [E]; GitHub runners not checked. Precedent: [Trezor's hid-bridge](https://github.com/trezor/trezor-firmware/tree/e6ae73d6540cf91a677e6d16a177f862a29810de/core/tools/hid-bridge). hidapi's Linux backend would enumerate it (`REG/hidapi-2.6.6/src/linux_native.rs:43-64, 79-164, 202-241`) [I] | not planned; revisit only if PR-U17 is declined |
| C. A staged copy of `ledger.rs` and `hid.rs` in `rtao-core-ext`, used in production until PR-U17 | rtao's copy, which is what ships | — | rejected: production would leave the core's COMPAT code for test convenience, and `rtao-core-ext` would gain `hidapi` and `ledger-apdu` |
| D. A **test-only APDU client** in `rtao-testkit`: the core's constants, path, chunking and error text transcribed | everything above the APDU layer: rtao's request, the core's proof, the app, the chain | [E]: run B's probe did exactly this | first proposed as the interim; after Q20, only the fallback if upstream declines PR-U17 |
| E. The backend seam alone (`FakeLedger`) | nothing below `LedgerSigner` | — | kept for unit tests; not enough |
| F. **A core patch**: rtao's root manifest builds `bittensor-core` from a fork branch, `core_tag` plus the PR-U17 commits, until a `core_tag` contains PR-U17 | the core's `ledger.rs` (not `hid.rs`) from the lane's first run, on any OS with Docker | decided by the user after this research (Q20) and prototyped (§8); subnet builds never see it, because Cargo ignores `[patch]` in dependencies | **adopted** until PR-U17 ships ([architecture §1.11](../architecture.md#111-the-pr-u17-core-patch)) |

## 4. Recommended design

### 4.1 The signer side (architecture §6.5)

- `LedgerSigner` drives the device through an rtao-internal `LedgerBackend` trait (`app_version`, `address`, `sign`),
  which mirrors Python's `_device_cls` seam (`PY/ledger.py:44-51`). Implementations: the core's `LedgerDevice` in
  production, `FakeLedger` for the ports of `test_ledger_signer.py`, and, in `rtao-testkit`, the core's own
  `LedgerDevice` built by `LedgerDevice::with_transport` over a Speculos TCP or replay transport (§4.2). The
  trait and `LedgerSigner::with_backend` are public only with `rtao-signer`'s feature `test-util`.
- The request is `sign(account, index, call ‖ extra ‖ additional, proof)`, never `SigningRequest.payload`; the proof
  comes from `generate_extrinsic_proof` with the runtime's `ChainInfo` (§2, Python parity); the 65-byte reply becomes
  `MultiSignature { Ed25519, … }`.
- One device mutex covers whole operations (the INIT…LAST sequence, address and version calls), and device I/O runs in
  `spawn_blocking`. A blocked device read cannot be cancelled, so emulator tests always end a review by approving or
  rejecting it [I].
- Raw signing stays refused (§2). Device errors are surfaced with the core's messages
  (`CORE/signers/ledger.rs:169-219`), which the troubleshooting guide quotes (`UP/docs/guides/ledger.mdx:100-110`).

### 4.2 Transport tiers

- **From the lane's first run.** `LedgerDevice::with_transport` over a TCP transport in `rtao-testkit` (Speculos
  framing: request `u32 BE length ‖ APDU`, response `u32 BE length of the data ‖ data ‖ SW`, the length excluding the
  status word;
  [mcu/apdu.py:75-106](https://github.com/LedgerHQ/speculos/blob/abc6b7d93639c58e9309c4d05c3966e554ba6a52/speculos/mcu/apdu.py#L75-L106)),
  and over a replay transport for `pr-gates`. The framing only consumes the core's bytes, so it is harness code, not a
  format rtao ships, and replay runs the core's own `ledger.rs`. Until PR-U17 ships in `core_tag`, `with_transport`
  comes from the core patch (option F; architecture §1.11), and only `rtao-testkit::ledger_emu` may name it, so no
  shipped crate depends on the patch. §8.7 ran both transports against the app.
- **Fallback.** If upstream declines PR-U17, the user chooses between carrying the patch, option D (a test-only
  `LedgerBackend` over Speculos's APDU port that transcribes `CORE/signers/ledger.rs`: constants, `bip44_path`,
  chunking, `device_error`, with the attribution header) and option B.

### 4.3 What runs and what it asserts

[E] in this table means that the device side was exercised in §1, through the probe's own APDU framing; rtao's
`LedgerSigner` does not exist yet.

| Where | Test | Status here |
|---|---|---|
| `ledger_emu` target (live, CI-only) | `app_version` equals the pinned app version | [E] |
| | `address(0, 0, 42, false)` equals the pinned public key and address of the test mnemonic, and the address equals `ss58_from_public(pk, 42)`; account 1; the displayed address approved over REST | [E] |
| | the `ledger_proof_vector.json` request signed through `LedgerSigner`: reply byte 0 is `0x00`, the signature verifies, and it equals the golden `00bc772a…d80e` | [E] |
| | core-built requests for golden calls (transfer, `add_stake`, `set_weights`, `burned_register`) verify and equal their golden signatures | [E] |
| | a `Utility.batch` above 256 B verifies over the core's `signature_payload` (its blake2-256 hash) | [E] |
| | a rejection maps to the core's rejected message; a tampered proof and a tampered metadata hash give `0x6984` with the app's reason | [E] |
| | mode Disabled gives `0x6984` `wrong metadata digest`; payload ‖ proof above 16 KiB gives `0x6983`, surfaced verbatim | [V] in the app source, not executed |
| | Flex: the vector signed and a rejection, with touch navigation | [E] for signing; the rejection ran on Nano S+ only |
| | on the pinned localnet (built with `metadata-hash`): the emulator's address, funded by Alice, signs a transfer through `Executor`, included with `ExtrinsicSuccess`; on the 12 s localnet, a shielded transfer with both approvals inside the 8-block shield era (on fast blocks 8 blocks last 2 s, less than one emulated approval flow, §1.2) | [I] |
| `pr-gates` (Rust-only) | `ledger_replay`: the recorded transcripts of the offline cases replayed through a replay transport handed to `LedgerDevice::with_transport` (from the core patch until PR-U17 ships): rtao's request bytes equal the recorded ones, and the recorded reply is handled, verified and equal to the golden | [I] |
| | `ledger_request_parity`: rtao's `(payload, proof, account, index)` equal Python's for the same requests, from an oracle dump of Python's `LedgerSigner` through its `FakeDevice` seam (`PYT/unit/test_ledger_signer.py:34-54, 88-129`) | [I] |
| `live-probe` (finney, read-only) | `TaggedTransactionQueue_validate_transaction` of a mode-Enabled extrinsic signed by an ephemeral ed25519 key with rtao's digest returns neither `UnknownTransaction::CannotLookup` (no compiled hash) nor `InvalidTransaction::BadProof` (a different hash) | [I] |

An interop lane that runs Python's `LedgerSigner` against the emulator is not planned: Python's device is
`bittensor_core.LedgerDevice()`, HID only (`PY/ledger.py:44-51, 82`), so it would need option B. The oracle dump gives
the same parity offline, and the emulator proves that the app accepts those bytes.

### 4.4 Where Python runs (Q2)

Speculos is a Python program (Flask and QEMU user mode) even inside its image
([Dockerfile](https://github.com/LedgerHQ/speculos/blob/abc6b7d93639c58e9309c4d05c3966e554ba6a52/Dockerfiles/Dockerfile#L18-L47)),
although the runner needs no Python. The live lane is therefore a CI-only lane like the oracle and upstream's
recorders: it records transcripts that Rust-only tests replay. A failing emulator can delay a core bump (`ledger-stale`)
but never blocks a build.

### 4.5 What only a real device covers

The manual device run per release stays, because the emulator cannot cover:

1. the core's HID transport: `hid.rs` framing, hidapi enumeration, OS device access (udev rules on Linux, IOKit on
   macOS) and `open()` picking the first Ledger. Until PR-U17 ships, the device-run build, like the lane, uses the
   patched core, whose `open()` path differs from upstream only by the seam;
2. the binary Ledger Live installs and the app version users actually run: the lane runs a source build of a pinned
   tag, which does not reproduce the released bytes (§1.2);
3. real firmware and secure element: Speculos emulates a subset of syscalls, has no watchdog, handles unaligned access
   differently, and does not guarantee identical behavior
   ([README](https://github.com/LedgerHQ/speculos/blob/abc6b7d93639c58e9309c4d05c3966e554ba6a52/README.md#L69-L115));
4. the human review on a real screen: the lane reads text events, not what a user sees.

### 4.6 Upstream

- **PR-U17** (`sdk/bittensor-core/src/signers/{mod.rs, ledger.rs, hid.rs}`):
  `pub trait ApduTransport: Send + Sync { fn exchange(&self, apdu: &[u8]) -> Result<Vec<u8>, CoreError>; }`, implemented
  by the HID `Transport`; `LedgerDevice` holds `Box<dyn ApduTransport>`;
  `LedgerDevice::with_transport(transport: impl ApduTransport + 'static) -> Self`; `open()` unchanged, so the bindings
  are untouched (prototyped in §8: 108 added lines in three files, 69 of them three unit tests [E]). Optional: a
  std-only Speculos TCP transport and a binding constructor, so upstream's own tests can run on the emulator. Precedent:
  stellar-cli is generic over Zondax's `Exchange` trait with HID and emulator implementations
  ([signer/ledger.rs:84-112](https://github.com/stellar/stellar-cli/blob/7c07fd2d0f89ea0f7eef500df44b147428d70fd0/cmd/soroban-cli/src/signer/ledger.rs#L84-L112)).
  Evidence: §3, §1 and §8. The emulator lane's transcripts strengthen the open PR. It is filed from the user's fork of
  the monorepo, whose `rtao-core-patch/<core_tag>` branches carry the same commits into rtao's workspace until a
  `core_tag` contains them (architecture §1.11).
- **App fix** (outside the monorepo; filed with `Zondax/ledger-polkadot` on 2026-10-13 at the user's request, Q22:
  issue [Zondax/ledger-polkadot#295](https://github.com/Zondax/ledger-polkadot/issues/295),
  PR [#296](https://github.com/Zondax/ledger-polkadot/pull/296)): the app formats an amount only when the field's type
  name is `Balance`, `BalanceOf<T>`, `BalanceOf<T, I>`, `DepositBalance`, `ExtendedBalance`, `PalletBalanceOf<T>` or
  `T::Balance` (`DOTAPP/app/src/metadata_parser.c:74-75`), so `add_stake`'s `amount_staked`, typed `TaoBalance`,
  appears as `1000000000` with no unit [E]. The PR adds `TaoBalance` to that list. `AlphaBalance`, a per-subnet token
  and never TAO, stays unformatted. rtao's Ledger guide notes the raw amounts until a released app version formats
  them (PLAN.md M7-12).

## 5. CI recipe

- **Pins** (`ci/ledger/pins.toml`): the app repository, tag and commit; the builder and Speculos images by digest
  (Speculos tags are inconsistent: no `v` prefix up to `0.25.13`, gaps after it,
  [package](https://github.com/LedgerHQ/speculos/pkgs/container/speculos)); the ELF sha256 per model and runner
  architecture; the seed. The app must not need an API level Speculos lacks: API 27 arrived in Speculos 0.27.0
  ([CHANGELOG](https://github.com/LedgerHQ/speculos/blob/abc6b7d93639c58e9309c4d05c3966e554ba6a52/CHANGELOG.md#L20-L24)).
- **Build** (run B's recipe [E]): `git clone --depth 1 --branch <tag> --recurse-submodules --shallow-submodules <repo>`,
  check the commit, then `docker run --rm -v <src>:/app -w /app/app -e BOLOS_SDK=/opt/<nanosplus|flex>-secure-sdk
  <builder@digest> bash -lc 'git config --global --add safe.directory "*"; make -j10 COIN=DOT PRODUCTION_BUILD=1'`,
  giving `app/build/{nanos2,flex}/bin/app.elf`; compare the sha256 with the pin. Cache the ELF by its pin. Do not vendor
  it: the app is Apache-2.0 and Speculos LGPL-3.0, and nothing is redistributed when the ELF is built in CI.
- **Run** [E]: `docker run -d --rm -v <elf dir>:/apps:ro -p 127.0.0.1:5000:5000 -p 127.0.0.1:9999:9999 <speculos@digest>
  --model <nanosp|flex> --display headless --apdu-port 9999 --api-port 5000 --seed "<mnemonic>" /apps/<elf>`. The
  harness publishes ephemeral host ports instead (`-p 127.0.0.1::9999`) and reads them with `docker port`, as the
  localnet harness does [I]. Speculos binds 0.0.0.0, so publish to loopback; the image does not `EXPOSE` 5000
  ([Dockerfile:40-45](https://github.com/LedgerHQ/speculos/blob/abc6b7d93639c58e9309c4d05c3966e554ba6a52/Dockerfiles/Dockerfile#L40-L45)).
- **Approve** [E] through the REST API
  ([api.py:90-108](https://github.com/LedgerHQ/speculos/blob/abc6b7d93639c58e9309c4d05c3966e554ba6a52/speculos/api/api.py#L90-L108)):
  poll `GET /events?currentscreenonly=true`; on Nano, `POST /button/right` until `APPROVE` (or `REJECT`), then
  `POST /button/both` with `{"action": "press-and-release"}`; on Flex, `POST /finger` taps or swipes until
  `Hold to sign`, then a press with a `delay`. Pin the screen strings per model.
- **Isolation.** One container per test: the APDU server serves one client at a time and a new connection takes over
  ([mcu/apdu.py:42-45](https://github.com/LedgerHQ/speculos/blob/abc6b7d93639c58e9309c4d05c3966e554ba6a52/speculos/mcu/apdu.py#L42-L45)),
  concurrent `POST /apdu` calls can cross their answers ([#685](https://github.com/LedgerHQ/speculos/issues/685)), and a
  busy APDU port silently becomes a random one ([mcu/apdu.py:24-36](https://github.com/LedgerHQ/speculos/blob/abc6b7d93639c58e9309c4d05c3966e554ba6a52/speculos/mcu/apdu.py#L24-L36)).
- **Record format.** Ledger's `=>`/`<=` line pairs, as in `@ledgerhq/hw-transport-mocker`'s `RecordStore`
  ([RecordStore.ts:140-149](https://github.com/LedgerHQ/ts-libs/blob/0afe035e9841c33aaaf04364d4e511f12643d7af/libs/hw-transport-mocker/src/RecordStore.ts#L140-L149))
  and LedgerHQ's Rust Bitcoin client tests ([client.rs:19-44](https://github.com/LedgerHQ/app-bitcoin-new/blob/bc76e7b66b9525f878b3901a82d35151614705e0/bitcoin_client_rs/tests/client.rs#L19-L44)).
- **Workflow.** Architecture Appendix E (`ledger-emu.yml`, and a step in `pin-bump.yml`). Docker on GitHub-hosted
  runners needs an Ubuntu runner
  ([GitHub docs](https://docs.github.com/en/actions/tutorials/use-containerized-services/use-docker-service-containers));
  the Speculos image and the localnet image (`UP/.github/workflows/docker-localnet.yml:132-135, 200-203`) both cover
  linux/amd64 and linux/arm64.

## 6. Landscape

| Tool | Fit |
|---|---|
| [Speculos](https://github.com/LedgerHQ/speculos) (LGPL-3.0) | adopted: models `nanox`, `nanosp`, `stax`, `flex`, `apex_p` (Nano S was removed in 0.25.7, [struct.py:15-21](https://github.com/LedgerHQ/speculos/blob/abc6b7d93639c58e9309c4d05c3966e554ba6a52/speculos/mcu/struct.py#L15-L21)); REST API for buttons, touch, events and screenshots; `--automation` deprecated in favor of REST ([automation.md](https://github.com/LedgerHQ/speculos/blob/abc6b7d93639c58e9309c4d05c3966e554ba6a52/docs/user/automation.md)) |
| [Zemu](https://github.com/Zondax/zemu) (Node) | reference only: Zondax's app suite runs on it; its navigate-until-text helpers and touch maps are the pattern to copy ([Zemu.ts:707-858](https://github.com/Zondax/zemu/blob/d25a8effe3e7e7074b4a7dfb44469bbf0a637fcd/src/Zemu.ts#L707-L858)) |
| [Ragger](https://github.com/LedgerHQ/ragger) (Python) | Ledger's app-test framework over Speculos and real devices; allowed in a CI-only lane, not needed |
| `ledger-transport-zemu` | deprecated and removed from ledger-rs ([ledger-rs#110](https://github.com/Zondax/ledger-rs/pull/110)) |
| `ledger-lib`, `ledger-sim` 0.1.0 (2023) | stale: the crates.io releases trail the git `main`, and `ledger-sim` still emits Speculos's removed `--sdk` flag and lists `nanos` ([lib.rs:71-194](https://github.com/ledger-community/rust-ledger/blob/c8ed12e89788e78d77cdc0dc9fb8a4bd4dc24b89/sim/src/lib.rs#L71-L194)); `ledger-lib`'s TCP transport ([tcp.rs:104-151](https://github.com/ledger-community/rust-ledger/blob/c8ed12e89788e78d77cdc0dc9fb8a4bd4dc24b89/lib/src/transport/tcp.rs#L104-L151)) is a reference only |
| `speculos-client` 0.1.2 | spawns a local `speculos` from `PATH`; no buttons, events or touch ([lib.rs:161-229](https://github.com/xJonathanLEI/speculos-client/blob/69a72a2f3ed93fb346ce98dbbd40dd6881290cd7/src/lib.rs#L161-L229)) |
| `testcontainers` 0.28 | not needed: rtao drives Docker with the plain CLI, as its localnet harness does |
| LedgerHQ's Bitcoin Rust client | the pattern for a TCP transport (`Mutex<TcpStream>` on port 9999, [transport.rs:52-93](https://github.com/LedgerHQ/app-bitcoin-new/blob/bc76e7b66b9525f878b3901a82d35151614705e0/bitcoin_client_rs/examples/ledger_hwi/src/transport.rs#L52-L93)) and for transcript replay |

Rust projects with Ledger emulator CI: stellar-cli (testcontainers, committed ELFs, fixed right-click counts, x64 on PRs,
macOS disabled; [ledger-emulator.yml](https://github.com/stellar/stellar-cli/blob/7c07fd2d0f89ea0f7eef500df44b147428d70fd0/.github/workflows/ledger-emulator.yml))
and starknet-rs (Ledger's dev-tools container, `speculos-client`, automation rules;
[test.yaml:155-179](https://github.com/xJonathanLEI/starknet-rs/blob/933ac0d61b6d425edb0f8afbb4de44da541b4a26/.github/workflows/test.yaml#L155-L179)).
In the Substrate and Bittensor ecosystem a GitHub code search on 2026-10-13 found no Speculos use in paritytech,
polkadot-fellows, TalismanSociety, novasamatech, Koniverse, polkadot-js, opentensor or RaoFoundation [E];
`subxt-signer` has no Ledger signer
([Cargo.toml:17-56](https://github.com/paritytech/subxt/blob/73c7022cc90de55073fc257869dca16016fe0647/signer/Cargo.toml#L17-L56));
wallets drive the app through `@zondax/ledger-substrate`'s `PolkadotGenericApp`
([Talisman](https://github.com/TalismanSociety/talisman/blob/c084f33d82e91716690b97b47a1e17bd87b5d3f7/apps/extension/src/ui/hooks/ledger/useLedgerPolkadot.ts#L10-L42),
[polkadot-js hw-ledger](https://github.com/polkadot-js/common/blob/9281daa14c2c2a41708c3489de8770a064525577/packages/hw-ledger/src/LedgerGeneric.ts#L7-L29)).
rtao's lane would be the first.

## 7. Open items

- Whether GitHub-hosted runners load `uhid` (only if option B is ever needed).
- The ELF hashes of amd64 builds and of `LedgerHQ/app-polkadot` `v100.0.30` built from source: recorded when the lane is
  built.
- The cases marked [V] or [I] in §4.3, the Stax and Apex+ models, and the 16 KiB limit: exercised when the lane is
  built.
- What the core-patch prototype could not run (§8.8): a GitHub fork, a merged PR-U17 and a device.

## 8. Core patch prototype

After Q20, the core patch's mechanics (architecture §1.11) were executed in Docker, so that M0-01 can start PR-U17 from
a tested change [E]. A local bare clone of the monorepo at `v477` (`5c6e83e2f4936f257234f246957f75684c35227c`) stood in
for the user's fork and was served to Cargo as a `file://` URL. Its branch `rtao-core-patch/v477` is one commit,
`b2919454293cd7e084c6f8274c8bd94484aa493f`, made with a fixed identity and date so that it is reproducible. Builds ran
in `rust:1.89-bookworm` (rustc, cargo and clippy 1.89.0), and the emulator run used Speculos `v0.27.1` with
`--model nanosp` and the run-B Nano S+ ELF of §1.1. Nothing was pushed to GitHub.

### 8.1 The PR-U17 change

`git diff --stat v477`: `signers/hid.rs` +6 −1, `signers/ledger.rs` +86 −2, `signers/mod.rs` +16; 108 added lines, 69
of them three unit tests [E].

- `signers/mod.rs` declares `pub trait ApduTransport: Send + Sync` with one method, `fn exchange(&self, apdu: &[u8]) ->
  Result<Vec<u8>, CoreError>`, outside the `ledger` gate: it needs nothing from hidapi or `ledger-apdu`, so it exists
  in every build, wasm32 included. Path: `bittensor_core::signers::ApduTransport`.
- `signers/hid.rs`: the existing `exchange` body moves unchanged into `impl ApduTransport for Transport`.
- `signers/ledger.rs`: `LedgerDevice { transport: Box<dyn ApduTransport> }`; `open()` keeps its signature and only adds
  `Box::new`; a new `pub fn with_transport(transport: impl ApduTransport + 'static) -> Self`, at
  `bittensor_core::signers::ledger::LedgerDevice::with_transport`. Every other signature is unchanged, and
  `LedgerDevice` stays `Send + Sync`, which the PyO3 class and its `allow_threads` closure need.
- Tests in `ledger.rs`: `ledger_device_is_send_and_sync`; `with_transport_carries_the_chunked_sign` (a scripted
  transport sees INIT, then ADD and LAST of 250 bytes each, for a 300-byte payload and a 200-byte proof); and
  `with_transport_maps_user_refusal` (`0x6986` gives "device error: the request was rejected on the device").

All of these passed in the monorepo workspace with `--locked` [E]: `cargo check -p bittensor-core
--no-default-features`, with and without `--features ledger` and for `wasm32-unknown-unknown` (upstream's portable-core
check); `cargo test -p bittensor-core --features ledger --lib signers::` (5 passed: 2 existing, 3 new);
`cargo check -p bittensor-core-py`; `cargo clippy -p bittensor-core -p bittensor-core-py --all-targets --all-features
-- -D warnings` under the workspace lints; `rustfmt --check`; and no rustdoc warning in `signers/`. Two failures predate
the change: at `v477`, `cargo test -p bittensor-core --no-default-features --features ledger --lib` does not compile
(the `timelock` tests call `host`-only functions), and `cargo doc` with `-D warnings` fails on `keys/`, `runtime/` and
`timelock/`. The PR's tests therefore run with the default features plus `ledger`. One review nit is left:
`LedgerDevice::exchange` still says "HID response was too short" whatever the transport.

The diff without its test hunk, derived from RaoFoundation/subtensor `sdk/bittensor-core/src/signers/` at `v477`
(Apache-2.0):

```diff
diff --git a/sdk/bittensor-core/src/signers/hid.rs b/sdk/bittensor-core/src/signers/hid.rs
index e3bed7e..4e9d299 100644
--- a/sdk/bittensor-core/src/signers/hid.rs
+++ b/sdk/bittensor-core/src/signers/hid.rs
@@ -21,6 +21,7 @@ use std::sync::Mutex;
 use hidapi::{DeviceInfo, HidApi, HidDevice};
 
 use crate::error::CoreError;
+use crate::signers::ApduTransport;
 
 const LEDGER_VID: u16 = 0x2c97;
 const LEDGER_USAGE_PAGE: u16 = 0xFFA0;
@@ -70,10 +71,12 @@ impl Transport {
             device: Mutex::new(device),
         })
     }
+}
 
+impl ApduTransport for Transport {
     /// One APDU round-trip. Returns the raw answer including the trailing
     /// two-byte return code; blocks until the device responds.
-    pub fn exchange(&self, apdu: &[u8]) -> Result<Vec<u8>, CoreError> {
+    fn exchange(&self, apdu: &[u8]) -> Result<Vec<u8>, CoreError> {
         let device = self
             .device
             .lock()
@@ -81,7 +84,9 @@ impl Transport {
         Self::write_apdu(&device, apdu)?;
         Self::read_apdu(&device)
     }
+}
 
+impl Transport {
     fn write_apdu(device: &HidDevice, apdu: &[u8]) -> Result<(), CoreError> {
         let total = u16::try_from(apdu.len())
             .map_err(|_| CoreError::Device("APDU exceeds the 64 KiB framing limit".into()))?;
diff --git a/sdk/bittensor-core/src/signers/ledger.rs b/sdk/bittensor-core/src/signers/ledger.rs
index 9422720..395e885 100644
--- a/sdk/bittensor-core/src/signers/ledger.rs
+++ b/sdk/bittensor-core/src/signers/ledger.rs
@@ -11,6 +11,10 @@
 //! `m/44'/354'/account'/0'/index'` — the same keys Ledger Live and every
 //! generic-app wallet derive, so one device shows one set of addresses
 //! everywhere.
+//!
+//! [`LedgerDevice::open`] reaches the device over USB HID;
+//! [`LedgerDevice::with_transport`] sends the same requests over any other
+//! [`ApduTransport`], such as Ledger's Speculos emulator.
 
 // Client-side device I/O, not runtime code: bounds are checked with explicit
 // length guards before slicing.
@@ -20,6 +24,7 @@ use ledger_apdu::{APDUAnswer, APDUCommand};
 
 use crate::error::CoreError;
 use crate::signers::hid::Transport;
+use crate::signers::ApduTransport;
 
 const CLA: u8 = 0xF9;
 const INS_GET_VERSION: u8 = 0x00;
@@ -50,7 +55,7 @@ pub struct LedgerAddress {
 
 /// One connected Ledger running the Polkadot generic app.
 pub struct LedgerDevice {
-    transport: Transport,
+    transport: Box<dyn ApduTransport>,
 }
 
 impl LedgerDevice {
@@ -60,10 +65,20 @@ impl LedgerDevice {
     /// when the OS denies HID access.
     pub fn open() -> Result<Self, CoreError> {
         Ok(Self {
-            transport: Transport::open()?,
+            transport: Box::new(Transport::open()?),
         })
     }
 
+    /// Drive the generic app over `transport` instead of USB HID.
+    ///
+    /// Requests are built and answers parsed exactly as for a device from
+    /// [`LedgerDevice::open`]; only the channel differs.
+    pub fn with_transport(transport: impl ApduTransport + 'static) -> Self {
+        Self {
+            transport: Box::new(transport),
+        }
+    }
+
     /// The generic app's version, as `(major, minor, patch)`.
     ///
     /// Also serves as the "is the right app open?" probe: any other app
diff --git a/sdk/bittensor-core/src/signers/mod.rs b/sdk/bittensor-core/src/signers/mod.rs
index 549cc2f..89de9bf 100644
--- a/sdk/bittensor-core/src/signers/mod.rs
+++ b/sdk/bittensor-core/src/signers/mod.rs
@@ -3,7 +3,23 @@
 //! Each backend is feature-gated so the default build (and the default
 //! wheel) carries no device-transport native dependencies.
 
+use crate::error::CoreError;
+
 #[cfg(feature = "ledger")]
 mod hid;
 #[cfg(feature = "ledger")]
 pub mod ledger;
+
+/// A blocking channel that carries one APDU to a device and returns its
+/// answer.
+///
+/// `ledger::LedgerDevice::open` uses the built-in USB HID transport;
+/// `ledger::LedgerDevice::with_transport` accepts any other implementation,
+/// such as a TCP client for Ledger's Speculos emulator or a replay of
+/// recorded exchanges, so the APDU logic can run without a device.
+pub trait ApduTransport: Send + Sync {
+    /// One round trip: send the serialized APDU, block until the device
+    /// answers, and return the answer's data followed by its two-byte
+    /// status word.
+    fn exchange(&self, apdu: &[u8]) -> Result<Vec<u8>, CoreError>;
+}
```

### 8.2 The root manifest

The first table alone fails [E]. With `[patch."https://github.com/RaoFoundation/subtensor.git"] bittensor-core =
{ git = <fork>, rev = … }` and the unchanged `[patch.crates-io] w3f-bls` line, the patched core's path dependency
`vendor/w3f-bls` takes the fork's source while `tle` keeps the tag's: `Cargo.lock` holds two `w3f-bls 0.1.3`,
`cargo tree -i w3f-bls` reports the specification as ambiguous, and `bittensor-core` fails with 4 × E0277
(`TinyBLSDrandQuicknet<…>: EngineBLS` not satisfied; "there are multiple different versions of crate `w3f_bls` in the
dependency graph"), the failure of architecture §1.3. Cargo prints no warning for that graph.

| Variant | `w3f-bls` in the lock | Build | Cargo warnings |
|---|---|---|---|
| the core table alone | tag and fork | 4 × E0277 | none |
| also `w3f-bls = { fork, rev }` in the same table | tag and fork | 4 × E0277: patches do not chain into the crates-io patch's replacement | none |
| the `[patch.crates-io] w3f-bls` line pointed at the fork's `rev` | fork only | ok, but the line no longer matches the subnets' line, and a build without the core table must restore it (E0277 otherwise) | none |
| `default-features` and `features` on the core's patch entry | tag and fork | 4 × E0277 | `patch for bittensor-core uses the features mechanism. default-features and features will not take effect …` |
| a second table, `[patch."<fork url>"]`, that sends `w3f-bls` back to the tag | tag only | ok | none |

`subtensor-macros`, the core's other in-repo path dependency, splits the same way once another crate names it from the
tag, as `rtao-core-ext` does. With only `w3f-bls` in the second table, a member that depends on
`subtensor-macros = { git = "https://github.com/RaoFoundation/subtensor.git", tag = "v477" }` builds, but the lock holds
two `subtensor-macros 0.1.0` and `cargo tree -i subtensor-macros` is ambiguous; with `subtensor-macros` in the second
table too, the lock holds one of each, and `bittensor-core` is the only package from the fork [E]. The working tables,
with the fork's GitHub URL and a placeholder in place of the prototype's `file://` URL and local commit:

```toml
[workspace.dependencies]
bittensor-core = { git = "https://github.com/RaoFoundation/subtensor.git", tag = "v477", default-features = false, features = ["ledger"] }

[patch.crates-io]
w3f-bls = { git = "https://github.com/RaoFoundation/subtensor.git", tag = "v477" }

[patch."https://github.com/RaoFoundation/subtensor.git"]
bittensor-core = { git = "https://github.com/sam0x17/subtensor.git", rev = "<full commit>" }

[patch."https://github.com/sam0x17/subtensor.git"]
w3f-bls = { git = "https://github.com/RaoFoundation/subtensor.git", tag = "v477" }
subtensor-macros = { git = "https://github.com/RaoFoundation/subtensor.git", tag = "v477" }
```

Further results [E]:

- The lock entry was `source = "git+file:///work/fork.git?rev=b2919454…#b2919454…"`, so the GitHub fork gives
  `git+https://github.com/sam0x17/subtensor.git?rev=<full commit>#<full commit>`. `cargo tree -i w3f-bls` shows one
  copy, used by `bittensor-core` and `tle`, and Cargo prints no warning.
- The core table matches its URL whatever the reference: with the dependency written as
  `rev = "5c6e83e2f4936f257234f246957f75684c35227c"` instead of `tag = "v477"`, the core still came from the fork. A
  build of monorepo `main` therefore has to drop the tables.
- Cargo fetched the full-commit `rev` from a shallow bare repository. After the patch branch of a copy was deleted and
  the repository pruned, a fresh resolve of the same `rev` failed with `revspec '<rev>' not found; class=Reference (4);
  code=NotFound (-3)`.
- Cargo's git database for the RaoFoundation URL holds the full history (294 MB, 13,406 commits). The fork's URL adds a
  second database of about that size while the patch exists [I]: the prototype's fork was shallow.

### 8.3 Downstream and the unpatched build

[E]

- A separate workspace that depends by git on crates of the patched workspace, with only the `w3f-bls` line, locks
  `bittensor-core`, `subtensor-macros` and `w3f-bls` at `?tag=v477#5c6e83e2…`, with no fork source and no warning:
  Cargo ignores the dependency's `[patch]` tables silently. Its crate that uses only the API before PR-U17 builds; the
  one that imports the trait fails with `error[E0432]: unresolved import bittensor_core::signers::ApduTransport`.
- With both tables deleted and the patched lock kept, as in `core-unpatched`, `--locked` fails with "the lock file …
  needs to be updated but --locked was passed to prevent this". Without it, Cargo locks one package, `bittensor-core`
  at the tag (two, with `subtensor-macros`, when the second table lacks it), and
  `cargo check --workspace --exclude <the crates that name PR-U17's items> --all-features` passes.
- `--exclude` does not keep an excluded crate out of the build when a feature of an included crate depends on it (a
  dev-dependency is not built by `cargo check`), so `core-unpatched` also fails if a shipped feature pulls in
  `rtao-testkit`.
- Deleting only the core table leaves the second table inert and silent; `strip` deletes both anyway.

### 8.4 The checks of `xtask pin --check`

The git half, as commands [E]: `git ls-remote https://github.com/RaoFoundation/subtensor.git refs/tags/v477` (equal to
`core.commit` and to `base`); a bare fetch of the patch branch at a depth of the PR-U17 commits plus one;
`git merge-base --is-ancestor base rev`; `git diff --name-only base rev`; and the `source` line of `bittensor-core` in
`Cargo.lock`.

| Case | `rev` descends from `base` | diff within `paths` | lock source is `repo` at `rev` |
|---|---|---|---|
| the patch branch | yes (1 commit) | yes: `signers/{hid,ledger,mod}.rs` | yes |
| plus a commit touching `vendor/w3f-bls/src/lib.rs` | yes | no | no (the lock pins the old `rev`) |
| the same tree, committed without a parent | no | yes | no |
| a downstream lock (the core from the tag) | yes | yes | no |

The obsolescence probe [E] is a crate outside the workspace, with its own `[workspace]` table and `w3f-bls` line; built
against the fork, it also carries the second table of §8.2, without which `w3f-bls` splits and the core fails with
E0277.

- A generic bound names the trait without a transport type:

  ```rust
  pub fn probe<T>(transport: T) -> bittensor_core::signers::ledger::LedgerDevice
  where
      T: bittensor_core::signers::ApduTransport + 'static,
  {
      bittensor_core::signers::ledger::LedgerDevice::with_transport(transport)
  }
  ```

  Against the unpatched `v477` it fails with `error[E0405]: cannot find trait ApduTransport in module
  bittensor_core::signers`, reported in its own `src/lib.rs` (`--message-format=json` gives the code and the file); a
  `use` of the trait gives E0432 instead. Against the fork it compiles.
- The path first written for `probe`, `bittensor_core::signers::LedgerDevice::with_transport`, fails even against the
  fork (E0412, E0433): the type lives in `signers::ledger`.
- Two root packages of the same name and version in different directories, sharing one `CARGO_TARGET_DIR`, reused each
  other's fingerprints, and one reported a false success until its source was touched. The probe needs its own target
  directory.
- A probe can also fail for the wrong reason (a network error, an E0277 from a `w3f-bls` mistake, a renamed item), so
  only unresolved-name codes from its own source mean "not shipped yet".
- The merge-commit clause was not run: nothing is merged.

### 8.5 Clippy entries

On clippy 1.89, with the two `allow-invalid` entries of architecture §1.8 and the patched core, the entries flag a `use`
of the trait, the trait in `impl` and `dyn` type positions, generic bounds and `where` clauses, and `with_transport`
calls, whether the path is imported or written in full; with `-D warnings` they fail the build [E]. A module-level
`#![allow(clippy::disallowed_methods, clippy::disallowed_types)]` exempts `ledger_emu`. A trait `impl` header written by
full path (`impl bittensor_core::signers::ApduTransport for X`) is not flagged. Against the unpatched core the run is
clean with `allow-invalid`; without it clippy warns that the path "does not refer to a reachable type", and
`-D warnings` does not turn that into an error.

### 8.6 Rebase

`git cherry-pick 5c6e83e2..b2919454` onto upstream `main` at `850f2755` (release 478, where `sdk/bittensor-core` is
unchanged since `v477`) applied cleanly, with the same three paths and the same `git patch-id --stable` [E]. Onto a
synthetic tag that edits the `transport: Transport,` line it stopped with `CONFLICT (content): Merge conflict in
sdk/bittensor-core/src/signers/ledger.rs` (exit 1), the `core-patch-conflict` case, and `--abort` left a clean tree.

### 8.7 The core's `ledger.rs` against the app

The probe owned only a Speculos TCP transport (a `Mutex<TcpStream>`: request `u32 BE length ‖ APDU`, reply `u32 BE
data length ‖ data ‖ SW`, returned as data ‖ SW like the HID transport; about 35 lines, 15 of them the trait impl), a
replay transport and the REST approver of §1. APDU building, `bip44_path`, 250-byte chunking, `APDUAnswer` parsing and
status-word mapping were the core's. Run 1 [E]:

```
[core over speculos] app_version = (100, 0, 29)
[core over speculos] address(0, 0, 42, confirm=false) = e1b4d72d…8419 5HAeMNeM1XzB9ab742XyV8rM4u1wsAUiQQod61n9cjMTrKbn
[core over speculos] address(0, 0, 42, confirm=true) = 5HAeMNeM1XzB9ab742XyV8rM4u1wsAUiQQod61n9cjMTrKbn (approved over REST)
[core over speculos] sign(0, 0, fixture payload, fixture proof) = 00bc772a…d80e
    16 APDUs on the wire: P1=0 Lc=22, 14 × (P1=1 Lc=250), P1=2 Lc=116
[core over speculos] sign with REJECT pressed -> Err("device error: the request was rejected on the device"); SW 6986
[replay] the same 5 calls via LedgerDevice::with_transport(Replay): identical results
[replay] a changed request is caught: device error: replay: request differs from the recording
```

Every value equals §1.2's: the address equals `keys::ss58_from_public(pk, 42)`, the signature verifies with the core's
`keys::verify`, and the INIT APDU and the signature are byte-identical to run B's hand-framed probe. Two runs wrote
identical 35-exchange transcripts. Measured on this host: 8.4 s and 8.6 s for the five calls with three on-screen
reviews; the replay of the five calls took 0.05 ms.

### 8.8 Not executed

Fetching `rev` from a GitHub fork and pushing patch branches; the merge-commit clause; the `cargo generate` template; a
real device (the emulator path runs neither `hid.rs` nor `LedgerDevice::open`); `allow-invalid` for `disallowed-types`
on clippy 1.99.
