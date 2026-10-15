# M2-15 validator metagraph synchronization

The validator policy fetches the documented 20 selected fields without
commitment scans. It refreshes when finalized `LastEpochBlock` advances, or
after 100 finalized blocks since the last successful snapshot. The fallback
continues to work when the epoch read fails. `EveryBlocks` and intervals of at
least 30 seconds are also supported. Full monitoring includes commitments by
default through `SyncPolicy::full()`.

`MetagraphSync::spawn` asynchronously fetches a nonempty finalized snapshot
before returning the handle and diff receiver. `MetagraphHandle::load()`
returns the current `Arc<Metagraph>` through `ArcSwap`, with no request-time
lock or decode. Each refresh builds the hotkey index once. Failed, missing,
empty, duplicate-hotkey or wrong-subnet responses preserve the exact last-good
Arc and publish no diff. Retained health reports the consecutive failure count
and marks it unhealthy after the configured threshold (three by default).
A subsequent successful refresh clears that failure state. The existing
session/head services recover physical connections; sync does not replace a
shared client behind an existing snapshot identity.

`MetagraphDiff` retains registered UID/hotkey pairs, replaced UIDs with both
hotkeys, removed UID/hotkey pairs, permit transitions and complete axon changes.
`score_resets()` returns reused and deregistered UIDs. Diffs name the previous
and new finalized heights; the handle can advance again before a receiver runs.
A lagged diff receiver must reconcile its previous identities with the latest
snapshot before reusing per-UID scores.

The worker owns no strong reference to its lifecycle handle. Caller
cancellation, `Client::closed()`, explicit `shutdown().await` and the final
handle's drop stop it and release pending reads. Its stop guard is constructed
before the task is spawned, including cancellation before the first task poll.
An unanswered graph RPC cannot prevent explicit shutdown. The last-good
snapshot remains available through existing loads after shutdown.

The new curated `mechagraph_with` and `selective_metagraph_with` reads encode
arguments from live metadata and require advertised `SubnetInfoRuntimeApi` v2.
Graph and optional commitment reads share one immutable snapshot. Selective
responses use their actual API output type, a checked typed projection and the
metadata fallback. Raw JSON preserves unrequested `None` fields. Their typed
row/header projection uses zero or empty values for unselected fields. The
chain's selective mechanism API always emits the base netuid, so the header's
mechanism comes from the request. Full mechanism responses use the packed
netuid. The moving-price check resolves the selective field's `Option<Fixed>`
and rejects changed fractional semantics whenever that field was requested.

A mutation regression found that non-breaking compatibility alone could admit
a changed fixed fraction into typed selective decoding. The gate now checks
shape equality; the regression covers typed refusal, dynamic refusal for a
requested price and safe dynamic fallback when the price was not selected.

Validation on 2026-10-14:

- Fifteen sync tests passed, covering UID resets, exact preset selection,
  finalized epochs and fallback, partial raw records, interval timing,
  consecutive failures and recovery, invalid snapshots, pending-RPC
  cancellation, immediate task drop and shared client closure.
- The existing M1 suite passed: 14 decoder tests and six fetch tests. Its two
  environment-only cases remain marked ignored and run through their existing
  child-process checks.
- Rust 1.89 passed all 35 integrated decoder, fetch and sync tests with all
  features. Strict Clippy passed for all metagraph targets and features after
  the final shape-gate correction. Formatting and whitespace checks passed.

The miner preset, debounced event refresh and persistence belong to M3.
`early_on_events=true` currently returns a policy error. M2 validator runtime,
localnet scenarios and soak evidence are owned by the parent task; this record
claims no milestone exit.

No temporary test directories were created. Build outputs and bounded logs
remain under `target/execute-plan.6Ro0PC`; that shared build tree is still live.
No local Python, external filings, commits, pushes or PLAN checkboxes were used.
