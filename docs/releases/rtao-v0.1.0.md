## About this release

The first versioned release (D-121). Earlier revisions were pinned by commit or
snapshot tag; none of them is a version. Pins, archives and the full changelog
follow in the rendered notes (`cargo xtask release-notes --tag rtao-v0.1.0`).

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
- D-116: repository maintenance only: the hourly runtime adoption workflow
  approves a new Finney runtime on evidence from independent public endpoints
  and the release wasm. No library change.
- D-120: journal format compatibility range and compaction of finished
  operations (below).
- D-121: this release process.

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

## Not in this release

- No validated runtime range is recorded in `COMPAT.md`.
- The hosted release workflow is deferred, so this is a source release with
  the macOS lane's archives; there are no hosted Linux archives or attestations.
