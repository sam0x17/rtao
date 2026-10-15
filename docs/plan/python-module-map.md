# Python module map: `sdk/python/bittensor` → rtao

Every file of the v11 Python package at `parity_tag` v477 (`UP/` at `850f2755`) with its rtao home, how rtao obtains
it, and the milestone. It seeds `parity/map.toml` ([architecture §4.9](architecture.md#49-parity-enforcement-the-machinery)):
one row per file, with `py` = the monorepo path, `rs` = the home column, `kind` = the map.toml kind column, and
`synced_blob` set to `pending` in M0 and filled in by `cargo xtask parity bless` as each row is ported. The map also
covers the watched files outside the package ([Other watched roots](#other-watched-roots)), so the seed has 357 rows at
v477 and the file layer passes from M0: it checks presence for every row and blob equality only for blessed rows.
Crate and module names are those of [architecture §2.2](architecture.md#22-crates); paths use the Rust spelling (`rtao_client::…`) for library crates and
the crate name for the `rtao-cli` binary, whose internal layout the architecture does not fix. Operation-level detail
(each intent, read, error code and CLI command) is in [parity-inventory.md](parity-inventory.md).

`§n` refers to [architecture.md](architecture.md); `PY/`-relative line ranges cite the file of the row unless another
file is named; `[I]` marks an inference where the architecture names no home.

## Scope and totals

220 package files: 194 `.py` files (59,723 lines), `namespaces.pyi` (738), `py.typed` (empty), 21
`evm/abi/*.json` (9,040 lines), `extension/static/bridge.html` (628), `vault/static/vault.html` (642) and
`vault/static/bittensor-chain-specs.png` (binary). The `.py` total matches the 59,723 lines architecture §0.4 cites.
Plus 137 other watched files ([Other watched roots](#other-watched-roots)): 357 `map.toml` rows in all.

### Kinds

| Kind (this document) | Meaning | `map.toml` kind |
|---|---|---|
| ported behavior (`ported`) | hand-written Rust mirroring the Python behavior, checked by the oracle (§11.5) | `ported` |
| generated from catalog | produced by `xtask regen` from vendored `CAT/*.json` (§4.1) | `generated` |
| generated from metadata | produced from pinned V15 metadata (§3.7); the Python file is a codegen cross-check (§11.2) | `generated` |
| extracted literal table | read by the Rust-native extractor from the Python (or runtime) source (§4.9 layer 4) | `extracted` |
| vendored asset | copied byte for byte at `parity_tag`, sha256 in `upstream.toml [artifacts]` (§1.2); re-vendored by the pin bump and auto-blessed only when its consumers' tests pass (§4.9) | `vendored` |
| replaced by bittensor-core | the bytes and logic come from the core through `rtao-core` (§1) | `n/a`, or `ported` when a thin seam is mirrored |
| rtao-native equivalent | rtao provides the function with a different design (Rust crate, type, or CLI library) | `ported` if Python behavior must be matched, else `n/a` |
| not applicable | markers, re-export modules, type stubs, terminal theming | `n/a` |

### Per directory

| Directory | Files | Lines | Main rtao homes | Milestones |
|---|---:|---:|---|---|
| `(package root)` | 33 | 13,543 | `rtao-client`, `rtao-tx`, `rtao-primitives`, `rtao-wallet`, `rtao-signer`, `rtao-metagraph`, `rtao-btauth`, `rtao-catalog` | M0–M8 |
| `_generated` | 6 | 2,978 | `rtao-runtime::generated` | M1 |
| `_transport` | 13 | 3,687 | `rtao-rpc`, `rtao-client`, `rtao-core-ext::ops` | M1–M7 |
| `_transport/utils` | 2 | 254 | `rtao-core-ext::ops::receipt` | M2 |
| `cli` | 23 | 13,609 | `rtao-cli` | M4–M8 |
| `cli/commands` | 21 | 7,872 | `rtao-cli` | M4–M8 |
| `cli/commands/evm` | 10 | 1,602 | `rtao-cli` (feature `evm`) | M7 |
| `error_descriptions` | 22 | 2,219 | `rtao-catalog` | M2 |
| `evm` | 8 | 1,243 | `rtao-evm`, `rtao-primitives::evm` | M5–M7 |
| `evm/abi` | 21 | 9,040 | `vendor/upstream/evm/abi/` → `rtao-evm` | M7 |
| `extension` | 9 | 1,226 | `rtao-signer` (feature `extension`), `rtao-cli` | M7 |
| `extension/static` | 1 | 628 | `vendor/` → `rtao-signer` | M7 |
| `intents` | 26 | 8,335 | `rtao-intents`, `rtao-tx`, `rtao-catalog` | M2–M5 |
| `reads` | 17 | 3,118 | `rtao-reads` | M1–M6 |
| `vault` | 6 | 775 | `rtao-signer` (feature `vault`) | M7 |
| `vault/static` | 2 | 642 | `vendor/` → `rtao-signer` | M7 |
| **total** | **220** | **70,771** | | |

`map.toml` kinds across the 220 package rows: `extracted` 6, `vendored` 24, `generated` 29, `n/a` 18, `ported` 143.

## Package root

Library modules of the SDK proper.

| Python file | Lines | rtao home | Kind | `map.toml` | M | Notes |
|---|---:|---|---|---|---|---|
| `__init__.py` | 276 | `rtao` facade: `prelude`, per-feature re-exports (§2.4) | rtao-native | `ported` | M0–M8 | public names (`Subtensor`, `Client`, intents, reads, `Balance`, signers, `set_weights`); every `__all__` name (`:122-219`) has a row in [Top-level exports](#top-level-exports-py__init__py122-219-__all__), and `xtask parity check` asserts it or a `#[doc(alias)]` hit (§4.9 layer 8); `_REMOVED_V10_HINTS` (`:238-276`) is n/a (migration guide) |
| `_live.py` | 81 | `rtao-cli` prompt module (PLAN §12.6) | rtao-native | `n/a` | M4 | Rich live-display pause used by `keyfiles.py:9`, `masked_input.py:18`, `cli/output.py:31`; the library never prompts (D-003), so only the CLI's prompt module needs the pause |
| `_substrate.py` | 851 | `rtao_client::{Client,Snapshot,View,DynView,ClientError}`; `rtao_tx::{Executor,ExtrinsicResult}`; `rtao_rpc::Session` | ported | `ported` | M1–M2 | Typed read/write ownership (§3.5): View is read-only, Client owns RPC/nonce/signing and Executor lifts write results; injected RPC supplies the backend for write fixtures. Public `submit_signed_raw` is pool/hash-only; external watched signing requires authenticated `UnsignedExtrinsic` plus `MultiSignature` (§3.9.3, §3.14). Receipt lookup returns index/report through separate client methods. Archive routing, exact public diagnostic projection, native chain balances and D-023 event retention are reviewed source relationships; current hosted/Python/oracle execution remains unclaimed. |
| `_subtensor.py` | 205 | `rtao::blocking::{Subtensor, set_weights, close_shared_clients}` (facade, feature `blocking`, §2.2, §2.4) | ported | `ported` | M6 | sync-or-async entry point; module-level `set_weights` builds `SetWeights` and resolves wallet names (`:152-158, 167-205`), so the facade feature enables `intents-all`, `metagraph` and `wallet`; shared clients keyed by canonical endpoint (`:129-150`) |
| `balance.py` | 222 | `rtao_primitives::{DynBalance, Balance<U>, pyfmt}` | ported | `ported` | M0 | `str(Balance)` byte for byte (`:113-149`, §5.3); pinned by 10,000 oracle vectors (§11.5) |
| `basket_index.py` | 361 | `rtao-cli` `root_helpers::basket_index` | ported | `ported` | M6 | CLI-only display pricing for basket funds, and the fallback for nodes without `BetaBasketRuntimeApi.get_all_beta_pricing` (API v3) and for historical blocks; `root list --json` emits `basket_index` through `index_level()` (`cli/commands/root.py:23, 156, 240`) and `cli/root_helpers.py:18` uses `staker_yield`, so it cannot be dropped; not in `rtao-catalog`, which every neuron build compiles |
| `basket_index_data.py` | 975 | `rtao-cli/src/generated/basket_index_data.rs`, generated by `xtask regen` from the extracted literal tuples (`vendor/upstream/extracted/basket_index_data.json`) | extracted literal table | `extracted` | M6 | frozen index series and per-fund baselines, regenerated upstream by `scripts/build_basket_index.py`; pure literal tuples (6/6 tables pass the §4.9 subset) |
| `client.py` | 688 | `rtao_client::Client` (incl. `decode_scale`, `estimate_weight`, `estimate_shielded_carrier_fee`, `block_info`; `BlockHeader`, `BlockInfo`, `EpochEvent`); `rtao_tx::Executor::{execute_tool, multisig}`; `rtao_intents::pow::PowSolver` | ported | `ported` | M1–M5 | `wait_for_*` `:376-444` (§3.13); token symbols at connect `:613-633` (§3.3 step 6); `execute_tool` `:483-488` (§4.6); `decode_scale` `:264-271`, `estimate_shielded_carrier_fee` `:464-466`, `estimate_weight` `:554-572`, `multisig` `:564-590` (§3.10, §4.6; M4); `block_info` `:349` (M5, `multisig pending`); dataclasses `:74-115` (§3.13); `mine_pow_registration` → `PowSolver` (§9.6) |
| `config.py` | 480 | `rtao_client::config` (M1): the seven btcli files with their environment overrides, `SETTABLE`, `btcli.json` load and save, the token-symbol and subnet-name caches; `rtao-cli` `books` (readers M4; the multisig book and call-cache writers M5; the proxy and address books M8) | ported | `ported` | M1–M8 | paths `:12-18`; overrides `:59, 139, 212, 274, 327, 355, 416`; TTLs `:22-27`; `SETTABLE` `:30-56`, extracted with the builtin type tags of §4.9 layer 4; multisig call cache `:326-352, 470-480` (§10.2); `NeuronConfig` precedence (§9.9) |
| `error_map.py` | 795 | `rtao_catalog::{ErrorCode, CHAIN_ERRORS}` (generated) + `rtao_catalog::py` (extracted); `rtao_tx::classify` | generated from catalog + extracted literal table | `extracted` | M2 | `ErrorCode` `:23-41` and `NAME_TO_CODE` `:46` (390) equal the catalog; `CUSTOM_TRANSACTION_ERRORS` `:488`, `INVALID_TRANSACTION_ERRORS` `:524`, `DISPATCH_ERRORS` `:555` extracted (§4.1, §4.7) |
| `executor.py` | 1,319 | `rtao_tx::{Executor, Plan, ExecuteOptions}`; `rtao_client::{shield, compose}`; completion follower | ported | `ported` | M2–M5 | defaults `:782-788`; `nest_origin_wrappers` `:99-121, 175-219` (§3.12); shield `:897-1214` (§3.11, M4); `_complete_subnet_registration` `:333-568` (§3.13, M4); constants `:45-66` extracted |
| `fee_filters.py` | 98 | `rtao_catalog::rt` (coldkey-pays-fee set) + `rtao_tx` plan step 6 | extracted literal table | `extracted` | M2 | mirror of `UP/runtime/src/fee_filters.rs:15-34`, which `syn` extracts as the authority; CI asserts this mirror equals it (§3.10, §4.1) |
| `http_auth.py` | 369 | `rtao_btauth` (core; `server`, `client` features) | ported | `ported` | M0–M3 | btauth/1 byte for byte (§7); messages `:274-341` are the wire contract; retention check moves to `Verifier::new` (D-009) |
| `hyperparams.py` | 769 | `rtao_primitives::ratio::{ratio_fraction, RATIO_TYPE_DENOMINATORS}` (derived from the §5.4 ratio types; a catalog-layer test asserts equality with the extracted `RATIO_TYPE_DENOMINATORS` `:32-37`) and `rtao_primitives::hyperparams::proportion_to_raw` (`:581-621`) (M1); `rtao_primitives::hyperparams::{Hyperparam, HyperparamTable}`, with `metadata_kind`, `kind_of`, `doc_of`, `short_of`, `normalized`, `annotate`, `value_forms`, `codec_of`, `raw_bounds` and `to_raw` as methods on `&HyperparamTable` (`:474-769`) (M4); data generated into `rtao_catalog::hyperparams::TABLE` from `HYPERPARAMS` `:110-408`, `STORAGE_ITEMS` `:416-461` (each item resolved at extraction to its value type ident and cross-checked against the pinned metadata), `_IDENT_KINDS` `:467-471`, `_IDENT_CODEC` `:626-643`, `_KIND_CODEC` `:646-657` and `_FRACTION_DENOMINATOR` `:83`; `rtao_intents::hyperparameters::to_raw` delegates to `TABLE` (§5.5) | extracted literal table + ported | `extracted` | M1–M4 | metadata-primary `kind_of`/`codec_of` precedence kept (`:474-500, 663-680`); consumers before M4: `metagraph.py:346` (M1), `reads/weights.py:73` (M2), `reads/delegation.py:25-28`, `intents/children.py:29, 80` (M5) |
| `keyfiles.py` | 217 | `rtao_wallet::{HotkeyFile, ColdkeyFile}` over core `keyfiles` and staged `keyfile_json` | ported | `ported` | M2 | `BT_PW_…` name rule (§6.2); 0o600 atomic writes `:128-163` (§6.1); TTY prompt rule `:42-62` applies to the CLI only (D-003) |
| `ledger.py` | 181 | `rtao_signer::LedgerSigner` (feature `ledger`) over core `signers::ledger::LedgerDevice` | ported | `ported` | M7 | §6.5; proof from `digest::generate_extrinsic_proof` |
| `macos_password.py` | 143 | `rtao_wallet` feature `keychain` | ported | `ported` | M4 | shells out to `/usr/bin/security` exactly as `:58-80` (§6.2), and the `osascript` dialog: password-chain steps 5-6 behind `--keychain-password`/`--macos-password` in CLI v0, with the Keychain test in the macos-14 `check` job (§11.6 lane 3); the `wallet keychain` commands over it land in M7 |
| `masked_input.py` | 147 | `rtao-cli` prompt module (`rpassword` hidden entry under `PromptPolicy::IfTty`; PLAN §12.6) | rtao-native | `n/a` | M4 | re-asks on empty or mismatched input as `keyfiles.py:42-62`; the per-character `*` echo is rendering and not gated; rtao's library never prompts (D-003) |
| `metagraph.py` | 573 | `rtao_metagraph::{Metagraph, MetagraphNeuron, MetagraphView, SubnetsMetagraphExt}`; the commitment parts → `rtao_reads::commitments` | ported | `ported` | M1 | `fetch` `:238-291` (curated `client.subnets().metagraph(..)` through `SubnetsMetagraphExt`, §8.2); `_build` `:297-403` → `from_dynamic` reference path; `NeuronCommitment`, the commitment map, status machine and decoders `:37-125, 406-560` and `fetch_commitments`/`_fetch_commitment_map` `:258-291` → `rtao_reads::commitments` (§4.8), whose `reveals_at` uses `rtao_primitives::drand::reveal_time` (`:25, 120-124`); axon URL `:563-573` → `rtao_primitives::axon::axon_base_url` (§7.3) |
| `multisig.py` | 319 | `rtao_tx::multisig::{Multisig, ApproveOptions}` and the funds preflight; `rtao_client::compose::{as_multi, approve_as_multi}` | ported | `ported` | M4 | `Multisig.approve` `:166-313` (§4.6; `rtao call --multisig` is CLI v0); `MultisigFundingError` `:32-49` → `ChainError { code: InsufficientBalance }` with `fund_help` remediation; deposit and `_FEE_ALLOWANCE_RAO` `:121-162` extracted (§3.12); the M5 multisig intents reuse the preflight; tests port `PYT/unit/test_multisig_safety.py` |
| `namespaces.py` | 294 | `rtao_reads` namespace types (14) and curated extras | generated from catalog + ported | `ported` | M1–M6 | one accessor per catalog `category` (§4.8); curated `:99-213` hand-written; curated `Subnets::commitments` in `rtao_reads::commitments`, curated `Subnets::metagraph` as `rtao_metagraph::SubnetsMetagraphExt` in the prelude (no inherent `Subnets::metagraph` is generated, `:161-188`); the catalog `metagraph` read stays the raw `get_metagraph` dict (`PY/reads/subnets.py:170-190`); the generated `Subnets` accessor lands in M1 |
| `namespaces.pyi` | 738 | none needed | not applicable | `n/a` | — | type stub generated by `python -m codegen.emit_namespaces` (`namespaces.py:14-16`); Rust's typed namespace methods carry the same information |
| `pow_registration.py` | 255 | `rtao_intents::pow::PowSolver` (feature `pow`; `pow-gpu`) | ported | `ported` | M4–M7 | `:18-253` (§9.6); seal and mining from core `pow`; prefix and seal are pure-oracle functions (§11.5); GPU in M7 |
| `py.typed` | 0 | none needed | not applicable | `n/a` | — | PEP 561 marker (empty) |
| `result.py` | 750 | `rtao_tx::{ExtrinsicResult, ChainError, TxError, PolicyError, classify}` | ported + extracted literal table | `ported` | M2 | `:697-749`, `:501-562`, `:45-73`, precedence `:483-498`, substrate-request parsing `:574-634` (§4.7); `REMEDIATION` `:78-111` equals the catalog; `EXPLANATIONS` `:116-287` and regexes `:27-36` extracted; nested module errors `:636-656` resolve through the block's runtime in rtao (D-028); `_CATALOG_DOCS_BY_NAME` `:35` is derived from `MODULE_ERRORS` (§4.7) |
| `settings.py` | 220 | `rtao_catalog::py` + `rtao-rpc/src/generated/py_rpc.rs` | extracted literal table | `extracted` | M1 | network pools (§3.2.4), era default `:22`, shield period `:29`, `GLOBAL_MAX_SUBNET_COUNT` `:31`, `RAO_PER_TAO` `:15`, docs and explorer templates; "no Python constant is hand-copied" (§4.1); `RAO_PER_TAO = 10**9` and the `os.getenv(…) or …` endpoints resolve through §4.9 layer 4's resolved forms |
| `signing.py` | 283 | `rtao_signer::{Signer, SignerRole, KeyView, DynKeySource}`; lazy unlock in `rtao_wallet`; address coercion in `rtao_tx` | ported | `ported` | M2 | `resolve_signer` `:227-263`, `public_view` `:266-283` (§6.4); `WalletSigner` `:143-224` (§6.1) |
| `snapshot.py` | 115 | `rtao_client::Snapshot` | ported | `ported` | M1 | §3.5; constants follow the snapshot runtime, not `:66-68` (D-007) |
| `sp_core.py` | 40 | `rtao_core::keys` (re-exports) | replaced by bittensor-core | `n/a` | M0 | thin binding over the core; `verify` reaches `CORE/keys/mod.rs:457-469` (§7.1) |
| `sync.py` | 479 | `rtao::blocking::{SyncClient, SyncSnapshot}` (facade, feature `blocking`) over `rtao_client::blocking::BlockingRuntime` | ported | `ported` | M6 | private loop `_Loop` `:64-98` → `BlockingRuntime` (§3.1); wraps reads, `plan`/`execute`/`execute_tool`/`submit_shielded`/`submit_call`/`tools`/`multisig` and `mine_pow_registration` (`:401-476`; the last under feature `pow`); all 14 namespaces where Python's `SyncClient` has 13 (D-027) |
| `timelock.py` | 392 | `rtao_tx::timelock::{seal, open, Timelocked, Reveal, OpenOptions, TimelockError, DrandClient}` over core `timelock`; the pure helpers `current_round`, `round_at`, `reveal_time`, `parse_duration`, `format_duration` → `rtao_primitives::drand` | ported | `ported` | M1 (helpers), M7 | the whole `__all__` `:380-392` (§9.8); `NeuronCommitment.reveals_at` needs `reveal_time` in M1; `Reveal::InBlocks` is an rtao addition (D-033); weight commits use `TimelockCommitter` (M2), not this module |
| `wallet.py` | 366 | `rtao_wallet::Wallet` | ported | `ported` | M2 | layout `:63-68`; `DEFAULT_WALLET_PATH` `:11`; reads no environment variable (`BT_WALLET_PATH` is the CLI's `--wallet-path` option, `PY/cli/main.py:203-206`; §6.1); `cold/hot` parsing `:29-70` |
| `wallets.py` | 542 | `rtao_wallet` (passwords, create/regen, listing) | ported | `ported` | M2–M8 | `resolve_wallet_password` `:62-118` (§6.2); hotkeys written without a password `:239-245`; message sign/verify/encrypt serve CLI commands of M8 |

## `_generated`

Python's metadata codegen output. rtao does not translate these files; it generates the equivalents from the same pinned metadata and cross-checks them (§3.7, §11.2).

| Python file | Lines | rtao home | Kind | `map.toml` | M | Notes |
|---|---:|---|---|---|---|---|
| `_generated/__init__.py` | 1 | none needed | not applicable | `n/a` | — | empty package marker |
| `_generated/calls.py` | 1,704 | `rtao_runtime::generated::calls` | generated from metadata | `generated` | M1 | rtao regenerates from pinned V15 metadata; this file is the codegen cross-check (§11.2); typed call builders and `CallDescriptor`s (§3.7-3.8) |
| `_generated/constants.py` | 187 | `rtao_runtime::generated::constants` | generated from metadata | `generated` | M1 | rtao regenerates from pinned V15 metadata; this file is the codegen cross-check (§11.2) |
| `_generated/errors.py` | 420 | `rtao_runtime::generated::errors::MODULE_ERRORS` | generated from metadata | `generated` | M1 | rtao regenerates from pinned V15 metadata; this file is the codegen cross-check (§11.2); docs and gates only (§4.7) |
| `_generated/runtime_apis.py` | 189 | `rtao_runtime::generated::runtime_apis` | generated from metadata | `generated` | M1 | rtao regenerates from pinned V15 metadata; this file is the codegen cross-check (§11.2) |
| `_generated/storage.py` | 477 | `rtao_runtime::generated::storage` | generated from metadata | `generated` | M1 | rtao regenerates from pinned V15 metadata; this file is the codegen cross-check (§11.2) |

## `_transport`

The behavioral reference for rtao's transport and chain client (3,941 lines, §3).

| Python file | Lines | rtao home | Kind | `map.toml` | M | Notes |
|---|---:|---|---|---|---|---|
| `_transport/__init__.py` | 39 | none needed | not applicable | `n/a` | — | re-exports only |
| `_transport/codec.py` | 520 | `rtao_runtime::RuntimeHandle` over core `runtime::Runtime` + `rtao_client::RuntimeCache`; staged `ops::metadata` | replaced by bittensor-core (thin seam ported) | `ported` | M1 | the codec engine is the core (§1.4-1.5); `RuntimeHandle` lives in `rtao-runtime` (§3.7); `strip_option_opaque_metadata` matches §3.4 fetch step 1 |
| `_transport/const.py` | 2 | core `DEFAULT_SS58_FORMAT` (`CORE/keys/mod.rs:31`) | replaced by bittensor-core | `n/a` | M1 | `SS58_FORMAT = 42`; connect asserts format 42 (§3.3 step 3) |
| `_transport/contract.py` | 245 | `rtao_runtime::CallBytes`; `rtao_client::{UnsignedExtrinsic, ExtrinsicReport}`; `rtao_signer::{SigningContext, SignerPayloadJson}` | ported | `ported` | M2–M7 | `UnsignedExtrinsic` JSON `:67-134` (§3.14, M5); `SignerPayloadJSON` quirks `:87-132` (§6.3, extension signer M7) |
| `_transport/errors.py` | 80 | `rtao_rpc::{RpcError, PolicyRefusal}`; `rtao_client` read errors | ported | `ported` | M1 | §3.2.3; `RpcPolicyRejection`, `StateDiscardedError`, `MaxRetriesExceeded` |
| `_transport/extrinsics.py` | 426 | `rtao_client::{NonceManager, NonceLease}`; `rtao_signer` dispatch | ported | `ported` | M2 | nonce cache `:55-84` (§3.9.1); `account_nextIndex` `:69-77`; payload JSON `:87-132`; signer capability order `:321-347` (§6.3) |
| `_transport/interface.py` | 810 | `rtao_client::{Client,Snapshot,View,ViewExt}`; full-call composer; `ops::{fee,extrinsic_lookup}` | ported | `ported` | M1–M5 | Complete source review; native cache, pinned stream, typed preparation/submission and lookup contracts are explicit in architecture sections 3.5, 3.6 and 3.9. Map presence is factored through public metadata/key-prefix/one-key primitives, without a dedicated wrapper or complete probe fixture. Private page/record/tolerant modes and exact request behavior are different APIs; null-pruning rows and current hosted execution remain pending. |
| `_transport/lru.py` | 25 | `rtao_client` runtime cache (LRU of 4 runtimes, 512 block→spec entries, §3.4) | rtao-native | `n/a` | M1 | generic LRU helper |
| `_transport/protocols.py` | 91 | `rtao_signer::SignerCaps` | rtao-native | `ported` | M2–M7 | `Keypair`, `ExtensionPayloadSigner`, `MetadataVerifyingSigner`, `UnsignedExtrinsicSigner` become the `Signer` trait plus capability bits `RAW_PAYLOAD`, `PAYLOAD_JSON`, `METADATA_HASH`, `UNSIGNED_TX` (§6.3) |
| `_transport/rpc.py` | 812 | `rtao_rpc::Session`, handshake, policy and subscription modules; generated `py_rpc.rs` | ported + extracted literal table | `ported` | M1 | Complete source review; terminal policy includes paced reconnect setup. HTTP refusal diagnostics retain TLS state with bounded prefix capture under the original deadline (§3.2.3). Typed IDs/batches, replay/fate rules, queue budgets and inherited pre-ack cancellation limits remain explicit. Source reconciliation does not establish complete pytest/oracle or current hosted execution. |
| `_transport/runtime.py` | 310 | `rtao_client::RuntimeCache` | ported | `ported` | M1 | V15 fetch and needle `:34, 292-312`; disk cache `:36-121`; parent-runtime resolution `:178-207`; version poll `:142-176`; `transactionVersion` `:42-54` (§3.4) |
| `_transport/runtime_api.py` | 242 | staged `rtao_core_ext::ops::runtime_api`; `rtao_client` legacy table | ported | `ported` | M1–M6 | encoding (§1.6 G1); `LEGACY_RUNTIME_APIS` `:95-171` and `_is_legacy_method` `:167` (§3.4; legacy table in M6) |
| `_transport/storage.py` | 85 | `rtao_client` reads; staged `ops::storage` | ported | `ported` | M1 | miss semantics `:8-27` (§3.6) |

## `_transport/utils`

| Python file | Lines | rtao home | Kind | `map.toml` | M | Notes |
|---|---:|---|---|---|---|---|
| `_transport/utils/__init__.py` | 17 | none needed | not applicable | `n/a` | — | JSON library selection |
| `_transport/utils/receipt.py` | 237 | staged `rtao_core_ext::ops::receipt` (PR-U2) | ported | `ported` | M2 | 6 nested wrappers `:8-16`, MevShield failures, fee fallback `:75-96` (§1.6 G3, §3.9.4) |

## `cli`

Shared CLI machinery. Crate `rtao-cli`, binary `rtao` (§10).

| Python file | Lines | rtao home | Kind | `map.toml` | M | Notes |
|---|---:|---|---|---|---|---|
| `cli/__init__.py` | 5 | none needed | not applicable | `n/a` | — |  |
| `cli/call.py` | 597 | `rtao-cli` `call` | ported | `ported` | M4 | raw call; the CLI builds its executor without a `Policy`, as btcli does (`cli/context.py:1990-1994`), so there is no `--allow-raw` flag; same wrapper nesting as intents (§10.1) |
| `cli/call_names.py` | 240 | `rtao-cli` name resolution inside JSON arguments [I] | ported | `ported` | M4 | address-book, proxy-book, multisig and wallet names inside `call --args` and nested intent specs; used by `cli/call.py`, `cli/tx.py` |
| `cli/context.py` | 2,140 | `rtao-cli` application context | ported | `ported` | M4 | `address_cli_name` (§10.1); MEV precedence `:260-292`, downgrade `:1047-1080` (§3.11); re-prompt `:1900-1935` (§6.2); non-interactive discipline `:2088-2103` (§10.3) |
| `cli/globals.py` | 542 | `rtao-cli` global flags | ported | `ported` | M4 | §10.2 |
| `cli/help_theme.py` | 48 | `rtao-cli` clap styles | not applicable | `n/a` | M8 | Rich/Typer colours; human output is not byte-identical by design (§10.3) |
| `cli/helpers.py` | 813 | `rtao-cli` shared helpers | ported | `ported` | M4–M8 | `wallet balance`/`inspect` rows (M4); overview and registration rows (M8); its read calls set several read milestones ([parity-inventory.md section 2.4](parity-inventory.md#24-read-milestones)) |
| `cli/hyperparams_view.py` | 151 | `rtao-cli` `hparams get`, `subnets hyperparameters` | ported | `ported` | M4–M6 | §10.5 |
| `cli/intent_prompts.py` | 177 | `rtao-cli` generated-`tx` prompt policies [I] | ported | `ported` | M4 | imported by `cli/tx.py` |
| `cli/logs.py` | 103 | `rtao-cli` logging (`tracing-subscriber`) | rtao-native | `ported` | M4 | verbosity ladder and `BITTENSOR_LOG` (§10.3) |
| `cli/main.py` | 549 | `rtao-cli` entry point and command tree | ported | `ported` | M4 | groups `:99-144`; hidden aliases `:116-135`; snake_case aliases `:449-477`; `tools` `:321`, `explain` `:405` (§10.1-10.2) |
| `cli/metagraph_view.py` | 211 | `rtao-cli` `subnets metagraph` | ported | `ported` | M4 | §10.5 |
| `cli/multisig_helpers.py` | 1,455 | `rtao-cli` `multisig` | ported | `ported` | M5 | saved multisigs, approvals, pending operations (§10.5) |
| `cli/null_pruning.py` | 392 | `rtao-cli` `hparams trim-null`; `rtao_neuron::OwnerOps::trim_null` | ported | `ported` | M4 | resumable ≤ 64-uid batches (§9.7) |
| `cli/output.py` | 2,484 | `rtao-cli` output (tables via `comfy-table`, JSON) | ported | `ported` | M4–M8 | JSON value-equal from M4; `PyJsonFormatter` for `print_json` `:562` in M8 (§10.3) |
| `cli/prompt.py` | 718 | `rtao-cli` prompts and pickers | ported | `ported` | M4 | prompts and flags match, not terminal rendering (§10.3) |
| `cli/query.py` | 266 | `rtao-cli` generated `query`; `_jsonable` semantics in `rtao_reads` `Serialize` models | generated from catalog (at run time) + ported | `ported` | M4–M6 | `build_query_app` `:258`; `_jsonable` `:104-116` defines read JSON (§4.8, §10.1) |
| `cli/root_helpers.py` | 526 | `rtao-cli` `root` | ported | `ported` | M6 | §10.5; imports `basket_index` |
| `cli/secrets.py` | 75 | `rtao-cli` secret handling [I] | ported | `ported` | M4 | argv-secret warnings and clipboard copy; used by `wallet regen-*` (`cli/commands/wallet.py:57, 461, 530`) |
| `cli/stake_exit.py` | 222 | `rtao-cli` `stake unstake-all`, `unstake-all-alpha` | ported | `ported` | M8 | `mount_stake_exit_commands` `:212-222` |
| `cli/stake_picker.py` | 676 | `rtao-cli` `stake wizard` and pickers | ported | `ported` | M8 | §10.6 M8 |
| `cli/tx.py` | 498 | `rtao-cli` generated `tx` | generated from catalog (at run time) + ported | `ported` | M4–M5 | clap builder from `rtao_catalog::INTENTS` (§10.1; `:1-11, 486-498`); list parsing `:62-82`; help panels `:470-484` |
| `cli/upgrade_helpers.py` | 721 | `rtao-cli` `upgrade` | ported | `ported` | M8 |  |

## `cli/commands`

Hand-written command groups (§10.5); per-command milestones are in [parity-inventory.md section 7](parity-inventory.md#7-cli-command-tree).

| Python file | Lines | rtao home | Kind | `map.toml` | M | Notes |
|---|---:|---|---|---|---|---|
| `cli/commands/__init__.py` | 0 | none needed | not applicable | `n/a` | — | empty |
| `cli/commands/addresses.py` | 271 | `rtao-cli` `addr` group | ported | `ported` | M8 | §10.5 |
| `cli/commands/axon.py` | 68 | `rtao-cli` `axon` group | ported | `ported` | M4 | §10.5; under `misc` |
| `cli/commands/collateral.py` | 171 | `rtao-cli` `collateral` group | ported | `ported` | M6 | §10.5; under `misc` |
| `cli/commands/config.py` | 94 | `rtao-cli` `config` group | ported | `ported` | M4 | §10.5 |
| `cli/commands/crowd.py` | 58 | `rtao-cli` `crowd` group | ported | `ported` | M8 | §10.5; under `misc` |
| `cli/commands/extension.py` | 120 | `rtao-cli` `extension` group | ported | `ported` | M7 | §10.5; under `misc`; feature `extension` |
| `cli/commands/lock.py` | 330 | `rtao-cli` `conviction` group | ported | `ported` | M6 | §10.5; alias `lock` |
| `cli/commands/misc.py` | 204 | `rtao-cli` `misc` group | ported | `ported` | M8 | §10.5; also mounts the operator groups `:42-51` |
| `cli/commands/multisig.py` | 344 | `rtao-cli` `multisig` group | ported | `ported` | M5 | §10.5 |
| `cli/commands/proxy.py` | 498 | `rtao-cli` `proxy` group | ported | `ported` | M5–M8 | §10.5; `book` subgroup in M8 |
| `cli/commands/root.py` | 708 | `rtao-cli` `root` group | ported | `ported` | M6 | §10.5 |
| `cli/commands/root_move.py` | 213 | `rtao-cli` `root` group | ported | `ported` | M6 | §10.5; `root move` command, mounted at `root.py:682` |
| `cli/commands/root_swap.py` | 219 | `rtao-cli` `root` group | ported | `ported` | M6 | §10.5; `root swap` command, mounted at `root.py:684` |
| `cli/commands/stake.py` | 446 | `rtao-cli` `stake` group | ported | `ported` | M5–M8 | §10.5; aliases M5, the rest M8 |
| `cli/commands/subnets.py` | 634 | `rtao-cli` `subnets` group | ported | `ported` | M4–M6 | §10.5; `register`, `create`, `metagraph` in M4 |
| `cli/commands/sudo.py` | 528 | `rtao-cli` `hparams` group | ported | `ported` | M4–M5 | §10.5; `set-take`, `stake-burn` in M5 |
| `cli/commands/timelock.py` | 336 | `rtao-cli` `timelock` group | ported | `ported` | M7 | §10.5; under `misc` |
| `cli/commands/upgrade.py` | 651 | `rtao-cli` `upgrade` group | ported | `ported` | M8 | §10.5; under `misc` |
| `cli/commands/wallet.py` | 1,784 | `rtao-cli` `wallet` group | ported | `ported` | M4–M8 | §10.5; keychain M7 |
| `cli/commands/weights.py` | 195 | `rtao-cli` `weights` group | ported | `ported` | M4 | §10.5; under `misc` |

## `cli/commands/evm`

| Python file | Lines | rtao home | Kind | `map.toml` | M | Notes |
|---|---:|---|---|---|---|---|
| `cli/commands/evm/__init__.py` | 53 | `rtao_cli::commands::evm` | ported | `ported` | M7 | Complete group registrations and generated claim-deposit alias reviewed; native Clap/feature-gated routing. The 25-route fixture establishes source surface, with execution/terminal limits unclaimed. |
| `cli/commands/evm/_shared.py` | 287 | `rtao_cli::commands::evm` and `evm::keys` | ported | `ported` | M7 | Complete shared source reviewed: public metadata, password precedence, empty endpoint fallback, typed RPC, preview/confirm/late unlock and recovery. Native u64/parser/security/output boundaries remain; repair fixtures authored, unexecuted. |
| `cli/commands/evm/address.py` | 90 | `rtao_cli::commands::evm` | ported | `ported` | M7 | Three public-only routes preserve hash/truncation and exact machine fields; native AccountId display is canonical prefix 42. Complete source reviewed; dedicated CLI output execution remains unverified. |
| `cli/commands/evm/association.py` | 55 | `rtao_cli::commands::evm::association` | ported | `ported` | M7 | Complete proof-to-intent source reviewed; D-051 explicit consent, effective dispatch identity and checked arguments remain. Real signer/network execution is unverified. |
| `cli/commands/evm/contracts_cmd.py` | 68 | `rtao_cli::commands::evm::contracts` | ported | `ported` | M7 | Complete deploy source reviewed; empty attached value means zero. Injected artifact/calldata/value fixtures retain dry-run, confirmation and checked native bounds; real deployment is unverified. |
| `cli/commands/evm/keys.py` | 239 | `rtao_cli::commands::evm::keys` | ported | `ported` | M7 | Complete key handlers reviewed; empty export destination selects stdout, with nonempty raw destinations refused. Existing secret/storage protections remain; real clipboard/KDF execution is unverified. |
| `cli/commands/evm/money.py` | 147 | `rtao_cli::commands::evm` | ported | `ported` | M7 | Balance, native fund intent and EVM sends reviewed; --all companions normalize through shared admission/request code. Exact native monetary policy and shared signing discipline remain. Source fixtures do not establish full command execution. |
| `cli/commands/evm/precompiles_cmd.py` | 228 | `rtao_cli::commands::evm::precompiles` | ported | `ported` | M7 | Complete ABI/read/write source reviewed; empty attached value means zero, empty custom ABI refuses as missing. Injected fixtures retain view isolation and native bounds; real signed calls are unverified. |
| `cli/commands/evm/setup.py` | 300 | `rtao_cli::commands::evm::setup` | ported | `ported` | M7 | Complete networks/config/doctor/setup source reviewed. Native exact-call consent and sequential recovery remain; setup configures a selected node without starting infrastructure. Real RPC/Sudo execution is unverified. |
| `cli/commands/evm/stake.py` | 135 | `rtao_cli::commands::evm` | ported | `ported` | M7 | Actual staking-v2 ABI/JSON and native u64/netuid behavior reviewed; alpha removal rejects root, root add remains. Injected write fixtures assert exact words and no private access; show/live execution remains unverified. |

## `error_descriptions`

Per-pallet descriptions of the 390 classified chain errors. rtao reads the same text from `CAT/errors.json`.

| Python file | Lines | rtao home | Kind | `map.toml` | M | Notes |
|---|---:|---|---|---|---|---|
| `error_descriptions/__init__.py` | 66 | `rtao_catalog::CHAIN_ERRORS` (description field) | generated from catalog | `generated` | M2 | merged `DESCRIPTIONS` equals `CAT/errors.json` `chain_errors[*].description` (390/390, verified) |
| `error_descriptions/admin_utils.py` | 82 | `rtao_catalog::CHAIN_ERRORS` (description field) | generated from catalog | `generated` | M2 | per-pallet slice of the 390 descriptions |
| `error_descriptions/balances.py` | 69 | `rtao_catalog::CHAIN_ERRORS` (description field) | generated from catalog | `generated` | M2 | per-pallet slice of the 390 descriptions |
| `error_descriptions/commitments.py` | 31 | `rtao_catalog::CHAIN_ERRORS` (description field) | generated from catalog | `generated` | M2 | per-pallet slice of the 390 descriptions |
| `error_descriptions/contracts.py` | 193 | `rtao_catalog::CHAIN_ERRORS` (description field) | generated from catalog | `generated` | M2 | per-pallet slice of the 390 descriptions |
| `error_descriptions/crowdloan.py` | 140 | `rtao_catalog::CHAIN_ERRORS` (description field) | generated from catalog | `generated` | M2 | per-pallet slice of the 390 descriptions |
| `error_descriptions/drand.py` | 36 | `rtao_catalog::CHAIN_ERRORS` (description field) | generated from catalog | `generated` | M2 | per-pallet slice of the 390 descriptions |
| `error_descriptions/ethereum.py` | 18 | `rtao_catalog::CHAIN_ERRORS` (description field) | generated from catalog | `generated` | M2 | per-pallet slice of the 390 descriptions |
| `error_descriptions/evm.py` | 77 | `rtao_catalog::CHAIN_ERRORS` (description field) | generated from catalog | `generated` | M2 | per-pallet slice of the 390 descriptions |
| `error_descriptions/grandpa.py` | 43 | `rtao_catalog::CHAIN_ERRORS` (description field) | generated from catalog | `generated` | M2 | per-pallet slice of the 390 descriptions |
| `error_descriptions/limit_orders.py` | 135 | `rtao_catalog::CHAIN_ERRORS` (description field) | generated from catalog | `generated` | M2 | per-pallet slice of the 390 descriptions |
| `error_descriptions/mev_shield.py` | 26 | `rtao_catalog::CHAIN_ERRORS` (description field) | generated from catalog | `generated` | M2 | per-pallet slice of the 390 descriptions |
| `error_descriptions/multisig.py` | 76 | `rtao_catalog::CHAIN_ERRORS` (description field) | generated from catalog | `generated` | M2 | per-pallet slice of the 390 descriptions |
| `error_descriptions/preimage.py` | 47 | `rtao_catalog::CHAIN_ERRORS` (description field) | generated from catalog | `generated` | M2 | per-pallet slice of the 390 descriptions |
| `error_descriptions/proxy.py` | 48 | `rtao_catalog::CHAIN_ERRORS` (description field) | generated from catalog | `generated` | M2 | per-pallet slice of the 390 descriptions |
| `error_descriptions/safe_mode.py` | 42 | `rtao_catalog::CHAIN_ERRORS` (description field) | generated from catalog | `generated` | M2 | per-pallet slice of the 390 descriptions |
| `error_descriptions/scheduler.py` | 26 | `rtao_catalog::CHAIN_ERRORS` (description field) | generated from catalog | `generated` | M2 | per-pallet slice of the 390 descriptions |
| `error_descriptions/subtensor.py` | 940 | `rtao_catalog::CHAIN_ERRORS` (description field) | generated from catalog | `generated` | M2 | per-pallet slice of the 390 descriptions |
| `error_descriptions/sudo.py` | 10 | `rtao_catalog::CHAIN_ERRORS` (description field) | generated from catalog | `generated` | M2 | per-pallet slice of the 390 descriptions |
| `error_descriptions/swap.py` | 46 | `rtao_catalog::CHAIN_ERRORS` (description field) | generated from catalog | `generated` | M2 | per-pallet slice of the 390 descriptions |
| `error_descriptions/system.py` | 52 | `rtao_catalog::CHAIN_ERRORS` (description field) | generated from catalog | `generated` | M2 | per-pallet slice of the 390 descriptions |
| `error_descriptions/utility.py` | 16 | `rtao_catalog::CHAIN_ERRORS` (description field) | generated from catalog | `generated` | M2 | per-pallet slice of the 390 descriptions |

## `evm`

`PY/evm/*` parity, 1,243 lines (§6.6).

| Python file | Lines | rtao home | Kind | `map.toml` | M | Notes |
|---|---:|---|---|---|---|---|
| `evm/__init__.py` | 94 | `rtao_evm` crate root | not applicable | `n/a` | M7 | re-exports |
| `evm/addresses.py` | 91 | `rtao_primitives::evm` (`h160_to_ss58`, `ss58_to_h160_truncated`); `rtao_evm` | ported | `ported` | M5–M7 | in primitives so EVM intents (M5) need no `rtao-evm` (§6.6) |
| `evm/contracts.py` | 102 | `rtao_evm` contract artifacts | ported | `ported` | M7 | deploy and call encoding |
| `evm/keys.py` | 244 | `rtao_evm::EvmKeystore` | ported | `ported` | M7 | keystore-V3 next to the wallet (§6.6) |
| `evm/networks.py` | 76 | `rtao_evm::EvmNetwork` | extracted literal table | `extracted` | M7 | chain ids 964 and 945 (§6.6) |
| `evm/precompiles.py` | 389 | `rtao_evm::{Precompile,PRECOMPILES,STANDARD_PRECOMPILES}` and `precompiles` helpers; generated `precompile_catalog.rs` | ported | `ported` | M7 | Complete source and 21 SDK ABIs reviewed; five authenticated source tables generate production data. Raw ABI/catalog order, typed JSON/Alloy inputs, checked native u64 rao, full-width generic/output integers and bounded ABI decoding are explicit (§6.6). Canonical CLI selections retain source notices. The 352-function inventory establishes source coverage; functional/all-selector execution and live-runtime agreement remain unclaimed. |
| `evm/rpc.py` | 107 | `rtao_evm::EvmRpc` | ported | `ported` | M7 | minimal eth JSON-RPC over reqwest; wei/rao conversions |
| `evm/transactions.py` | 140 | `rtao_evm` transactions and `association_proof` | ported | `ported` | M7 | EIP-191 proof (§6.6) |

## `evm/abi`

Precompile ABIs, vendored verbatim.

| Python file | Lines | rtao home | Kind | `map.toml` | M | Notes |
|---|---:|---|---|---|---|---|
| `evm/abi/addressMapping.json` | 21 | `vendor/upstream/evm/abi/` → embedded by `rtao_evm::Precompiles` | vendored asset | `vendored` | M7 | copied verbatim at `parity_tag`, sha256 in `upstream.toml [artifacts]` (§1.2, §6.6) |
| `evm/abi/alpha.json` | 651 | `vendor/upstream/evm/abi/` → embedded by `rtao_evm::Precompiles` | vendored asset | `vendored` | M7 | copied verbatim at `parity_tag`, sha256 in `upstream.toml [artifacts]` (§1.2, §6.6) |
| `evm/abi/balance.json` | 47 | `vendor/upstream/evm/abi/` → embedded by `rtao_evm::Precompiles` | vendored asset | `vendored` | M7 | copied verbatim at `parity_tag`, sha256 in `upstream.toml [artifacts]` (§1.2, §6.6) |
| `evm/abi/balanceTransfer.json` | 50 | `vendor/upstream/evm/abi/` → embedded by `rtao_evm::Precompiles` | vendored asset | `vendored` | M7 | copied verbatim at `parity_tag`, sha256 in `upstream.toml [artifacts]` (§1.2, §6.6) |
| `evm/abi/crowdloan.json` | 281 | `vendor/upstream/evm/abi/` → embedded by `rtao_evm::Precompiles` | vendored asset | `vendored` | M7 | copied verbatim at `parity_tag`, sha256 in `upstream.toml [artifacts]` (§1.2, §6.6) |
| `evm/abi/drand.json` | 129 | `vendor/upstream/evm/abi/` → embedded by `rtao_evm::Precompiles` | vendored asset | `vendored` | M7 | copied verbatim at `parity_tag`, sha256 in `upstream.toml [artifacts]` (§1.2, §6.6) |
| `evm/abi/ed25519Verify.json` | 13 | `vendor/upstream/evm/abi/` → embedded by `rtao_evm::Precompiles` | vendored asset | `vendored` | M7 | copied verbatim at `parity_tag`, sha256 in `upstream.toml [artifacts]` (§1.2, §6.6) |
| `evm/abi/leasing.json` | 216 | `vendor/upstream/evm/abi/` → embedded by `rtao_evm::Precompiles` | vendored asset | `vendored` | M7 | copied verbatim at `parity_tag`, sha256 in `upstream.toml [artifacts]` (§1.2, §6.6) |
| `evm/abi/metagraph.json` | 388 | `vendor/upstream/evm/abi/` → embedded by `rtao_evm::Precompiles` | vendored asset | `vendored` | M7 | copied verbatim at `parity_tag`, sha256 in `upstream.toml [artifacts]` (§1.2, §6.6) |
| `evm/abi/neuron.json` | 1,526 | `vendor/upstream/evm/abi/` → embedded by `rtao_evm::Precompiles` | vendored asset | `vendored` | M7 | copied verbatim at `parity_tag`, sha256 in `upstream.toml [artifacts]` (§1.2, §6.6) |
| `evm/abi/proxy.json` | 371 | `vendor/upstream/evm/abi/` → embedded by `rtao_evm::Precompiles` | vendored asset | `vendored` | M7 | copied verbatim at `parity_tag`, sha256 in `upstream.toml [artifacts]` (§1.2, §6.6) |
| `evm/abi/registry.json` | 53 | `vendor/upstream/evm/abi/` → embedded by `rtao_evm::Precompiles` | vendored asset | `vendored` | M7 | copied verbatim at `parity_tag`, sha256 in `upstream.toml [artifacts]` (§1.2, §6.6) |
| `evm/abi/runtimeConfiguration.json` | 787 | `vendor/upstream/evm/abi/` → embedded by `rtao_evm::Precompiles` | vendored asset | `vendored` | M7 | copied verbatim at `parity_tag`, sha256 in `upstream.toml [artifacts]` (§1.2, §6.6) |
| `evm/abi/scheduler.json` | 183 | `vendor/upstream/evm/abi/` → embedded by `rtao_evm::Precompiles` | vendored asset | `vendored` | M7 | copied verbatim at `parity_tag`, sha256 in `upstream.toml [artifacts]` (§1.2, §6.6) |
| `evm/abi/sr25519Verify.json` | 13 | `vendor/upstream/evm/abi/` → embedded by `rtao_evm::Precompiles` | vendored asset | `vendored` | M7 | copied verbatim at `parity_tag`, sha256 in `upstream.toml [artifacts]` (§1.2, §6.6) |
| `evm/abi/staking.json` | 135 | `vendor/upstream/evm/abi/` → embedded by `rtao_evm::Precompiles` | vendored asset | `vendored` | M7 | copied verbatim at `parity_tag`, sha256 in `upstream.toml [artifacts]` (§1.2, §6.6) |
| `evm/abi/stakingV2.json` | 2,021 | `vendor/upstream/evm/abi/` → embedded by `rtao_evm::Precompiles` | vendored asset | `vendored` | M7 | copied verbatim at `parity_tag`, sha256 in `upstream.toml [artifacts]` (§1.2, §6.6) |
| `evm/abi/subnet.json` | 1,924 | `vendor/upstream/evm/abi/` → embedded by `rtao_evm::Precompiles` | vendored asset | `vendored` | M7 | copied verbatim at `parity_tag`, sha256 in `upstream.toml [artifacts]` (§1.2, §6.6) |
| `evm/abi/timestamp.json` | 28 | `vendor/upstream/evm/abi/` → embedded by `rtao_evm::Precompiles` | vendored asset | `vendored` | M7 | copied verbatim at `parity_tag`, sha256 in `upstream.toml [artifacts]` (§1.2, §6.6) |
| `evm/abi/uidLookup.json` | 76 | `vendor/upstream/evm/abi/` → embedded by `rtao_evm::Precompiles` | vendored asset | `vendored` | M7 | copied verbatim at `parity_tag`, sha256 in `upstream.toml [artifacts]` (§1.2, §6.6) |
| `evm/abi/votingPower.json` | 127 | `vendor/upstream/evm/abi/` → embedded by `rtao_evm::Precompiles` | vendored asset | `vendored` | M7 | copied verbatim at `parity_tag`, sha256 in `upstream.toml [artifacts]` (§1.2, §6.6) |

## `extension`

Browser-extension signing (§6.5).

| Python file | Lines | rtao home | Kind | `map.toml` | M | Notes |
|---|---:|---|---|---|---|---|
| `extension/__init__.py` | 41 | none needed | not applicable | `n/a` | — | re-exports |
| `extension/bridge.py` | 244 | `rtao_signer::extension::{BridgeServer, run_bridge}` (feature `extension`): axum page plus a WebSocket upgrade through hyper and tokio-tungstenite 0.30 | ported | `ported` | M7 | HTTP + WebSocket bridge on 127.0.0.1:39295 serving `bridge.html` (`:14, 17-40, 235`, §6.5); library API (`PY/__init__.py:198-206`); `rtao-cli` `extension bridge` runs it (§2.3 rule 2) |
| `extension/browser.py` | 65 | `rtao-cli` (`--extension-browser`) | ported | `ported` | M7 | opens the bridge page |
| `extension/client.py` | 137 | `rtao_signer::extension::{BridgeClient, connect_extension_signer}` (feature `extension`, tokio-tungstenite) | ported | `ported` | M7 | WebSocket client (`websockets.asyncio.client`, `:11`; §6.5) |
| `extension/daemon.py` | 50 | `rtao-cli` `extension bridge --daemon` | ported | `ported` | M7 | the background bridge process; `ensure_bridge` spawns `rtao extension bridge --daemon` instead of `python -m bittensor.extension.daemon` (§6.5) |
| `extension/picker.py` | 147 | `rtao-cli` account picker | ported | `ported` | M7 |  |
| `extension/session.py` | 287 | `rtao_signer::extension::{ensure_bridge, stop_bridge_daemon}` | ported | `ported` | M7 | pid file (`BITTENSOR_EXTENSION_BRIDGE_PID`, `:24`), readiness, restart; the spawn at `:121-140` becomes `rtao extension bridge --daemon` (§6.5) |
| `extension/signer.py` | 221 | `rtao_signer::ExtensionSigner` | ported | `ported` | M7 | `PAYLOAD_JSON` capability (§6.3) |
| `extension/tokens.py` | 34 | `rtao_signer::extension` (bridge token file) | ported | `ported` | M7 | `~/.bittensor/extension_bridge.token`, override `BITTENSOR_EXTENSION_BRIDGE_TOKEN` (`:14`; §6.5) |

## `extension/static`

| Python file | Lines | rtao home | Kind | `map.toml` | M | Notes |
|---|---:|---|---|---|---|---|
| `extension/static/bridge.html` | 628 | embedded with `include_bytes!` from `vendor/` | vendored asset | `vendored` | M7 | served verbatim, wire-compatible with the Python daemon (§6.5) |

## `intents`

Intent groups map file for file onto `crates/rtao-intents/src/<group>.rs` (§4.4); the shared machinery goes to `rtao-tx` and `rtao-catalog`.

| Python file | Lines | rtao home | Kind | `map.toml` | M | Notes |
|---|---:|---|---|---|---|---|
| `intents/__init__.py` | 191 | `rtao_intents` crate root | not applicable | `n/a` | — | imports register every intent; rtao's `AnyIntent` (generated) is the registry |
| `intents/_money.py` | 165 | `rtao_intents::money`; `rtao_primitives::{Money, Amount, Spend}` | ported + extracted literal table | `ported` | M2 | `"all"` sentinel `:34`, `Spend` `:37-48`, schema `:29-69`; `call_amount`/`tao_amount`/`alpha_amount` `:51-165` → the per-field resolution of §5.2 (unit from the call argument, netuid from a sibling field, netuid 0 tagged as TAO `:85-98`); `_FALLBACK_AMOUNT_UNITS` `:89-111` extracted |
| `intents/_root_claim_fee.py` | 771 | `rtao_intents::root_claim_fee` | ported | `ported` | M5 | 771-line fee model, preview only (§4.3); v466→v467 changed it with the catalog unchanged (§12.1) |
| `intents/association.py` | 95 | `rtao_intents::association` (feature `association`) | generated from catalog + ported | `ported` | M5 | 2 intents (2 generated `build()`, 0 ported); see [parity-inventory.md section 3](parity-inventory.md#3-intents-83); docstring `:44-95` fixes the EIP-191 proof format (§6.6); `evm/transactions.py` `association_proof` builds it |
| `intents/base.py` | 422 | `rtao_tx::{IntentInterface, Intent, DynIntent, IntentPreflight, Built, BuildCx, PreflightCx}` | ported | `ported` | M2 | hooks `:138-399`, `IntentPreflight` `:109-134`, `BuiltCall` `:414-422`; `to_dict` `:357-364` → `IntentInterface::to_json` (`"op"` first); `from_args` `:366-372` ignores `"op"` and rejects other unknown keys → `AnyIntent::from_tool` strips `"op"`, then `deny_unknown_fields` (§4.2, §4.5) |
| `intents/batch.py` | 167 | `rtao_intents::batch` (feature `batch`) | ported | `ported` | M5 | 1 intent (0 generated `build()`, 1 ported); see [parity-inventory.md section 3](parity-inventory.md#3-intents-83) |
| `intents/children.py` | 247 | `rtao_intents::children` (feature `children`) | generated from catalog + ported | `ported` | M5 | 5 intents (4 generated `build()`, 1 ported); see [parity-inventory.md section 3](parity-inventory.md#3-intents-83) |
| `intents/coldkey.py` | 185 | `rtao_intents::coldkey` (feature `coldkey`) | generated from catalog + ported | `ported` | M5 | 4 intents (3 generated `build()`, 1 ported); see [parity-inventory.md section 3](parity-inventory.md#3-intents-83) |
| `intents/collateral.py` | 176 | `rtao_intents::collateral` (feature `collateral`) | ported | `ported` | M5 | 2 intents (0 generated `build()`, 2 ported); see [parity-inventory.md section 3](parity-inventory.md#3-intents-83) |
| `intents/crowdloan.py` | 417 | `rtao_intents::crowdloan` (feature `crowdloan`) | generated from catalog + ported | `ported` | M5 | 10 intents (9 generated `build()`, 1 ported); see [parity-inventory.md section 3](parity-inventory.md#3-intents-83) |
| `intents/evm.py` | 119 | `rtao_intents::evm` (feature `evm`) | ported | `ported` | M5 | 2 intents (0 generated `build()`, 2 ported); see [parity-inventory.md section 3](parity-inventory.md#3-intents-83); mirror address `blake2_256("evm:" ++ h160)` documented at `:26-33`; `h160_to_ss58` lives in `rtao_primitives::evm` (§6.6) |
| `intents/governance.py` | 235 | `rtao_intents::governance` (feature `governance`) | generated from catalog + ported | `ported` | M4–M5 | 5 intents (4 generated `build()`, 1 ported); see [parity-inventory.md section 3](parity-inventory.md#3-intents-83) |
| `intents/hyperparameters.py` | 155 | `rtao_intents::hyperparameters` (feature `hyperparameters`) | ported | `ported` | M4 | 1 intent (0 generated `build()`, 1 ported); see [parity-inventory.md section 3](parity-inventory.md#3-intents-83); `OWNER_HYPERPARAMETERS` `:31-66` extracted (34 names, 33 setters) |
| `intents/identity.py` | 121 | `rtao_intents::identity` (feature `identity`) | generated from catalog + ported | `ported` | M4–M5 | 2 intents (2 generated `build()`, 0 ported); see [parity-inventory.md section 3](parity-inventory.md#3-intents-83) |
| `intents/leasing.py` | 114 | `rtao_intents::leasing` (feature `leasing`) | generated from catalog + ported | `ported` | M5 | 2 intents (2 generated `build()`, 0 ported); see [parity-inventory.md section 3](parity-inventory.md#3-intents-83) |
| `intents/lock.py` | 174 | `rtao_intents::lock` (feature `lock`) | generated from catalog + ported | `ported` | M5 | 4 intents (4 generated `build()`, 0 ported); see [parity-inventory.md section 3](parity-inventory.md#3-intents-83) |
| `intents/multisig.py` | 536 | `rtao_intents::multisig` (feature `multisig`) | ported | `ported` | M5 | 4 intents (0 generated `build()`, 4 ported); see [parity-inventory.md section 3](parity-inventory.md#3-intents-83); adapters `:276-398` → `MultisigAdapter`, `MultisigThreshold1Adapter` (§4.2) |
| `intents/plan.py` | 135 | `rtao_tx::{Policy, Plan}` | ported | `ported` | M2 | `Policy` `:21-85`, `Plan` `:89-135` (§4.6) |
| `intents/proxy.py` | 320 | `rtao_intents::proxy` (feature `proxy`) | generated from catalog + ported | `ported` | M5 | 6 intents (5 generated `build()`, 1 ported); see [parity-inventory.md section 3](parity-inventory.md#3-intents-83); `PROXY_TYPES` `:21-41` extracted; CI checks it against the metadata `ProxyType` enum (§3.12) |
| `intents/registration.py` | 595 | `rtao_intents::registration` (feature `registration`) | generated from catalog + ported | `ported` | M4–M5 | 8 intents (7 generated `build()`, 1 ported); see [parity-inventory.md section 3](parity-inventory.md#3-intents-83); `neuron_registration_split` `:24-44` (burn/lock preview, §9.6) |
| `intents/registry.py` | 66 | `rtao_catalog::INTENTS`; `rtao_intents::AnyIntent`; `rtao_tx::Executor::tools` | generated from catalog | `generated` | M2 | `list_tools` `:43-66` |
| `intents/root.py` | 135 | `rtao_intents::root` (feature `root`) | ported | `ported` | M5 | 2 intents (0 generated `build()`, 2 ported); see [parity-inventory.md section 3](parity-inventory.md#3-intents-83) |
| `intents/serving.py` | 190 | `rtao_intents::serving` (feature `serving`) | ported | `ported` | M2 | 4 intents (0 generated `build()`, 4 ported); see [parity-inventory.md section 3](parity-inventory.md#3-intents-83) |
| `intents/staking.py` | 1,925 | `rtao_intents::staking` (feature `staking`) | generated from catalog + ported | `ported` | M5 | 14 intents (3 generated `build()`, 11 ported); see [parity-inventory.md section 3](parity-inventory.md#3-intents-83) |
| `intents/transfer.py` | 116 | `rtao_intents::transfer` (feature `transfer`) | generated from catalog + ported | `ported` | M2 | 2 intents (1 generated `build()`, 1 ported); see [parity-inventory.md section 3](parity-inventory.md#3-intents-83) |
| `intents/weights.py` | 563 | `rtao_intents::weights` (feature `weights`) | ported | `ported` | M2 | 3 intents (0 generated `build()`, 3 ported); see [parity-inventory.md section 3](parity-inventory.md#3-intents-83); `normalize`/`clip_to_max_weight` `:54-114`, `_conform` `:218-258` → `rtao_intents::weights::conform` (§9.3); `DEFAULT_COMMIT_REVEAL_VERSION` `:33` |

## `reads`

Read modules map file for file onto `crates/rtao-reads/src/reads/<module>.rs` (§4.8).

| Python file | Lines | rtao home | Kind | `map.toml` | M | Notes |
|---|---:|---|---|---|---|---|
| `reads/__init__.py` | 64 | `rtao_reads` crate root | not applicable | `n/a` | — | imports register every read |
| `reads/accounts.py` | 119 | `rtao_reads::reads::accounts` | ported | `ported` | M4–M6 | 6 reads; see [parity-inventory.md section 5](parity-inventory.md#5-reads-102) |
| `reads/base.py` | 185 | `rtao_reads` generated `ReadSpec`, `ReadName`, `dispatch`; `utf8_text` | generated from catalog + ported | `ported` | M1 | render hints `:33-54`; `scalar_read` `:156-166` generated whole (§4.8); `_coercing` address rule |
| `reads/chain.py` | 77 | `rtao_reads::reads::chain` | ported + generated | `ported` | M2–M6 | 6 reads (1 scalar, generated whole); see [parity-inventory.md section 5](parity-inventory.md#5-reads-102) |
| `reads/collateral.py` | 156 | `rtao_reads::reads::collateral` | ported | `ported` | M6 | 3 reads; see [parity-inventory.md section 5](parity-inventory.md#5-reads-102) |
| `reads/delegation.py` | 235 | `rtao_reads::reads::delegation` | ported | `ported` | M4–M6 | 9 reads; see [parity-inventory.md section 5](parity-inventory.md#5-reads-102) |
| `reads/epochs.py` | 128 | `rtao_reads::reads::epochs` | ported + generated | `ported` | M2–M4 | 5 reads (1 scalar, generated whole); see [parity-inventory.md section 5](parity-inventory.md#5-reads-102) |
| `reads/hyperparameters.py` | 54 | `rtao_reads::reads::hyperparameters` | generated from catalog | `generated` | M2–M4 | 7 reads (7 scalar, generated whole); see [parity-inventory.md section 5](parity-inventory.md#5-reads-102) |
| `reads/identity.py` | 162 | `rtao_reads::reads::identity` | ported | `ported` | M4–M6 | 5 reads; see [parity-inventory.md section 5](parity-inventory.md#5-reads-102); `commitments` calls `metagraph.fetch_commitments`, which lives in `rtao_reads::commitments` (§4.8) |
| `reads/leasing.py` | 112 | `rtao_reads::reads::leasing` | ported | `ported` | M6 | 5 reads; see [parity-inventory.md section 5](parity-inventory.md#5-reads-102) |
| `reads/locks.py` | 419 | `rtao_reads::reads::locks` | ported | `ported` | M4–M6 | 7 reads; see [parity-inventory.md section 5](parity-inventory.md#5-reads-102); `subnet_convictions` float math ported exactly (`:188-286`, §4.8) |
| `reads/neurons.py` | 135 | `rtao_reads::reads::neurons` | ported | `ported` | M2–M6 | 6 reads; see [parity-inventory.md section 5](parity-inventory.md#5-reads-102) |
| `reads/prices.py` | 95 | `rtao_reads::reads::prices` | ported | `ported` | M6 | 4 reads; see [parity-inventory.md section 5](parity-inventory.md#5-reads-102) |
| `reads/root.py` | 451 | `rtao_reads::reads::root` | ported | `ported` | M4–M6 | 11 reads; see [parity-inventory.md section 5](parity-inventory.md#5-reads-102); its reads belong to the `Staking` namespace |
| `reads/staking.py` | 323 | `rtao_reads::reads::staking` | ported | `ported` | M4–M6 | 11 reads; see [parity-inventory.md section 5](parity-inventory.md#5-reads-102) |
| `reads/subnets.py` | 290 | `rtao_reads::reads::subnets` | ported | `ported` | M1–M6 | 14 reads; see [parity-inventory.md section 5](parity-inventory.md#5-reads-102) |
| `reads/weights.py` | 113 | `rtao_reads::reads::weights` | ported | `ported` | M2–M6 | 3 reads; see [parity-inventory.md section 5](parity-inventory.md#5-reads-102) |

## `vault`

Polkadot Vault (QR, air-gapped) signing (§6.5).

| Python file | Lines | rtao home | Kind | `map.toml` | M | Notes |
|---|---:|---|---|---|---|---|
| `vault/__init__.py` | 22 | none needed | not applicable | `n/a` | — | re-exports |
| `vault/qr.py` | 28 | `rtao_signer::VaultSigner` (feature `vault`) | ported | `ported` | M7 | UOS frames as SVG QR data URIs: byte mode without mode optimization, error correction M, 4-module quiet zone (`:15-28`); a QR crate chosen at M7 [I], tested by decoding the symbol back to the frame (§2.7) |
| `vault/scan.py` | 49 | `rtao_signer::vault::scan_vault_address` | ported | `ported` | M7 | imports an address by scanning the phone's QR; parses `substrate:<ss58>:<genesis>` or a bare ss58 (`:21-49`) |
| `vault/server.py` | 242 | `rtao_signer::vault::VaultSessionServer` (feature `vault`) | ported | `ported` | M7 | one-shot loopback HTTP and WebSocket server on an ephemeral port (`websockets` server, `:26, 69-80`): serves `vault.html` and the chain-specs PNG, and speaks the page's JSON protocol over `/ws` (`:181-242`); in rtao on axum with a hyper WebSocket upgrade, which dependency rule 2 allows under feature `vault` (§2.3, §6.5) |
| `vault/signer.py` | 286 | `rtao_signer::VaultSigner` | ported | `ported` | M7 | `UNSIGNED_TX \| OUT_OF_BAND`; shield needs two scans inside the 8-block era (§6.5) |
| `vault/uos.py` | 148 | `rtao_signer::VaultSigner` (UOS framing) | ported | `ported` | M7 | "port of `PY/vault/uos.py`" (§6.5) |

## `vault/static`

| Python file | Lines | rtao home | Kind | `map.toml` | M | Notes |
|---|---:|---|---|---|---|---|
| `vault/static/bittensor-chain-specs.png` | binary | embedded with `include_bytes!` from `vendor/` | vendored asset | `vendored` | M7 | binary, 5,631 bytes |
| `vault/static/vault.html` | 642 | embedded with `include_bytes!` from `vendor/` | vendored asset | `vendored` | M7 | served verbatim (§6.5) |

## Other watched roots

Files outside the package that the file layer of [§4.9](architecture.md#49-parity-enforcement-the-machinery) watches.
Each is one `map.toml` row (137 at v477), seeded `pending` in M0 like the package rows.

| Files | Count | `map.toml` | rtao home / note | M |
|---|---:|---|---|---|
| `sdk/python/codegen/check.py` | 1 | `extracted` | `rtao_catalog::py::{COVERED_PALLETS, RAW_ONLY}` (the coverage gate, §4.9 layer 6) and `tests/hyperparam_units.rs` (§5.5) | M2 |
| `sdk/python/codegen/{__init__,__main__,emit_python,emit_namespaces,metadata}.py` | 5 | `n/a` | replaced by `xtask regen` (§3.7) | M2 |
| `sdk/python/tests/unit/*.py` | 34 | `oracle-only`, except 21 `ported` | ported with `// pyt:` provenance (§11.2): the table-shaped `test_rpc` (M1), `test_http_auth` (M0), `test_balance_properties` (M0), `test_null_weights`, `test_slippage_protection`, `test_root_claim_fee`, `test_lock_ownership_gate`, `test_hyperparams_units`, `test_fee_filters`, `test_multisig_safety`, `test_prepare_attach` and `test_wallet_compat`; and the behavior tests the oracle does not reach: `test_v446_accounting_and_commitments` (M1 commitments, M6 locks), `test_pow_registration` (M4: the solver and intent keep the runtime challenge), `test_cli_null_pruning` (M4: owner pruning must advance on chain, not only return successful receipts), `test_natural_name_key` (M4: `coldkey2` before `coldkey10` in CLI listings), `test_facades` (M6: `SyncClient` namespace parity), `test_ledger_signer`, `test_extension_bridge`, `test_extension_signer` and `test_evm` (M7, the M7 exit) | the milestone of the code each tests |
| `sdk/python/tests/harness/{samples,fake_substrate}.py` | 2 | `extracted` | oracle inputs and `FakeChain` defaults (§4.1, §11.3) | M2 |
| `sdk/python/tests/harness/fake_node.py` | 1 | `ported` | `rtao-testkit::FakeNode` (§11.3) | M1 |
| `sdk/python/tests/harness/__init__.py` | 1 | `n/a` | | — |
| `sdk/python/tests/fixtures/golden.json`, `shape_corpus/corpus.json` | 2 | `vendored` | `vendor/upstream/` (§11.2) | M0 |
| `sdk/python/tests/e2e/*.py` | 3 | `oracle-only` | mapped in `e2e_map.toml` as `pyt:e2e/<file>::<function>` → §11.4 scenarios 4, 14 and 19 | M2-M7 |
| `sdk/python/pyproject.toml` | 1 | `extracted` | the package version (`python_package_version`) and `[project.scripts]` (the `btcli` entry point) | M0 |
| `sdk/python/completions/**` | 3 | `n/a` | `rtao-cli` generates its own with `clap_complete` | M8 |
| `sdk/python/scripts/{record_golden,record_shape_corpus,build_basket_index}.py` | 3 | `oracle-only` | run by `xtask record` through `ci/oracle/record.py` (§11.2); `build_basket_index.py` regenerates `basket_index_data.py` | M2, M6 |
| `sdk/bittensor-core/**` | 35 | `n/a` | guarded by `core-surface` and `staged --check` (§1.6, §1.9) | M0 |
| `sdk/bittensor-core-py/**` | 12 | `oracle-only` | the 1,865-line PyO3 binding through which Python reaches the core; the rows for `values.rs`, `runtime.rs`, `timelock.rs` and `keys.rs` name the rtao code that mirrors their shapes (§4.9) | M0 |
| `pallets/subtensor/src/rpc_info/**` | 9 | `extracted` | `freeze_struct` hashes, `SelectiveMetagraphIndex` (§4.9 layer 5) | M1 |
| `common/src/{transaction_error,proxy}.rs`, `runtime/src/{fee_filters,check_mortality,transaction_payment_wrapper}.rs`, `pallets/subtensor/src/coinbase/{run_coinbase,reveal_commits}.rs` | 7 | `extracted` (with `syn`) | custom codes, coldkey-pays-fee calls, `MAX_SHIELD_ERA_PERIOD`, epoch and reveal rules (§4.9 layer 5) | M1-M2 |
| `pallets/subtensor/src/subnets/{serving,weights,subnet}.rs`, `pallets/subtensor/src/utils/{misc,rate_limiting}.rs`, `pallets/subtensor/src/coinbase/tempo_control.rs`, `pallets/admin-utils/src/lib.rs` | 7 | `ported` | the chain rules the framework mirrors: `AxonPublisher`, `WeightScheduler`, `OwnerOps::create_subnet`, `HyperparamPlanner`, staged `epoch_schedule` (§4.9, §9.5, §9.7) | M2-M4 |
| `docs/guides/signed-requests.mdx` | 1 | `vendored` | the btauth/1 specification and vectors (§7) | M0 |
| `website/apps/bittensor-website/public/catalog/{intents,reads,errors}.json` | 3 | `vendored` (compared normalized, §4.9) | `rtao-catalog` (§4.1) | M0 |
| `eco-tests/**` | 7 | `n/a` | | — |

## Top-level exports (`PY/__init__.py:122-219` `__all__`)

Every exported name has a home here, and `xtask parity check` asserts that each `__all__` name has a row in this table
or a `#[doc(alias)]` hit (§4.9 layer 8).

| `__all__` names | rtao home | M |
|---|---|---|
| `Subtensor`, `subtensor`, `SyncClient`, `set_weights`, `close_shared_clients` | `rtao::blocking` | M6 |
| `Client`, `Snapshot` | `rtao_client` | M1 |
| `Substrate` | `rtao_client::View`, and `Client::from_rpc` over a custom `RpcClientT` (§3.3) | M1 |
| `RpcSubstrate` | `rtao_rpc::Session` | M1 |
| `BlockHeader` | `rtao_client::BlockHeader { number, parent_hash, raw }` (§3.13) | M1 |
| `BlockInfo` | `rtao_client::BlockInfo`; multisig opening recovery separately retains exact call bytes from its pinned raw block | M5 |
| `EpochEvent` | `rtao_client::EpochEvent { netuid, block, previous }`, re-exported by `rtao::blocking` | M2 |
| `Metagraph`, `MetagraphNeuron` | `rtao_metagraph` | M1 |
| `NeuronCommitment` | `rtao_reads::commitments` | M1 |
| `Neuron`, `SubnetInfo`, `StakePosition`, `StakeValuation`, `DelegateInfo`, `DelegatedStake`, `Commitment`, `SwapQuote` | `rtao_reads` models | the milestone of their read ([parity-inventory §5](parity-inventory.md#5-reads-102)) |
| `Balance`, `tao`, `alpha`, `rao` | `rtao_primitives::DynBalance` and its constructors | M0 |
| `UnitMismatchError` | `rtao_primitives::UnitError::UnitMismatch` | M0 |
| `ExtrinsicResult`, `BittensorError`, `ChainError`, `ConnectionNotReady`, `ErrorCode`, `PolicyError`, `RpcConnectionError`, `RpcPolicyError` | `rtao_tx` (`ExtrinsicResult`, `TxError`, `ChainError`, `PolicyError`) and `rtao_catalog::ErrorCode` | M2 |
| `Intent`, `Plan`, `Policy` | `rtao_tx` | M2 |
| `Multisig` | `rtao_tx::multisig::Multisig` | M4 |
| `Signer`, `KeyedWallet`, `WalletLike`, `WalletSigner`, `public_view`, `resolve_signer`, `SigningContext` | `rtao_signer::{Signer, SigningContext, KeyView, KeySource<R>, DynKeySource, IntentKeyView}`; typed public identity/role/lazy acquisition replaces the Python structural wallet helpers, with `rtao_wallet::{Wallet, WalletRef}` consumers | M2 |
| `UnsignedExtrinsic` | `rtao_client::UnsignedExtrinsic` | M5 |
| `Wallet`, `wallet`, `wallets` | `rtao_wallet` | M2 |
| `Timelocked`, `TimelockError`, `TimelockNotReady`, `timelock` | `rtao_tx::timelock` (`TimelockError::NotReady`); the pure helpers in `rtao_primitives::drand` | M7 (helpers M1) |
| `BridgeClient`, `BridgeError`, `BridgeServer`, `ExtensionAccount`, `ExtensionSigner`, `connect_extension_signer`, `ensure_bridge`, `run_bridge`, `stop_bridge_daemon` | `rtao_signer::extension` | M7 |
| `LedgerSigner`, `LedgerError`, `VaultSigner`, `VaultError` | `rtao_signer` | M7 |
| `CRYPTO_ED25519`, `CRYPTO_SR25519`, `DEFAULT_CRYPTO_TYPE`, `parse_crypto_type`, `format_crypto_type` | `rtao_primitives::CryptoType` variants and checked numeric conversion; callers select the typed enum and wallet/CLI defaults retain sr25519. Python's dynamic string helpers are represented by typed selection, without same-named public Rust helpers | M0 |
| `reads`, `intents` | the `rtao_reads` and `rtao_intents` crate roots | M1-M6, M2-M5 |
| `evm` | `rtao_evm`; the h160↔ss58 math in `rtao_primitives::evm` | M7 (M5 for the math) |
| `http_auth` | `rtao_btauth` | M0 |
| `storage`, `runtime_api`, `constants`, `calls` | `rtao_runtime` (generated descriptors and call builders) | M1 |
| the 83 intent classes (`_INTENT_EXPORTS`) | `rtao_intents::<Class>` | per [parity-inventory §3](parity-inventory.md#3-intents-83) |
| `_REMOVED_V10_HINTS` (`:238-276`, not in `__all__`) | n/a (the migration guide, §13) | — |

## Gaps found

Python modules that [architecture.md](architecture.md) gave no home, or a home that conflicted with
[§2.2](architecture.md#22-crates) or the layering rules of [§2.3](architecture.md#23-dependency-layers-and-enforced-rules).
All are now decided in architecture.md or PLAN.md; the rows above follow the decisions.

| Python module | Gap | Evidence | Resolution |
|---|---|---|---|
| `basket_index.py` (361), `basket_index_data.py` (975) | no home: not mentioned anywhere in the architecture | CLI-only display pricing for basket funds; imported by `cli/root_helpers.py:18`, `cli/commands/root.py:23`, `cli/commands/root_move.py:11`. `root list --json` emits `basket_index` through `index_level()` (`cli/commands/root.py:156, 240`), so dropping it would break that output. | **Decided**: `rtao-cli` `root_helpers::basket_index` (M6); the data generated into `rtao-cli/src/generated/basket_index_data.rs` from extracted literal tuples, not `rtao-catalog`, which every neuron build compiles (PLAN §14.3 #7). |
| `config.py` (480) | no single home; three crates need it | `Client::connect` loads token symbols through its 7-day cache (§3.3 step 6, `:22-27`); `NeuronConfig` reads `btcli.json` (§9.9); the CLI owns the settable keys, the books and the multisig call cache (§10.2). | **Decided**: `rtao_client::config` (M1) for the seven files, their overrides and the caches; `rtao-cli` `books` for readers (M4) and writers (M5, M8) (§10.2; PLAN §14.3 #8). |
| `metagraph.py` `fetch_commitments` (`:258`) and `namespaces.py` curated `Subnets::metagraph`/`commitments` (`:161-188`) | home inconsistent with the layering of §2.3 | The curated methods and the `commitments` read live in `rtao-reads` and call the commitment machinery and `Metagraph`, which §8.2 placed in `rtao-metagraph`, an `rtao-reads` dependent. | **Decided**: the commitment machinery in `rtao_reads::commitments`; the curated `metagraph()` as `rtao_metagraph::SubnetsMetagraphExt` in the prelude, with no inherent `Subnets::metagraph`; drand round math in `rtao_primitives::drand` (§4.8, §8.2; PLAN §14.3 #5). |
| `hyperparams.py` (769) | two homes, one against the layering | Functions in `rtao-primitives` (L2) over a table in `rtao-catalog` (L3); `to_raw` also listed in `rtao-intents`. | **Decided**: table-free helpers in `rtao_primitives::ratio` and `::hyperparams` (M1); the ported functions as methods on `HyperparamTable`; the data in `rtao_catalog::hyperparams::TABLE`; `rtao_intents::hyperparameters::to_raw` delegates (§5.5; PLAN §14.3 #6). |
| `extension/bridge.py` (244), `extension/client.py` (137) | home conflicts with dependency rule 2 of §2.3 | The bridge is a WebSocket server and client (`websockets.asyncio.server` `:14`, `websockets.asyncio.client` `client.py:11`) and is library API (`PY/__init__.py:198-206`). | **Decided**: server, client and `ensure_bridge` in `rtao-signer` under feature `extension`; rule 2 allows `tokio-tungstenite` there; the server's WebSocket upgrade goes through hyper, because axum 0.8.9's `ws` requires tokio-tungstenite 0.29 (§2.3, §6.5; PLAN §14.3 #9). |
| `_live.py` (81), `masked_input.py` (147) | no explicit home | Terminal plumbing for password prompts, imported by the library module `keyfiles.py:9` and by the CLI. The library never prompts in rtao (D-003) and the CLI uses `rpassword` (§2.7). | **Decided**: the `rtao-cli` prompt module in M4 (`rpassword` hidden entry under `PromptPolicy::IfTty`, re-asking as `keyfiles.py:42-62`; PLAN §12.6), map kind `n/a` for both. |
| `vault/server.py` (242), `vault/qr.py` (28) | home conflicted with dependency rule 2 of §2.3, and no QR encoder was in §2.7 | The Vault page server is a `websockets` server (`server.py:26, 69-80`), like the extension bridge; `qr.py` renders frames with the `qrcode` package (`:15-28`). | **Decided**: `rtao_signer::vault` under feature `vault`; rule 2 allows tokio-tungstenite there; the upgrade goes through hyper as for the bridge; a QR crate chosen at M7 [I] (§2.3, §2.7, §6.5; PLAN §14.4 #19). |
| `namespaces.pyi` (738) | no home (none needed) | Generated type stub for the dynamic namespaces. | map kind `n/a`; listed so the file layer of §4.9 has a row for it. |

The `map.toml` kind for vendored static assets (`evm/abi/*.json`, `extension/static/`, `vault/static/`) is now
`vendored` (§4.9; PLAN §14.3 #13). The internal module layout of `rtao-cli` is not specified; this map assumes one
module per Python command file.
