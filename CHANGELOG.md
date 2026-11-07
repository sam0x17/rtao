# Changelog

## Unreleased

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
