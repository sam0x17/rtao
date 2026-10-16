# M3 miner metagraph synchronization

The validation below describes the 2026-10-14 preparation checkpoint. Q30
requires applicable hosted client checks and retires its local build campaign.

`Preset::Miner` selects exactly `Hotkeys`, `ValidatorPermit`, `TotalStake` and
`Validators`. `SyncPolicy::miner()` defaults to a 600-second interval, no
commitments and early refreshes from finalized events. The existing minimum
interval remains 30 seconds for every preset.

The worker reads `System.Events` at each new finalized block when event
refreshes are enabled. `NeuronRegistered` and `AxonServed` are relevant only
when their netuid matches. `HotkeySwapped*` records with a netuid must match
the subnet; global swaps must name an old or new hotkey in the current graph.
Other pallets and unrelated subnets or global hotkeys do not trigger a graph
fetch. Event decoding uses the shared client and its pinned runtime.

The first relevant event opens a one-second window. Later events in that
window share the pending refresh without moving its deadline. This coalesces
bursts while allowing a continuous stream to refresh. The refresh pins the
latest finalized block when the window expires, retaining the selective
fetch, hotkey index and atomic publication behavior from M2. A cadence refresh
that succeeds during the window satisfies the pending event refresh.

Event read errors contribute to retained health and schedule a conservative
selective refresh because the unread block may contain a membership change.
Failed event refreshes retain that invalidation and retry after 30 seconds.
Missing, malformed or failed graph reads keep the last-good snapshot and
publish no diff. Caller cancellation, client closure, handle drop and explicit shutdown also cover
pending event reads and debounce windows.

Validation on 2026-10-14:

- All 21 sync tests pass on current Rust and Rust 1.89, covering the six M3
  additions and 15 M2 regressions. The M3 cases
  check exact miner defaults and native selective wire arguments, the
  ten-minute interval, each relevant event family, unrelated events, burst
  coalescing to the latest finalized height and cancellation during an
  unanswered event read. Two failure-recovery regressions verify conservative
  refresh after an unread event block and a retained event invalidation with
  a bounded retry delay after a failed graph fetch.
- The current all-feature suite passes all 48 decoder, fetch, persistence and
  sync tests. Its two environment-only cases remain ignored in the parent
  process and pass through the existing child-process regressions.
- Strict Clippy passes all metagraph targets and features. Persistence details
  and focused MSRV validation are recorded separately.
- Formatting and whitespace checks pass for the edited files.

The new native `u16` immunity-period read and its validation are recorded in
`m3-immunity.md`. The M2 pending-read regression was updated for that read;
the historical M2 execution record remains unchanged.

This work created no temporary directories. Its original logs and shared build
outputs used `target/execute-plan.6Ro0PC`; Q30 cleanup retired that tree and
its raw logs. Do not recreate it. No current hosted result or milestone exit
is established by this historical record.
