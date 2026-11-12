## About this release

The first versioned release (D-121) and the first on crates.io (D-181, D-183).
Earlier revisions were pinned by commit or snapshot tag; none of them is a
version. The rendered notes (`cargo xtask release-notes --tag rtao-v0.1.0`)
add the pins, the published packages and this version's changelog section.

Every workspace crate is published at 0.1.0 (`rtao-arena` at 0.2.0). The core
is `rtao-upstream-core` 0.490.0, rtao's republish of the reviewed
`bittensor-core` v490 revision (the core patch, sources byte for byte), with
the five packages it took from Git; no consumer needs a `[patch]`, and
`rtao doctor` checks a workspace's lock. The transport crate is `rtao-jsonrpc`
(formerly unpublished as `rtao-rpc`).
Rust 1.90 is the minimum supported version.

## Changes by decision

The divergence register in PLAN.md §8.4 (D-001 to D-083) describes how this
release differs from the pinned Python package. The changes since the last
widely pinned revision:

- D-083, D-087: durable native work. The versioned operation journal records
  logical work and signed attempts; journaled MEV-shield dispatch works on
  every chain, with safety enforced on the dispatch path.
- D-084: a fresh nonce waits for a pending shielded carrier from the same
  account.
- D-085: `stake add` stakes to a named wallet hotkey without the picker.
- D-086: commit-reveal follows a changed version under known runtime code;
  the evidence is bound to the stored tag.
- D-088: weight calls compose by name or through an application encoder
  after a runtime change.
- D-089: validator storage reads fall back to name resolution against live
  metadata.
- D-090 to D-092: `rtao-arena` 0.2.0, a fixed-capacity allocator with lazy
  regions.
- D-093: `OwnerRowPolicy`; owner burn rows can be submitted.
- D-094 to D-097: validators on a paced endpoint (scheduler-anchored clock,
  slot return after an unsigned failure, bounded backfill and read budgets,
  retried 403/429 reconnects, configurable admission window).
- D-098: the self/owner guard decides from the dispatch block's UIDs; held
  queued work reports `OwnerRowHeld`; `WeightScheduler::retire_pending`
  retires work that never reached the chain.
- D-110 to D-115: the commit-reveal tag decision in `rtao-core`,
  `Timestamp.Now` by name, reveal period changes inside skipped spans, bounded
  `on_finalized`, typed `set_commitment` and the downstream `[patch]` set.
- D-115: superseded by D-183. The downstream `[patch]` set is gone: the core
  graph is republished on crates.io.
- D-116: repository maintenance only: the hourly runtime adoption workflow
  approves a new Finney runtime on evidence from independent public endpoints
  and the release wasm. No library change.
- D-120: journal format compatibility range and compaction of finished
  operations (below).
- D-121: this release process.
- D-122 to D-124: no runtime lock-in. The commit-reveal tag is followed
  under any runtime code and a missed reveal limits commits to one per epoch
  instead of pausing; typed calls, reads and events whose compiled shape
  differs use live metadata by name with strict checks; shield guards
  qualify unlisted code from signing metadata by default.
- D-125 to D-128, D-162 to D-168: runtimes 487 to 490 adopted. New
  `ProxyType` variants; a parity pin whose core inputs move holds the core
  instead of failing; new upstream surface is pending work, not a pin failure;
  adoption fixes in the hourly workflow (repository maintenance).
- D-129, D-135, D-137, D-155, D-161, D-178: the v488 and v490 hand ports and the
  classification of new upstream rows and tests.
- D-130, D-132: typed gamma and service reads; the seven gamma intents.
- D-133: a later multisig round rebuilds a call only when its build carries
  policy metadata, under the approved inputs.
- D-134, D-140: the Shield carrier fee is quoted from the actual inner call; the
  registration Shield default is an opt-in executor setting
  (`Executor::with_auto_shield`).
- D-136, D-138, D-139, D-142, D-169: the core moved from v478 to v488 and v490
  through dispatched core bumps; the reviewed core patch carries adaptations
  that keep the Ed25519 erasure repair for hashed keys and mnemonic seeds.
- D-143 to D-145: safe multisig account derivation (`AccountMode`), receiving
  addresses in reads and limit orders; new CLI multisig groups are safe.
- D-146: superseded by D-183 for the patch tables; the subnet template builds
  the reviewed core.
- D-147 to D-152, D-156 to D-160, D-170 to D-177: hashed (rotating) and
  post-quantum (ML-DSA-65, Falcon) accounts are first class, with btcli's
  defaults: an additive authority signer through the ordinary signing seam,
  wallets, registration, receiving destinations, CLI identities and output,
  multisig fee checks for long proofs. Post-quantum HTTP authentication and
  message signing stay refused, as upstream v490 defines none (D-174); the
  classical-only compatibility protocols refuse those hotkeys up front
  (D-176).
- D-131, D-141, D-153, D-172, D-173: HTTP authentication and message
  verification take each sender's scheme from configuration or finalized chain
  state, never from the request, and fail closed without one. The miner trusts
  finalized chain state by default.
- D-154, D-158: EVM funding through native deposits, multisig protection and
  approval obligations; EVM commands with native receiving identities.
- D-179: the core's strict verifier and password codec replace rtao's staged
  copies; ECDSA key helpers.
- D-180: external ECDSA signatures, verified before attachment.
- D-181 to D-186: crates.io publication. Packaging prep; the transport crate
  renamed `rtao-jsonrpc`; the core graph republished as `rtao-upstream-*`
  packages, crates.io `ruint` (RUSTSEC-2026-0220 fixed) and Rust 1.90; build
  inputs copied into the crates that read them; release notes that fit GitHub's
  limit; the republished core's `MODIFICATIONS` lists the files the core patch
  changes; a macOS release lane that passes with default-feature CLI tests.
- D-187: the pair-budget readiness test no longer depends on host speed
  (repository maintenance; the SDK is unchanged).

## Journal compatibility

Operation journal (`OperationJournal`, native file or injected store):

- Reads schemas 1 and 2 (migrated once to 3, as before), 3 and 4, and any
  newer schema whose `readable_by` is 4 or lower. It keeps a newer image's
  schema and unknown fields when it writes.
- Writes schema 3, byte-compatible with the revisions before this release,
  until `OperationJournal::compact` compacts a finished operation. That raises
  the image to schema 4 with `readable_by` 4.
- Refuses an older-unknown or incompatible newer schema with a version error
  and leaves the file unchanged.

Records inside the journal, each at format 1 with `readable_by` honored:

| record | key | reads | writes |
|---|---|---|---|
| weight work (operation effects) | `format` (absent = 1) | 1; newer with `readable_by` 1 | 1, key omitted |
| weight attempt material | `format` (absent = 1) | 1; newer with `readable_by` 1 | 1, key omitted |
| weight scheduler checkpoint | `schema` | 1; newer with `readable_by` 1, rewritten as 1 | 1 |

A record at format 1 must contain only known fields; an unknown field is
refused, not ignored.

Rollback:

- To the revision before this release (for example `snapshot-2026-11-03.2`):
  safe for an uncompacted journal. That revision reads schema 3 only. A
  compacted (schema 4) journal makes it refuse with `unsupported journal schema
  4` and leave the file unchanged, so compact only when that rollback is no
  longer needed. Work queued with `OwnerRowPolicy::Submit` carries
  `owner_rows` in its effects; revisions before D-093 refuse that work. A
  checkpoint holding a pending commitment whose reveal period changed carries
  `period_changed` (D-112); revisions before D-112 refuse that checkpoint, so
  let such commitments settle first.
- From a later release to this one: a later release that adds journal or
  record fields declares `readable_by`. This release opens its journal when the
  declaration covers it and refuses with a version error otherwise.

Persisted formats are unchanged since D-120: no later decision adds a journal
or record field. Hashed and post-quantum signing keys use the upstream keyfile
formats of the pinned core (D-150).

## Not in this release

- No validated runtime range is recorded in `COMPAT.md`.
- The hosted release workflow is deferred, so there are no hosted Linux
  archives or attestations; install the CLI from crates.io
  (`cargo install --locked rtao-cli`). macOS archives are attached only when the
  operator-run macOS lane ran for this tag.
