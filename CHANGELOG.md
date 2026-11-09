# Changelog

## Unreleased

### Core bumped from v478 to v488 (D-127, D-136)

- `bittensor-core` and its companions move from v478 to v488, the parity pin's core (`b3:3936452e140b597df13a0ab4cc6a7d08b5ac605fe8ccfaf858ee017db97dd730`); `[parity] core_fingerprint` is gone. The reviewed core patch is carried onto v488 (fork revision `cd1a2e6ae41bf558eef8e21d00348c5a65e77d33`), and every downstream `[patch]` line names v488: applications that copy rtao's patch set must update theirs.
- The parity pin (v488) and the runtime are unchanged. 81 hand port(s) remain listed in `parity/port_due.toml`.
- Checked by the hosted runtime-adopt workflow ([run](https://github.com/sam0x17/rtao/actions/runs/42502998044)): pin provenance and live evidence for the pinned runtime, the core patch's review identity, Ed25519 erasure check and patched/unpatched API probes, the lock, imports, the core surface, extraction, regeneration, compatibility, parity, formatting, a workspace build, the core, runtime, reads, intents, client, signer and wallet tests, and live reads with the built CLI. No oracle comparison, localnet e2e or image execution.

### Since `snapshot-2026-11-08.5`

Not a versioned release. Each item links to its section below.

- Payments and ownership changes to hashed and post-quantum receiving addresses register or check the recipient in the same atomic batch, and the executor refuses to sign a first registration without explicit sponsorship consent (`max_registration_deposit`, `on_registration_sponsorship`, `rtao --max-registration-deposit`) (D-148).

### Registration guards and sponsorship consent (D-148)

#### rtao-primitives

- `Ss58Address` accepts a `tao_`/`bth` receiving address, keeping its spelling and the descriptor it carries (`descriptor`, `is_receiving`); its `account` is the hashed or post-quantum account. New: `receiving::descriptor_account`.

#### rtao-signer

- `KeyView` gains `hotkey_descriptor` and `local_descriptor` (default `None`): public descriptors a wallet can supply so that an operation defaulting to its hotkey, or a payment to one of its own hashed keys, registers it. No wallet implements them yet.

#### rtao-tx

- New `hashed` module (Python `hashed.py`, write side): `with_recipient_registration` and its batch form `recipient_registration_calls`, `receiving_recipients`, `has_receiving_setup_inputs`, `atomic_calls`, `hashed_accounts_enabled`, `descriptor_registration_enabled`, `require_receiving_support`, the protected-account reads (`protected_account_kind`, `is_pq_required`, `quote_registration_deposit`, moved from rtao-intents) and `guarded_build`/`check_build_recipients`.
- The executor resolves receiving addresses at the call boundary under that guard. A first payment to a registered target (`transfer`, `transfer_all`, coldkey announcement and swap, `swap_hotkey`, the registrations) is `batch_all[check_registration_deposit, register, call]`; a registered recipient adds `check_registered`; an announced coldkey swap, PoW registration or announced proxy execution requires the finalized registration and stays direct; a stateless post-quantum recipient needs only the runtime's support. Wrapping a first registration in proxy, multisig or Sudo is refused, as is a first registration in a batch (batches carry check-only guards). An intent build that names a receiving address or a `hashed_descriptor` outside such a guard refuses. A later multisig round with recipient inputs rebuilds and must reproduce the approved bytes.
- New `sponsorship` module: `RegistrationSponsorship`, `SponsorshipConsent`, `SponsorshipRequired`, `registration_sponsorship_quote`, `authorize_registration_sponsorship`; `ExecuteOptions::{max_registration_deposit, on_registration_sponsorship}` and `TxError::RegistrationSponsorship`. Execution authorizes the composed call's reserve before any signer is resolved, on the plain and Shield routes.
- `IntentPreflight` gains `registration_sponsorship` and `account_registration`, and previews the setup reserve (the maximum, a first payment's deposit, the repriced batch and, for a transfer, the sender's remaining balance). Source-breaking for `IntentPreflight` and `ExecuteOptions` struct literals without `..Default::default()`.

#### rtao-intents

- `Transfer`, `TransferAll` and `AnnounceColdkeySwap` accept `hashed_descriptor` (decoded strictly at validation) instead of refusing it. `Batch` guards each child in place.

#### rtao-cli

- `--max-registration-deposit` authorizes up to that much TAO for automatic recipient registration; `--yes` never does. Without it an interactive session asks, and JSON or piped sessions refuse with the flag to pass.

### Since `snapshot-2026-11-08.4`

Not a versioned release. Each item links to its section below.

- `rtao multisig add` saves new groups as safe by default (`--account-mode`), and `--multisig-account-mode` selects or checks a group's derivation (D-145).
- The subnet template, `fuzz/` and the CI probe workspaces build the reviewed core patch, kept equal to the root's by the pin and template checks (D-146).
- Hashed and post-quantum accounts can sign and submit through the new `AuthoritySigner` and `Client::submit_authorized`, from authority read on chain and verified before sending (D-147); the existing 64-byte signing paths are unchanged.

### Hashed and post-quantum signing (D-147)

#### rtao-primitives

- New: `AuthorityType` (non-exhaustive): the core's hashed and post-quantum crypto codes (hashed Sr25519/Ed25519, rotating and standard ML-DSA-65, Falcon-512 and Falcon-1024), with Python's `pq_lengths`, `hashed_proof_length` (`proof_length`), `is_stateless_pq` (`is_stateless`), `requires_registration`, the descriptor profile and a fee-quote `placeholder_proof`; `HASHED_PROOF_LENGTH` (136) and `MLDSA_PROOF_LENGTH` (5367). `CryptoType` stays Ed25519/Sr25519.
- New: `receiving::raw_bytes` (Python `hashed.raw_bytes`).

#### rtao-signer

- New: `AuthoritySigner`, an object-safe signer for hashed and post-quantum accounts that returns a variable-length `AuthorizationProof` over the complete unshortened payload (`AuthorizationRequest`) at a generation read from chain state; `KeypairAuthoritySigner` wraps an unlocked core keypair, refuses a classical key and checks that its account derives from its public descriptor (`expect_identity` checks it against a public wallet). `Signer` and `MultiSignature` are unchanged.

#### rtao-core

- New re-exports in `keys`: `verify_hashed_authorization`, `verify_mldsa_authorization`, `account_crypto_type` and the Falcon crypto codes.

#### rtao-client

- New: `HashedSigningSnapshot`, `AuthorizedExtrinsic` and `Client::{hashed_snapshot, prepare_hashed_snapshot, sign_authorized, submit_authorized}`. A rotating account signs at the generation and confirmed nonce of its `HashedAccounts` record (the best block for classical hashed accounts, the finalized block for post-quantum ones); the runtime must support the scheme, `AdminUtils.HashedAccountsEnabled` must be true, the record must name the signer's descriptor and the signer's key must match its commitment; a pending transaction, another nonce and a third Shield offset are refused. A standard post-quantum account needs no registration and takes ordinary nonce leases. Every proof is verified against the chain-read authority before the core encodes the extrinsic. The journal is refused for these submissions for now.

### CLI account modes; the template's core patch (D-145, D-146)

#### rtao-cli

- Changed: `rtao multisig add` saves `account_mode`. **A new group without `--account-mode` is now saved as `safe`**, as in Python v488: its account differs from the legacy account of the same members, and it needs at least two distinct members. Pass `--account-mode legacy` to save a group for an existing standard multisig account. Replacing a saved group with `--overwrite` keeps its saved mode; existing saved groups are unchanged.
- New: `--multisig-account-mode legacy|safe` on `multisig pending`, `call` and `upgrade sign`: the derivation of an inline signer set (legacy when omitted); with a saved group it must match the saved mode, and with a release manifest's sudo group it overrides the manifest's `account_mode`. Co-signer replay commands for a safe group include `--multisig-account-mode safe`.
- Changed: `rtao doctor` accepts a workspace that carries rtao's core patch without an `upstream.toml` of its own, checking the replacement against the record beside the `rtao-core` it resolves.

#### Template and workspaces

- Changed: `templates/subnet/Cargo.toml` carries the root's core patch tables (`bittensor-core` at the reviewed fork revision, the fork's path packages at the core tag), so a generated project builds the core that wipes Ed25519 and hashed-account signing keys. Projects generated earlier can copy the two tables from the same rtao revision's `Cargo.toml`.
- `fuzz/` and `ci/validator/probe/` carry the same tables and lock the core from the fork. `cargo xtask pin --check` and the template source check refuse a secondary manifest whose tables differ from the root's; a core-patch adoption rewrites them together.

### Since `snapshot-2026-11-08.3`

Not a versioned release. Each item links to its section below.

- Safe multisig derivation: the multisig intents, `Executor::multisig_with_mode` and the blocking `SyncClient::multisig` take `account_mode` and compose the `*_safe` calls (D-143); saved safe groups resolve and approve as their safe account.
- Limit orders accept receiving addresses; a receiving fee recipient is resolved only after its finalized registration, through the new `PreparedOrder::prepare_checked` (D-144).
- The secondary lockfiles follow the v488 core, and `rtao-intents` builds on Rust 1.89 again.

### Safe multisig derivation; receiving identities in limit orders (D-143, D-144)

#### rtao-primitives

- New: `AccountMode` (`Legacy`, `Safe`) and `multisig_account_id(signatories, threshold, mode)`, Python's `account_mode` and `multisig_account`; safe derivation is the core's `safe_multisig_account_id`.
- New: `receiving::{descriptor_value, descriptor_bytes, descriptor_storage, registration_descriptor_value, is_stateless_pq_descriptor}` (Python `hashed`).

#### rtao-core

- New: `extrinsic::safe_multisig_account_id`.

#### rtao-client

- New: `compose::{as_multi_threshold_1_with_mode, as_multi_with_mode, as_multi_existing_with_mode, approve_as_multi_with_mode}`, which compose the `*_safe` calls for a safe group; the existing functions are unchanged (legacy).

#### rtao-tx

- New: `Multisig::with_mode`, `Executor::multisig_with_mode` (Python's `Client.multisig(..., account_mode=)`).
- Changed (source-breaking): `Multisig` has a public `account_mode` field; struct literals must set it.
- New: `receiving::{finalized_descriptor, account_for_registered_write}`: a receiving address on a write that cannot carry a registration guard resolves only to a finalized, matching registration.

#### rtao-intents

- Changed: `multisig_execute`, `multisig_approve`, `multisig_threshold_1` and `multisig_cancel` accept `account_mode = "safe"` (they refused it) and compose the safe calls from the safe account; other signatories may be receiving addresses and are deduplicated by account.
- Changed: limit orders accept receiving addresses (they refused them). `signer`, `hotkey` and `relayer` resolve to their accounts; a receiving `fee_recipient` is resolved only after its registration is finalized. `PreparedOrder::prepare` and `encoded_order` stay synchronous and refuse a receiving fee recipient, naming the new `PreparedOrder::prepare_checked(client, nonce, order)`; new `limit_orders::{order_for_chain, registered_fee_recipient}`.

#### rtao (blocking)

- Changed (source-breaking): `SyncClient::multisig(signatories, threshold, account_mode)`.

#### rtao-cli

- Fixed: a saved multisig with `account_mode: "safe"` resolves to its safe account, and a saved or manifest sudo group used with `--multisig` composes its own mode's calls; before, a saved safe group used as a preset would have been approved as the legacy account.

### Secondary lockfiles at the v488 core; Rust 1.89 build restored

#### Repository maintenance

- Changed: `fuzz/`, `ci/tls-two-providers/` and `ci/validator/probe/` lock the v488 core (`cargo update -p bittensor-core -p subtensor-macros -p w3f-bls`, which adds `fips204`, `subtensor-hashed`, the `fn-dsa` crates and `hkdf` at the root lock's versions); the two-provider fixture's `[patch]` tables equal the root's again (the core patch at `cd1a2e6a` with `fips204` and `subtensor-hashed`), as `verify-graph.mjs` requires. Each builds with `--locked`; a generated `templates/subnet` project builds its validator, miner, owner tool and e2e harness against this checkout.
- Fixed: `rtao-intents` compiles on the minimum supported Rust (1.89) again: `add_collateral` used an `if let` match guard, which 1.89 does not accept.
- Docs: the README and getting-started name the v488 core pin, and the downstream patch table lists the fork's `fips204` and `subtensor-hashed` lines and the hashed-account Ed25519 wipe (D-139).

### Since `snapshot-2026-11-07.3`

Not a versioned release. Each item links to its section below.

- The core is bumped from v478 to v488 (D-127, D-136, D-142); reads accept receiving addresses for their address parameters (D-142).
- Account registration can take MEV Shield automatically when enabled with `Executor::with_auto_shield` (D-140); HTTP authentication keeps its verifier until hashed identities verify (D-141).
- The core patch extends its Ed25519 wipe fix to v488's hashed-account keys through a reviewed adaptation, which unblocks the core bump (D-139, issue #13); the same `hashed.rs` fix is proposed upstream as RaoFoundation/subtensor#3317.
- The core bump can run on GitHub's hosted runners: `runtime-adopt` takes a `core_bump` dispatch input (D-136, issue #12). Its first run is blocked by v488's hashed Ed25519 keys, which the core patch's erasure guard refuses; the core stays at v478 (D-138, needs a decision).
- v488 hand ports, fifth batch (D-137, issue #12): plans select the Shield route and period as execution does and report it (`Plan::shielded`), `rtao gamma`, and the four limit-order intents with classical identities.

### Receiving addresses in reads; core bump completion (D-142)

#### rtao-primitives

- New: `receiving` (`is_receiving_address`, `parse_recipient`, `account_for_read`, `Recipient`), Python's `bittensor.receiving` for reads: a `tao_` receiving address resolves to its hashed or ML-DSA account through the core's strict decoder.

#### rtao-reads

- New: every catalog read's address parameters (`*_ss58`, `*_ss58s`) accept receiving addresses through `dispatch`, as Python's read coercion (`reads/base.py`); `receiving::coerce_read_params`.

#### rtao-core

- Changed: `keyfile_json` follows the v488 core's keyfile format: written keyfiles carry `accountType` and `signingScheme`, hashed and post-quantum keyfiles their public descriptor (`hashedDescriptor`, or `mldsaDescriptor` with `walletVersion` 2), parsed with the core's checks; a standard `cryptoType` with a descriptor field is refused.
- New: `keys::{decode_hashed_receiving_address, encode_hashed_receiving_address, CRYPTO_HASHED, CRYPTO_HASHED_ED25519, CRYPTO_MLDSA65, CRYPTO_MLDSA65_FIXED}`.

#### Automation

- Fixed: runtime-adopt's push job died of SIGPIPE computing the snapshot message for a core bump (`toml_get` stopped reading a pipe); the third core bump's validated bundle was pushed by hand with the same checks.
- Changed: an open core-bump issue closes on a quiet run once the pins no longer hold a core, with that reason; while a core is held it stays open.

### Opt-in registration Shield default (D-140)

#### rtao-tx

- New: `Executor::with_auto_shield(bool)` (default off): account registration (`HashedAccounts.register*`, `Proxy.create_protected_pure`, `Utility.as_protected_derivative`, also inside wrappers and batches) is planned and executed through MEV Shield when the network has a shield key, as Python's default `shielded=None`; without a key it is planned unshielded with Python's warning. `ExecuteOptions::shielded` and `PlanOptions::shielded` are unchanged.
- New: `contains_account_registration(runtime, call)`, Python's `contains_account_registration`.

### Core patch adaptation for v488 (D-139)

#### Automation (xtask)

- New: `[[core.patch.adaptation]]` in `upstream.toml`, a reviewed diff of fork commits applied after the combined PR-U17/#3251 patch to the core it was recorded for (and later ones until replaced); one for a later tag is staged and only verified. The exact-tree check applies the combined diff and then the adaptation, and a core-patch rebase cherry-picks both. The v488 adaptation is recorded (`rtao-core-patch/v488`).
- New: `keys/hashed.rs` joins the core patch's modify-only path allowlist; the Ed25519 erasure guard also requires, when the core has that file, that its Ed25519 derivation returns `Zeroizing<SigningKey>` and that its regression test and vectors remain. No existing rule changed.
- New: `[core.patch] shared`, the core's in-repository path packages (v488 adds `subtensor-hashed` and `fips204`), kept at the core tag in the fork's patch table and checked against the core's manifests, so the lock still holds one package from the fork.
- New: `cargo xtask core-patch verify-security <commit> [<repository>]`, a local, read-only run of the erasure guard.
- Fixed: runtime-adopt's live-evidence verdict did not compile under the runner's jq 1.7 (a subtraction as an object value needs parentheses), so every adoption or core bump stopped at its evidence stage.
- Fixed: `pin --prepare-candidate` with a moved core patch checked the lock before the candidate re-resolves it, so every such core bump failed at provenance; the lock is now checked by the full `pin --check` after the lock stage.

#### rtao-runtime

- Fixed: `CallBytes::decode` refused calls containing a newtype over a sequence (for example `Vec<Vec<u8>>` wrappers) under v488's core, whose encoder tries a bare list before removing a legacy nesting level. The import bridge's private encoder now names such a field and encodes it as a one-field record, the same bytes under every core.

#### rtao-reads

- Fixed: `subnet_states` and `subnet_state` on a runtime without `SubtensorModule.SubnetState` (482 and earlier) returned the core's "not found in this runtime" error instead of the legacy lifecycle.

### v488 hand ports, fifth batch: planning route, gamma CLI, limit orders (D-137)

#### rtao-tx

- New: `PlanOptions::shielded` and `PlanOptions::period`, so `plan`/`preflight` quote the route execution takes (Python's `plan(shielded=, period=)` and `preflight(shielded=, period=)`); `Plan::shielded` reports the selected route and `Plan::to_json` carries `"shielded"`.
- Changed (source-breaking): `PlanOptions` no longer derives `Default`; its `Default` sets `period: Some(128)`, and struct literals need the two new fields (or `..PlanOptions::default()`).
- Fixed: the Shield submission path quoted its carrier at 8 blocks even for a 4-block shield era.

#### rtao-intents

- New (feature `limit_orders`): `AuthorizeOrder`, `CancelOrder`, `ExecuteOrders` and `ExecuteBatchedOrders` replace `NotYetPorted`, with Python's checks, the owner check, pinned `order_data` and subnet scope; `limit_orders::PreparedOrder` (prepare, `order_id`, check, account approval) and `encoded_order`. A receiving address (`tao_`/`bth`) in an order is refused until the core bump (D-138).

#### rtao-cli

- New: `rtao gamma` (`show`, `list`, `mint`, `mint-with-stake`, `move`, `close`, `set-spender`, `set-cap`, `spend --spend HOTKEY=AMOUNT[:REFERENCE]`).
- Changed: the hotkey of `mint_gamma`, `mint_gamma_with_stake`, `close_gamma_deposit`, `set_gamma_deposit_spender` and `set_gamma_deposit_daily_spend_cap` never falls back to the wallet's hotkey: it is typed on a terminal and refused when missing; its help, and `claim_root_with_hotkey`'s, no longer offers the wallet default. `mint_gamma_with_stake` picks its stake source and `move_gamma` asks for both hotkeys before the amount.

#### Automation

- Changed: a failed core bump keeps its issue open through runs that find nothing to adopt (D-138).

### Core bump through runtime-adopt (D-136)

#### Automation

- New: `runtime-adopt` dispatch input `core_bump` (default false; scheduled runs never bump the core). While the core is held behind the parity pin (`[parity] core_fingerprint`, D-127) and Finney runs the parity runtime, the decision is the new action `core-bump`: the adopt job re-pins the parity source with `RTAO_PIN_CORE_BUMP=1` (core-patch rebase onto the parity tag, downstream core references), checks that only `[core]` moved and the runtime pins did not, gathers live evidence for the pinned runtime, re-resolves the lock, regenerates the core surface and runs the adoption's checks plus full `pin --check` (patch review identity, Ed25519 erasure check, patched and unpatched API probes) and the core, signer and wallet tests; the push job pushes it with a snapshot tag. Backoff, issue and alert handling are an adoption's. When no fork branch carries the rebased patch yet, the run retains the publication candidate in its evidence and stops.
- Changed: the commit guard allows a core bump to change manifests and locks, and in a manifest only the core's `git`/`tag`/`rev` references and their `[patch]` headers; every other tooling path stays refused for every action.
- Fixed: the candidate pipeline's core lock update used `cargo update -p w3f-bls`, which is ambiguous (the git companion and the crates.io package share the name); it is `cargo update --workspace` now.
- Changed: `core-patch` rebase refuses to offer for publication a carried tree that fails the Ed25519 erasure check, and refuses an existing fork branch that fails it, instead of failing later at `pin --check`.

### Since `snapshot-2026-11-07.2`

Not a versioned release. Each item links to its section below.

- v488 hand ports, fourth batch (D-134, D-135, issue #12): the shield carrier is quoted from the actual inner call (the earlier maximum-size quote overstated it about fourfold after the v488 bound change), Shield routes report and check both fees, protected pure-proxy reserves count as spend, and gamma receipts carry `spent`/`skipped`/`minted`. The core bump and the hashed/ML-DSA surface wait on a hosted pipeline run (D-135).
- v488 hand ports, third batch (D-133, issue #12): a later multisig round that must rebuild its call keeps round 1's dynamic inputs and must reproduce its bytes; pure proxies take `account_mode` and quantum-resistant payers create protected pure proxies; nested multisig, announced-proxy and crowdloan calls compose against their real origin with their sudo wrapper. 6 more sources are blessed (84 due).
- Unchanged: core v478 and its patch, runtime stamps, V15, signing status and extensions, the metadata hash, the commit-reveal path, journal formats.

### v488 hand ports, fourth batch: shield quotes, reserves and gamma receipts (D-134)

#### rtao-client

- Changed (source-breaking): `Client::estimate_shielded_carrier_fee(payer, call, period)` quotes the carrier for the actual inner call at a mortal era of `period` blocks, and refuses a call whose ciphertext would exceed the runtime bound. New: `SHIELD_ENCRYPTION_OVERHEAD`, `shielded_ciphertext_size`.

#### rtao-tx

- Changed: a Shield route (a required-Shield intent, or `submit_shielded`) quotes the inner and carrier fees; `IntentPreflight::estimated_fee` and `Plan::fee` are their sum, the preview lists both and the total, and `required_free` includes the carrier. A `max_fee` policy refuses an unavailable total ("max_fee_tao … is configured but the fee could not be estimated") instead of the former shield-specific message.
- New: `Policy::check_with_setup_reserve`; a protected pure proxy's permanent registration reserve counts toward `max_spend` and `Plan::spend`, and its reserves, deposit and every payer's balance are previewed (also for `multisig_approvals` extras).
- New: receipts of `spend_gamma` carry `spent` and `skipped` (with the skip reason), and of `mint_gamma`/`mint_gamma_with_stake` carry `minted`, as Python's executor attaches them; the shield path adds the same operation data and pure-proxy creation coordinates as a clear execution (D-132 resolved).

#### rtao

- Changed (source-breaking): `SyncClient::estimate_shielded_carrier_fee(fee_payer, call, period)`.

#### rtao-cli

- Changed: the shield carrier warning and preflight quote the planned call.

### v488 hand ports, third batch: pinned rebuilds and pure-proxy modes (D-133)

#### rtao-tx

- New: `pinned` (`PinnedInputs`, `scope`, `compose_pinned`, `compose_staking_call`, `derived_u64`, `approved_amount`, `approved_staking_amount`, `approved_function`, `current`, `advance`, `is_active`), Python's `_pinned` module.
- New: `Intent::requires_build_for_policy` and `DynIntent::requires_build_for_policy_dyn` (default false).
- Changed: a later multisig round whose semantic intent requires its build for policy is rebuilt under the pinned inputs and refused unless the rebuilt call equals the pinned bytes ("imported multisig call bytes do not match ..."); other later rounds reuse the pinned bytes as before. Their extras now merge through `multisig::merge_approval_metadata`.
- New: `compose_nested_intent_call`, composing a nested intent as a direct submission from its real origin would be (sudo wrapper for root operations, inner adapter, extras).
- New: `multisig::merge_approval_metadata`.

#### rtao-intents

- Changed: staking, lock, collateral, take, stake-burn and crowdloan-contribution builds take their `ALL` amounts, derived limit prices and the take branch from the approval while rebuilding; `increase_take`, `decrease_take`, `lock_stake`, `contribute_crowdloan`, `create_pure_proxy` and `kill_pure_proxy` are hand builds (no longer generated pass-throughs).
- Changed: `CreatePureProxy` accepts `account_mode` `safe` (`Proxy.create_safe_pure`, extras `pure_proxy_account_mode`); a payer registered as an ML-DSA account or derived authority composes `create_protected_pure` behind `HashedAccounts.check_registration_deposit` and reports its reserves. `KillPureProxy` composes `kill_safe_pure` for `safe`. Unknown modes refuse when building ("account_mode must be 'legacy' or 'safe'") instead of at validation.
- Changed (behavior): `MultisigExecute`, `MultisigApprove`, `MultisigCancel`, `MultisigThreshold1`, `ExecuteProxyAnnounced` and `CreateCrowdloan` compose their inner call against the dispatching account with its sudo wrapper; an announced inner call no longer defaults its hotkey from the caller's wallet. Executing dispatches carry the inner extras.
- Changed: `Batch` sums its children's `protected_pure_registration_deposit_rao`, `pure_proxy_deposit_rao` and `pure_proxy_reserves` and collects `multisig_approvals`.

#### rtao-cli

- Changed: saving a multisig refuses an `account_mode` other than `legacy` or `safe`; a saved group with `account_mode` `safe` refuses to resolve until its derivation is supported, rather than resolving to the legacy account.

### Since `snapshot-2026-11-07`

Tagged as `snapshot-2026-11-07.2`; not a versioned release. Each item links
to its section below.

- v488 hand ports, first batch (D-129 to D-131, issue #12): subnet emission weights allow burn and recycle up to 33% combined; wallet keyfile pairs publish recoverably and an unlock refuses a secret that is not the public companion's key; Polkadot Vault refuses a scan of another scheme; the `services`, `tao_usd_price` and four gamma reads and the `set_service` intent are ported. 22 sources are blessed; 90 remain due.
- v488 hand ports, second batch (D-132): the seven gamma intents (`mint_gamma`, `mint_gamma_with_stake`, `move_gamma`, `close_gamma_deposit`, `set_gamma_deposit_spender`, `set_gamma_deposit_daily_spend_cap`, `spend_gamma`) replace `NotYetPorted`; the pending register keeps the four limit-order intents.
- Unchanged: core v478 and its patch, runtime stamps, V15, signing status and extensions, the metadata hash, the commit-reveal path, journal formats. Rollback to `snapshot-2026-11-07` needs no state migration; a wallet written by this revision is the same pair of files.

### v488 hand ports, second batch: gamma intents (D-132)

#### rtao-intents

- New (feature `gamma`): `MintGamma`, `MintGammaWithStake`, `MoveGamma`, `CloseGammaDeposit`, `SetGammaDepositSpender`, `SetGammaDepositDailySpendCap` and `SpendGamma`, with Python's constructor checks (one spender or service, one cap choice, a rate tolerance in [0, 1) that cannot be disabled, 1 to 512 spend entries with 32-byte references, gamma amounts with at most 9 decimals), mint quotes from `sim_mint_gamma(_with_stake)` (a refused quote is the chain error), the minimum out as the quote less the tolerance, stake mints quoted at the stake held, mint previews (`Quote` facts and the `preview` record), deposit blocks and warnings, and spend contracts (TAO for `mint_gamma`, unbounded otherwise). `SpendGamma` stores its entries normalized, as Python does.
- The receipt's `data["minted"]`, `data["spent"]` and `data["skipped"]` landed with the fourth batch (D-134).

#### rtao-reads

- New: `reads::gamma::sim_mint`, the shared mint simulation; `MintQuote` carries `gamma_units` (not serialized).

#### rtao-wallet

- Fixed (first batch): an unlock no longer requires a public companion that does not exist; a standalone keyfile such as a remote-signer client key unlocks as before, and an existing companion is still compared (D-129).

#### Tests

- All-features builds of the `rtao` and `rtao-testkit` tests name `Transfer::hashed_descriptor`; the blocking client's namespace test takes its read and tool counts from the pinned catalog.

#### rtao-testkit

- Catalog intent states for gamma operations seed deposits owned and spent by the sample coldkey, as Python's `seed_gamma` does; `move_gamma` samples move to a second hotkey.

### v488 hand ports, first batch (D-129 to D-131)

#### rtao-primitives

- Changed: emission weight rules follow runtime 487. Burn and recycle are adjustable slots (`ADJUSTABLE_SLOTS` lists all five) under the combined cap `MAX_BURN_RECYCLE` (33%), reported as `BurnRecycleEmissionAboveMaximum`; `merge` keeps omitted burn and recycle; `renormalize` rescales every unpinned slot and no longer refuses a nonzero burn or recycle; `Rules::table` adds a `burn+recycle` row.
- New: `gamma` (exact decimal gamma amounts and 10^9 chain units: `gamma_amount`, `gamma_units`, `gamma_from_units`) and `pyfmt::is_printable`.

#### rtao-intents

- Changed: `SetSubnetEmissionWeights` accepts nonzero burn and recycle; its vector text lists slots in order.
- New: `SetService` (feature `serving`) composes `SubtensorModule.set_service` with Python's checks: at most 256 UTF-8 bytes, an http(s) url with a host, or empty to remove; the hotkey defaults to the wallet's and the spender to the coldkey.
- `gamma_enabled` plans as an owner hyperparameter (`sudo_set_subnet_gamma_enabled`).

#### rtao-reads

- New: `gamma_deposit`, `gamma_deposits`, `quote_mint_gamma`, `quote_mint_gamma_with_stake`, `tao_usd_price` (the accepted price stepped toward the feed as the chain does) and `services` (unprintable urls as 0x-hex, owner marked only while the owner coldkey owns its hotkey), typed as `reads::gamma::{GammaDeposit, MintQuote, TaoUsdPrice, Service}` with Python's JSON (D-130). They no longer return `NotYetPorted`.
- New: `ReadError::Chain { name, message }` for a runtime API that answers with a dispatch error (source-breaking for exhaustive matches).

#### rtao-wallet

- Changed: creating or regenerating a key publishes the public companion, then the secret, under an advisory lock on the wallet directory, with backups and rollback of only its own files; a failed write leaves the previous pair instead of a new secret beside an old companion. Public companion regeneration takes the same lock (D-129).
- Changed: an unlock reads the public companion first and refuses a secret whose account or scheme differs (`WalletError::IdentityMismatch`, "unlocked signing identity does not match the public wallet"); a companion that exists but cannot be read refuses the unlock. A key with no companion (a legacy hotkey without `hotkeypub.txt`, a standalone or remote-signer client key) still unlocks (D-129).

#### rtao-signer

- Changed: `VaultSigner::validate_signature` refuses a versioned scan whose scheme byte is not the signer's ("the scanned signature uses a different crypto type than this signer").

#### rtao-cli

- Changed: `subnets emission set` takes `--burn` and `--recycle` (default 0) and checks that all five shares sum to 100%; `set-value`, `increase` and `decrease` accept every slot; `show` lists burn and recycle with their range and the `burn+recycle` cap; the read-back lists every slot.

#### Parity

- Blessed (22): the admin-utils, serving, subnet, rate-limiting, rpc_info and eco-tests mock pallet sources; `emission_weights.py`, `intents/emission_weights.py`, `cli/commands/subnet_emission.py`, `test_emission_weights.py`, `intents/hyperparameters.py`, `result.py`, `keyfiles.py`, `vault/signer.py`, `intents/serving.py`, `reads/subnets.py`, `reads/__init__.py`, `namespaces.py`, `namespaces.pyi`, `snapshot.py`, `intents/_money.py` and the new `reads/gamma.py`. The pending register keeps 11 intents and no reads.
- Kept due: `signing.py` and `wallet.py` (their hashed and post-quantum parts), `http_auth.py` and `test_http_auth.py` (the trusted-scheme requirement waits for hashed identities, D-131).

### Since `snapshot-2026-11-05.2`

Tagged as `snapshot-2026-11-07`; not a versioned release. Each item links to
its section below.

- Runtime 488: the parity pin moves from v486 to v488, the runtime Finney finalized at block 9,442,816 (`:code` `0x3147daab…`); testnet runs the same code and is recorded. Generated descriptors, extracted tables and catalogs follow; new upstream reads and intents are generated as pending entry points.
- D-125: `ProxyType::Lock` (21) and `ProxyType::GammaSpend` (22).
- D-126 to D-128 (repository maintenance): the runtime-adopt failures behind issues #10 and #11 are fixed, the core is held at v478 when a parity pin moves the core inputs (core bump recorded as due), and new upstream operations, files, exports and e2e keys become pending ports instead of failing adoption.
- Unchanged: core v478 and its patch, runtime stamps, V15, signing status and extensions, the metadata hash, the commit-reveal path, journal formats. Rollback to `snapshot-2026-11-05.2` needs no state migration.

### Runtime 488 (parity v488)

- Parity moves from v486 (`72323e13`) to v488 (`b6a81d14`). Finney finalized spec 488 at block 9,442,816; four public endpoints agree at the pinned block on genesis, spec, the `:code` hash and the V15/V16 metadata digests, and the release `subtensor.wasm` hashes to the same `:code`. Testnet runs the same code; its 488 row is recorded under `vendor/metadata/testnet/`.
- Runtime 488 contains 487 ("Gamma and quantum-resistant wallets"): new pallet `HashedAccounts` (now in the codegen scope, as upstream covers it), gamma deposits in `SubtensorModule` (`mint_gamma`, `mint_gamma_with_stake`, `move_gamma`, `spend_gamma`, deposit spender and daily-cap calls, `GammaRuntimeApi`), the TAO/USD price feed, service URLs (`set_service`), `LimitOrders` order authorization, safe/protected `Utility`, `Multisig` and `Proxy` calls. Typed descriptors for all of these are regenerated from the pinned metadata; see `docs/compat/v488-upgrade.md` for the metadata drift.
- The upstream catalog grows from 86 to 98 intents and from 105 to 111 reads. The new ones (`mint_gamma`, `mint_gamma_with_stake`, `move_gamma`, `close_gamma_deposit`, `set_gamma_deposit_spender`, `set_gamma_deposit_daily_spend_cap`, `spend_gamma`, `set_service`, `authorize_order`, `cancel_order`, `execute_orders`, `execute_batched_orders`; reads `gamma_deposit`, `gamma_deposits`, `quote_mint_gamma`, `quote_mint_gamma_with_stake`, `tao_usd_price`, `services`) have generated typed inputs and return `NotYetPorted` until ported; `rtao_runtime::compose_by_name` and `rtao call` compose their calls today. Gamma amounts are `rtao_primitives::Amount` (exact decimal or `"all"`) until a gamma unit is ported.
- `rtao-intents` gains features `gamma` and `limit_orders` (both in `all`).
- `rtao-client`: the shielded carrier bound follows the pinned Python, now 32768 bytes (runtime 488's `MevShield` `MaxEncryptedCarrier`; it was 8192), so larger inner extrinsics shield instead of refusing locally.
- `rtao-cli`: a hand-written command that does not offer an intent field (such as `wallet transfer` and the new `hashed_descriptor`) passes the field's catalog default instead of panicking.
- Changed (source-breaking for struct literals): the generated `Transfer`, `TransferAll` and `AnnounceColdkeySwap` gain `hashed_descriptor: Option<String>` (default `None`), `MultisigApprove`, `MultisigCancel`, `MultisigExecute`, `MultisigThreshold1` and `KillPureProxy` gain `account_mode: String` (default `"legacy"`) and `CreatePureProxy` gains `account_mode: Option<String>`, as upstream's intents do. JSON and defaults are unchanged for existing callers. A hashed destination or a non-legacy account mode is refused with `IntentError::InvalidArgument` until its behavior is ported, so no intent silently derives a different account.
- Not ported yet (listed as due): Python `Client.register_wallet`, `prepare_mldsa_call`, `prepare_pq_call`, the call-dependent `estimate_shielded_carrier_fee`, `preflight(shielded=…, period=…)` and `multisig(account_mode=…)`.
- Parity: 111 changed hand-ported sources are listed in `parity/port_due.toml`, 84 new upstream files have pending map rows, new public exports and e2e keys have pending rows; strict parity names them.

### Runtime 487 `ProxyType` variants (D-125)

#### rtao-runtime

- New: `ProxyType::Lock` and `ProxyType::GammaSpend` (runtime 487, part of 488). Composing either under an earlier runtime refuses that call, since the live enum has no such variant.

#### rtao-intents

- `PROXY_TYPES` lists `Lock` and `GammaSpend`.

### Runtime adoption fixes (D-126 to D-128, repository maintenance)

- Finney adoption failed at stage provenance: the core-input fingerprint refused upstream's vendored `vendor/fips204`, which declares its own empty `[workspace]`. A self-rooted path package that inherits nothing is now accepted (inheriting from its own workspace still refuses), and the source-free Cargo projection drops feature entries naming a stripped dev-dependency (`subtensor-hashed`'s `hex/std`) (D-126).
- Testnet recording failed at stage drift: the drift self-test created its scratch directory under `$RTAO_TEMP_ROOT/tmp`, which a fresh runner does not have; and a testnet row recorded for the parity runtime now carries the parity tag and commit that the probe pins need (D-126).
- `pin --parity` holds the core when the parity commit's core inputs differ, records `[parity] core_fingerprint` (verified exactly by `pin --check`) and leaves the core bump to `pin --parity vN --core-bump`; before, the pin tried to rebase and publish the core patch and the adoption's core stage refused (D-127).
- Catalog counts and read categories derive from the pinned catalog; `parity pending --base` (run before regen in runtime-adopt) adds new operations to the pending register; `parity report --adopt` adds pending rows for new watched files, public exports and e2e keys and drops vanished unbound e2e keys. Several extraction adapters accept the v488 source shapes (new test imports, a moved shielded-carrier bound, `dict.update` and `**` in test samples, backend constant tables guarded by `try`/`raise`, an optional future import in description leaves, the paired wallet write's `BT_PW_*` read, an owner setter Python lists only in `HYPERPARAMS`, and the payment wrapper's new declarations) (D-128).
- The adopt step stops on metadata drift only when the signing envelope or runtime identity breaks; breaking call, storage, event and model shapes are regenerated and listed in the adoption's changelog entry. `ci/runtime-adopt-test.sh` no longer counts the checkout's own due ports as an issue of the simulated loop (D-128).

### Since `snapshot-2026-11-05`

Tagged as `snapshot-2026-11-05.2`; not a versioned release. Each item links to
its section below.

- D-122: the commit-reveal tag is followed under any runtime code; a missed reveal limits commits to one per subnet epoch and is reported (`rtao_weights_cr_version_refuted{mech}`) instead of pausing; the reviewed-runtime list is an observation (`rtao_weights_cr_version_reviewed{mech}`). `Unreviewed::Pause` stays opt-in.
- D-123: typed calls whose compiled shape differs compose from live metadata by name with strict per-field checks and a strict import (`compose_by_name` for calls the build lacks); typed reads and owner-monitor events decode by name; intent review warns instead of refusing.
- D-124: `ShieldProtocolGuard::qualify_unlisted` defaults to true.
- Unchanged: runtime stamps, V15, signing status, the metadata hash, unknown signed extensions and shield evidence. No journal format change.

### Since `snapshot-2026-11-03.2`

Tagged as `snapshot-2026-11-05`; not a versioned release. Each item links to
its section below.

- D-098: the self/owner weight-row guard reads the owner's UID at the dispatch block; held queued work reports `OwnerRowHeld`; `WeightScheduler::retire_pending`; `IncorrectWeightVersionKey` names the observed key and block.
- D-110: the commit-reveal tag decision and the reviewed-runtime list live in `rtao_core::commit_reveal` (builds for `wasm32-unknown-unknown`). Changed default: a changed tag under unreviewed code is followed and reported, not paused.
- D-111: `Timestamp.Now` falls back to a by-name read (scheduler anchor and runtime clock worker).
- D-112: the reveal watcher sees a reveal period that changes and returns inside a skipped span. Checkpoint caveat: `period_changed`.
- D-113: bounded `on_finalized` (`with_bounded_dispatch`, `with_recovery_budget`).
- D-114: typed `SetCommitment` intent (feature `commitments`).
- D-115: the downstream `[patch]` set is documented.
- D-120: journal format compatibility range (`readable_by`, `JournalError::Newer`) and explicit compaction of finished operations.
- D-121: versioned release process (`rtao-vX.Y.Z`, curated notes in `docs/releases/`); the first version tag waits for the macOS lane.
- Repository maintenance: the hourly `runtime-adopt` workflow, which approves an adoption on evidence from independent public endpoints (D-116). Testnet runtime 486 is recorded. The build-weight measurement is refreshed for the `commitments` feature's manifest change, and a doc link in `rtao-cli` is qualified so the workspace docs build with `-D warnings`.
- Rollback to `snapshot-2026-11-03.2`: ordinary journals stay schema 3 and open there. A compacted journal (schema 4) and a checkpoint carrying `period_changed` do not; see the sections below.

### Automatic runtime adoption (repository maintenance)

- New hourly `runtime-adopt` workflow (`ci/runtime-adopt.sh`, decision checks in `ci/runtime-adopt-test.sh`). When Finney finalizes a newer spec it pins it (the final `vN` release, else upstream main's release-N merge), imports, extracts, regenerates, reconciles and checks it in a disposable candidate, and if every stage passes pushes the commit to `main` with a changelog entry, an upstream-log entry and a `snapshot-<date>` tag. A testnet runtime missing from its index is recorded with `drift --network testnet --apply`.
- A red stage or a case only a maintainer can decide leaves `main` unchanged, opens or refreshes one issue and, when the `RUNTIME_ALERT_URL` and `RUNTIME_ALERT_TOKEN` repository secrets are set, posts an alert. Upstream branches that declare a spec newer than any network runs keep one informational issue.
- The job that compiles and runs code has a read-only token; the validated commit leaves it as a git bundle, and a separate job that builds nothing pushes main and the tag atomically. A prerelease of the new spec is reported as blocked (pin accepts neither the prerelease nor an unreleased merge while it exists). A run that cannot observe a network does not close an open adoption issue.
- Adoption approves itself on evidence (D-116). A new runtime is acted on only when a second public endpoint reports the same runtime at the same finalized block. After the pin, at least two endpoints (the default list spans two operators; operator diversity is not required, so one operator's outage does not stop adoption) must serve the pinned block with the pinned spec, `:code` hash, BLAKE2b-256 of the `:code` bytes (computed by the workflow) and V15/V16 metadata digests, and a published release wasm must hash to the same `:code`. The built CLI then reads a constant, `Timestamp.Now` and subnet hyperparameters from Finney.
- `cargo xtask parity report --adopt`: unchanged sources carry their reconciliation to the new tag, cosmetic changes auto-bless (the absent Q30 oracle is reported unverified, and extraction is compared without source tags, commits and blobs), generated, extracted and vendored sources are re-derived and checked in the same run, and other changed hand ports are listed in the new `parity/port_due.toml` instead of failing. Ordinary parity accepts exactly those listed changes; strict parity names them as due; `parity bless` removes an entry. One informational issue lists them, without an alert.
- A failed adoption backs off (1 h doubling per failed attempt, measured from when the attempt failed, at most 24 h; the open issue records the state, which another network's report, an unreachable network or a failed decide step does not reset) instead of rerunning hourly, retries at once when main changes the adoption inputs, and alerts once per failure. `workflow_dispatch` gains `force`.
- Witnesses and evidence endpoints must hold the block on their own finalized chain, testnet records are witnessed by testnet's second public name, and an adoption commit that changes the tooling that validated it (scripts, xtask, workflows, manifests, the lock, the toolchain) is refused before it is bundled and again before it is pushed.
- Fixed: GitHub refused to parse the workflow, so neither the hourly schedule nor a dispatch ever ran it. The adopt job's job-level `env` read the `runner` context, which is not available there; those paths are now set by the job's first step.
- No library change: the SDK's run-time compatibility checks and pin provenance are unchanged.

### No runtime lock-in (D-122 to D-124)

A newer runtime never stops the SDK by itself. Checks compare against live
metadata and refuse only what live metadata cannot encode or decode; runtime
code authentication, metadata-hash and signing checks are unchanged.

#### rtao-core

- Changed: `commit_reveal::select_tag` follows the stored `CommitRevealWeightsVersion` under any code by default (`Unreviewed::Follow`). The review list and reveal evidence are reported, never required. New `TagBasis::Refuted` (the latest commitment with this tag under this code missed its reveal with no other explanation) and `TagBasis::CodeUnknown` (no `:code` hash was read) replace the pauses those cases caused. New `TagDecision::basis` and `TagDecision::once_per_epoch` (true under `Refuted`: commit at most once per subnet epoch until a reveal proves the tag again).
- `Unreviewed::Pause` is unchanged and still opt-in. It now also covers the refuted and unidentified cases, and its documentation says it ties commits to the build's list.

#### rtao-neuron

- Changed: the weight scheduler commits with the chain's tag after an unexplained missed reveal instead of pausing. While the refutation lasts it commits at most once per subnet epoch, even under a `RateLimited` cadence, and it warns. A reveal under that code and tag clears it.
- Changed: a failed `:code` read in `WeightPreflight::fetch` warns and the tag is followed (`CodeUnknown`). Before, the preflight failed.
- Changed: `WeightPreflight::check` no longer refuses a stored tag other than 4. New `WeightPreflight::commit_reveal_tag`.
- New gauges: `rtao_weights_cr_version_reviewed{mech}` (the listed spec version of the live code, 0 when unlisted) and `rtao_weights_cr_version_refuted{mech}`. `rtao_weights_cr_version_unreviewed{mech}` is also 1 when the code identity was not read.
- The pause refusal (only under `Unreviewed::Pause`) no longer tells operators to upgrade rtao.
- A new pin no longer has to be added to `REVIEWED_CR_RUNTIME_CODE` for the tests to pass; the test still checks that the pinned reveal path decodes the version 4 payload without reading the tag.
- Owner monitor: changed events are read by field name, so a `Proxy.ProxyAdded` or `ProxyRemoved` with a new `ProxyType` no longer ends the event stream. Positional fields are refused only when the live field lost its compiled type identity. A changed `AlphaAssets.AlphaBurned` contract decodes by name; only an absent entry refuses.
- Miner readiness accepts `serve_axon` and `serve_axon_tls` that resolve by name in live metadata.
- Reads outside `VALIDATOR_READS` decode by name against live metadata when their shape changed, instead of failing.

#### rtao-intents

- Changed: `SetWeights` and `CommitWeights` on a commit-reveal subnet read `CommitRevealWeightsVersion` in their snapshot and tag the commit with it. A `CommitWeights.commit_reveal_version` other than the default 4 is kept as an explicit override.
- The crowdloan callback inspection no longer refuses a decoded callback because the compiled shape of one of its calls changed. Its fields are read by name from the live decode.

#### rtao-runtime

- Changed: `Call::compose` composes a call whose compiled argument shape differs from live metadata by name instead of refusing it. Every live field must be supplied and no other; each value must fit its live type without lenient coercion; the bytes must import through the strict `CallBytes::decode`. It refuses with `ComposeError::SurfaceChanged` only when that fails (a removed variant the value uses, an added or renamed field, a field type the value does not fit). Each such composition warns once per call and runtime.
- New: `compose_by_name` (a call without a compiled descriptor, same checks), `live_compositions` (process-wide count), `call_resolves_by_name`, `event_field_matches`.
- Unchanged: the surface report still classifies a call shape difference as breaking; it is a report, not a refusal.

#### rtao-client

- Changed: typed storage reads (`ViewExt::fetch`, `ViewExt::storage`, unpinned head reads) no longer fail on a changed shape. Keys use the live types, hashers and prefix; values decode strictly against the live type by name, so only a value the typed model cannot represent fails. The difference warns once per runtime and entry.
- Changed: `ShieldProtocolGuard::qualify_unlisted` defaults to true, so an allowlist adapter qualifies a newer runtime from its signing metadata. An adapter that must refuse unlisted code returns false explicitly.

#### rtao-tx

- Changed: intent review reports a wrapped call's breaking surface difference as a plan warning instead of refusing. Composition decides.

#### rtao-cli

- Changed: a raw call (`rtao call Pallet.function`) that this build has no descriptor for is composed from live metadata by name when the runtime has it.

#### Not changed

- V16-only metadata is still rejected; the core parser needs V16 support first. Every current runtime serves V15.

### Owner-row guard and weight refusals at the dispatch block (D-098)

#### rtao-neuron

- Fixed: the self/owner weight-row guard took the owner hotkey's UID from the last-good metagraph. If the owner re-registered at another UID after that graph was taken (for example, by an application that keeps one snapshot and stops the refresh worker), a queued row for the miner now at the old UID was held on every head, and its operation stayed pending with no reason. Under `Hold`, a row for the owner's new UID was signed as an ordinary row. The guard now reads the owner's `Uids` entry at the block the attempt is prepared at, alongside this validator's UID and `SubnetOwnerHotkey`. The read happens only when exactly one other UID carries positive weight, so ordinary rows cost no extra request.
- Queued (journaled) work that the guard holds now reports `WeightAttemptOutcome::Refused` named `OwnerRowHeld` (`policy_violation`) instead of `HoldPrevious`. The refusal names the operation, the block and the held destinations. The work stays pending and is decided again at every head. A held callback row still reports `HoldPrevious`.
- New: `WeightScheduler::retire_pending(mechanism, id)` retires a pending operation that never reached the chain: it has no attempts, or every attempt is `ExpiredUnexecuted`. It is marked `scheduler_retired` (and handed off, so earlier releases skip it too). Its ID stays idempotent, so a replacement needs a new ID.
- `IncorrectWeightVersionKey` (`invalid_argument`, unchanged) now reports the observed `WeightsVersionKey`, its block and the version the attempt carries. For queued work it also says the version is part of the immutable effects and points to `retire_pending`. The check still runs before signing on every path.
- No journal or checkpoint schema change.

### Journal format compatibility and compaction (D-120)

#### rtao-client

- The journal envelope may declare `readable_by`, the oldest schema whose readers can read and update it. This release reads schemas 1 and 2 (migrated to 3), 3 and 4, and a newer schema whose `readable_by` is 4 or lower; it keeps a newer image's schema and unknown fields when it writes. New: `JOURNAL_SCHEMA_MAX`, `JOURNAL_READABLE_BY`, `JournalImage::readable_by`, `JournalImage::check_schema`.
- An incompatible newer journal or record is refused with the new `JournalError::Newer { record, version, readable_by, supported }` and left unchanged.
- `journal::RecordFormat` applies the same rule to SDK-owned records inside a journal: strict at or below the reader's format, a newer record only when it declares `readable_by` within range.
- `OperationJournal::compact(&CompactionPolicy, select)` compacts finished operations: a unique uncontradicted finalized effect, every attempt terminal, effect finalized below `CompactionPolicy::finalized_before`, optionally one scope, and selected by the predicate. The intent and the exact effect attempt stay in `JournalImage::compacted` (`CompactedOperation`); the other attempts keep only ID, hash, role and outcome (`DroppedAttempt`). Unresolved work is never compacted. Returns `CompactionReport`.
- `begin`, `bind` and `record` return the new `JournalError::Compacted` for a compacted ID with the same intent, so it is never dispatched again; a different intent is still `Identity`.

#### rtao-neuron

- Weight work and attempt material (`format`, omitted at 1) and the scheduler checkpoint (`schema`) decode through `RecordFormat`. Format 1 records still refuse unknown fields; a newer record that declares `readable_by` 1 is read, and a newer checkpoint is rewritten as format 1.
- `WeightScheduler::compactable`: a predicate for `compact` that keeps weight work not yet handed to the reveal watcher.
- `WeightScheduler::enqueue` of a compacted ID with the same work succeeds without queuing anything, as redelivery of finished work did before.

#### Journal

- Ordinary writes keep schema 3 and add no field, so revisions before this release still open the journal after a rollback.
- The first compaction raises the image to schema 4 with `readable_by` 4. Revisions before this release refuse it with `unsupported journal schema 4` and leave it unchanged. Compact only when that rollback is no longer needed.

### Versioned releases (D-121)

- Releases are immutable `rtao-vX.Y.Z` tags with curated notes in `docs/releases/`, mapped to D-numbers and stating the journal compatibility range; see `docs/releases/README.md`. The first version is 0.1.0.
- `cargo xtask release-notes` includes the curated notes and refuses a tag without them.

### rtao-neuron

#### Added

- `weights::OwnerRowPolicy` (`Hold`, the default, and `Submit`; `#[non_exhaustive]`) with the public predicate `OwnerRowPolicy::holds` (D-093).
- `WeightScheduler::on_finalized_with`: the callback returns a proposal and its owner-row policy.
- `WeightScheduler::enqueue_with_owner_rows`: queues durable work with a policy.
- `Validator::owner_row_policy`: a provided method that returns `Hold` unless overridden.
- Under `Submit`, an owner burn row goes through the normal path: a direct `set_weights` row, or a timelocked commit with its automatic reveal. In an owner burn row, every positive entry goes to the subnet owner hotkey's UID.
- These rows still hold under `Submit`:
  - a row with positive weight on the validator's own UID when that UID is not the owner's;
  - a row with no positive entry.

#### Journal

- Work queued under `Submit` records `"owner_rows":"submit"` in its effects. Default work keeps its exact bytes, and effects without the key load as `Hold`.
- Restart, recovery and the re-preparation after a rejected commitment use the recorded policy. Changing the policy for an existing operation ID is refused, the same as changed input.
- An earlier release cannot read effects that carry the key, and it refuses every mechanism of that scope. Let `Submit` work finish before rolling back.

#### Fixed

- A durable callback proposal that the self/owner guard holds is no longer journaled. Before, it was proposed again from the journal without the callback, and the mechanism never signed again.

#### Metrics

- New: `rtao_weights_owner_row_total`.
- The re-preparation after a rejected commitment now counts `rtao_weights_hold_total` when it holds.

#### Unchanged

- With the default policy, every existing caller holds exactly as before.

### Validators on a paced endpoint (D-094 to D-097)

An application that drives `WeightScheduler::on_finalized` from its own loop,
without `NeuronRuntime`, against an endpoint paced to a few requests a minute.

#### rtao-neuron

- The scheduler anchors its own timelock clock (D-094). Before each timelocked commit it reads `Timestamp.Now` at the block the commit's preflight is pinned to and observes it on its clock and `ClockGuard`. A `ChainClock::unanchored()` clock no longer needs the runtime's clock worker, and a `SystemClock` gets its skew sample. The clock only moves forward. A failed read warns and counts `rtao_clock_sync_errors_total`; an unanchored clock still refuses the commit. Cost: one request per timelocked commit, none on other heads.
- New: `Clock::observe_timestamp_at` (provided; defaults to `observe_timestamp`), `ChainClock::observe_at`, `ChainClock::is_anchored`.
- A weight dispatch that provably signed nothing, such as an admission timeout, gives its cadence slot back (D-095). The same epoch may commit again after 1 block, then 2, doubling to 32 while such failures repeat. A failure after the hotkey was asked to sign, or with a new signed journal attempt, keeps the slot. Such an attempt reports `WeightAttemptOutcome::Failed`, not `Unresolved`. A journaled release stays uncertain until recovery reports the work rebuildable.
- The checkpoint's per-mechanism slot may carry `unsigned_retry`. Earlier releases ignore it.
- `WeightScheduler::with_submit_deadline` and the provided `Validator::weight_submit_deadline` configure each weight transaction's admission window; the default is `DEFAULT_WEIGHT_SUBMIT_DEADLINE` (30 s) (D-097). The overall bound is the window plus a 128-block era. New metric: `rtao_weights_commit_epoch_slip_total`, for a commit that landed in the epoch after the one it was prepared for.
- `RevealWatcher` reads in full only the blocks that can change a pending reveal outcome (epoch-due blocks, a window's first block, and the first changed block found by bisection) and skips spans whose end state matches its prediction (D-096).
- The runtime's scheduler and coalescing loops follow the newest finalized head (`Backfill::Latest`); the reveal watcher and recovery follow history from their own cursors.

#### rtao-client and rtao

- `Backfill` (`Complete`, the default; `Bounded(n)`; `Latest`; `#[non_exhaustive]`) and `HeadTracker::blocks_with`, `Client::blocks_with` and the blocking facade's `blocks_with` (D-096). `blocks` is unchanged and equals `Complete`. `Latest` never makes a request for a skipped height.
- The shared tracker recovers finalized gaps up to `SHARED_FINALIZED_BACKFILL` (16) once for every follower. After a longer gap it publishes the newest head promptly, and each `Complete` follower recovers the gap itself.
- Preparation and attachment read each value once: the runtime handle and the signing stamp come from one `state_getRuntimeVersion` read (D-097). From the nonce lease to the author call an ordinary submission makes 10 requests instead of 15 or 16.

#### rtao-metagraph

- `MetagraphSync` decides from the finalized height and its cached epoch schedule before making any request, so heads between refreshes cost nothing (D-096). An unpredicted epoch is refreshed at the predicted block or the fallback, whichever comes first.

#### rtao-rpc

- An HTTP 403/429 refusal of a reconnect handshake, after the session has connected, no longer ends the session (D-097). The session stays `Reconnecting`, waits the longer of the refusal's `Retry-After` (at most 10 minutes) and a backoff of 1 s doubling to 60 s per consecutive refusal, and dials again. Without `retry_forever` it becomes terminal after `max_retries` further refusals. A refused initial connect, a JSON-RPC policy error and a policy close (1008) stay terminal. New metric: `rtao_rpc_reconnect_refusals_total`.

### Portable commit-reveal tags, bounded `on_finalized` and commitments (D-110 to D-115)

#### rtao-core

- New module `commit_reveal` (D-110): `NATIVE_PAYLOAD_VERSION`, the reviewed-runtime list (`REVIEWED_RUNTIME_CODE`, `REVIEWED_RUNTIMES`, `reviewed_runtime`), the tag decision `select_tag` (`TagDecision`, `TagBasis`, `TagPause`, `RevealEvidence`, `Unreviewed`) and `TagEvidence<N>`, a fixed-array evidence store. No I/O and no allocation; it builds for `wasm32-unknown-unknown`.

#### rtao-neuron

- The scheduler's commit-reveal tag comes from `rtao_core::commit_reveal::select_tag` (D-110). `NATIVE_CR_PAYLOAD_VERSION`, `REVIEWED_CR_RUNTIME_CODE` and `reviewed_cr_runtime` are now the core's values.
- Changed: a changed `CommitRevealWeightsVersion` under runtime code that is neither reviewed nor proven by a reveal is now followed, not paused. The scheduler warns once and sets `rtao_weights_cr_version_unreviewed{mech}` to 1. An unexplained miss of a commitment that carried the tag under that code still pauses timelocked commits. `WeightScheduler::with_unreviewed_cr_tags(Unreviewed::Pause)` restores the earlier pause. New: `WeightScheduler::commit_reveal_decision`, `RevealWatcher::reveal_evidence`.
- The chain-clock reads of `Timestamp.Now`, the scheduler's anchor read and `NeuronRuntime`'s clock worker, fall back to a by-name read when the compiled shape no longer matches (D-111).
- `RevealWatcher` verifies each pending commitment's queue across skipped spans before its window opens too, and reads its inclusion block in full (D-112). A reveal period that drops (to 0, say) and returns between two read blocks, and reveals or expires a commitment early, is now seen at the block where it happened. A period change while a commitment is pending counts `rtao_weights_reveal_period_changed_total`, and that commitment's miss never refutes a followed tag. The queue read shares the schedule's storage request; on the request-budget fixture the live path makes 31 requests instead of 30 and a restart replay 16 instead of 12.
- Bounded `on_finalized` (D-113): `WeightScheduler::with_bounded_dispatch(wait_blocks)` makes a journaled dispatch wait at most the admission window plus `wait_blocks` blocks; an attempt still unresolved stays journaled and uncertain, and the recovery at the start of later calls resolves it, instead of one call scanning inline until the era ends. `WeightScheduler::with_recovery_budget(transaction_blocks, reveal_blocks)` sets the per-call bounds of that recovery (defaults `DEFAULT_RECOVERY_BLOCKS` = 128 and `REVEAL_BLOCK_READS` = 256); both scans resume from their cursors. New: `RevealWatcher::set_read_budget`, `RevealWatcher::read_budget`.

#### Checkpoints

- A pending commitment whose reveal period changed records `"period_changed":true`. An earlier release cannot read a checkpoint that carries it; let such commitments settle before rolling back. Other checkpoints keep their bytes.

#### rtao-client

- `Fetch` is implemented for `Vec<F>`: a homogeneous list of typed queries coalesced with the rest of a plan.

#### rtao-intents and rtao

- New feature `commitments` with `commitments::SetCommitment`, a typed `Commitments.set_commitment` signed by the hotkey (D-114). It checks raw field lengths, refuses a failed-reveal field and blocks more fields than `Commitments.MaxFields`. It is an rtao addition, outside the generated Python catalog and `AnyIntent`. `all` and the facade's `validator` and `miner` features enable it.

#### Documentation

- The README and the book list rtao's workspace `[patch]` tables and when an application copies each: `w3f-bls` is required to build, and the `bittensor-core` fork patch with its companion table is recommended because it carries the Ed25519 erasure repair (D-115).

### Also in this release, since the last widely pinned revision

- Validator storage reads fall back to name resolution against live metadata after a runtime change (D-089). Ratio and fixed-point types are refused rather than guessed.
- Weight calls can be composed by name or through an application encoder.
- A fresh nonce waits for a pending shielded carrier from the same account instead of taking its inner nonce (D-084), and the shield era anchor stays fresh across that wait.
- Journaled MEV-shield dispatch is supported from per-runtime evidence.
- Commit-reveal follows a changed version under known runtime code, and the evidence is bound to the stored tag.
- `stake add` stakes to a named wallet hotkey without the target picker.
- `rtao-arena` 0.2.0: a fixed-capacity allocator whose heaps take their region lazily on any target (D-090 to D-092).

### Migrating from `hold_previous`

- Nothing changes unless you opt in.
- To commit an owner burn row, pick one:
  - pass `OwnerRowPolicy::Submit` with the proposal through `on_finalized_with`;
  - queue durable work with `enqueue_with_owner_rows(.., OwnerRowPolicy::Submit)` and keep driving `on_finalized`;
  - override `Validator::owner_row_policy`.
- A burn row from a validator that is not the owner needs an effective `MinAllowedWeights` of 1.
- Under a stored `MaxWeightsLimit` below 65535, list only the owner UID. Zero entries would otherwise be spread over by the clipping.
- An application that mirrors the old guard can call `OwnerRowPolicy::holds` with the same policy, so its check agrees with the scheduler's.
