# M2-06 portable receipt, fee and inclusion helpers

## Current source and scope

Current work follows [PLAN.md Q30](../../PLAN.md#current-execution-scope-q30-2026-10-14): client
source and bounded free `ubuntu-latest` checks. M2-S02 is fixed by the
combined core patch at fork rev `626f2a4f`, and M2-26 is complete. The record
below preserves earlier source snapshots and their measured results.

## Historical preparation and validation

`rtao-core-ext::ops::{receipt, dispatch_error, fee, extrinsic_lookup}` now
provide protocol helpers without transport, nonce-cache mutation or signing.
Callers supply the runtime selected for the included block. The public modules
carry PR-U1 or PR-U2 staged probes and are re-exported through `rtao-core`.

Receipt resolution follows the pinned Python event order. An outer
`System.ExtrinsicSuccess` succeeds only when there is no resolved failure.
The six nested-result events use their source-defined fields, including
`Sudo.Sudid.sudo_result`, `Sudo.SudoAsDone.sudo_result` and the bare Utility
dispatch errors. All five MevShield failure events retain their Python error
and weight behavior. The last `TransactionFeePaid.actual_fee` overrides
deposit fallback; otherwise Treasury and Balances deposits are summed.
Amounts are checked native `u64` rao, including fallback overflow rejection.

`cargo xtask extract` renders the nested-event table and exact system-error
descriptions from the authenticated Rust-only named-table corpus. Separate
private generated files let dispatch errors move upstream with PR-U1 while
receipts remain staged for PR-U2. Module errors resolve pallet and error
indices through the supplied runtime, preserving metadata documentation.
Legacy and modern module-error layouts and Python diagnostic representations
for strings, bytes, tuples and other values are covered. Result messages use
Python's truthy docs, then name, then full error-map fallback. String lists
join with spaces; other reasons preserve Python `str`, including bytes and
dictionary MevShield reasons. Non-string list members are rejected, matching
Python's string-join failure.

The fee probe accepts the caller's uncached nonce and encodes an immortal
extrinsic with a zeroed 64-byte signature, zero tip and disabled metadata
hash. Its request appends the little-endian `u32` encoded length before
calling `TransactionPaymentApi_query_info`. Strict decoding uses the live
runtime-API output type and rejects trailing bytes and fees outside `u64`.
Extrinsic lookup hashes the complete raw SCALE bytes with BLAKE2b-256, returns
an optional index and preserves Python's exact absent-inclusion diagnostic.

The FakeChain regression composes `Sudo.sudo(Balances.transfer_keep_alive)`
and seeds actual SCALE-encoded `System.Events` through the runtime codec.
The modeled inner Root dispatch emits `Sudid(Err(BadOrigin))` followed by
outer `ExtrinsicSuccess`. Lookup finds index 1 in block 12, both RPCs use
the same block hash, and decoded attributes contain `sudo_result` without
the legacy core's assumed `result` field. Receipt resolution returns
`success: false` with the `BadOrigin` error. FakeChain models these events;
a separately executed optional hosted M5 scenario could supply live runtime
evidence. No such result is inferred from these fake events.

Validation on 2026-10-14:

- All seven `receipt_protocol` tests and the encoded FakeChain `receipts` test
  pass with current Rust and Rust 1.89.
- Strict all-target Clippy passes for `rtao-core-ext`, `rtao-testkit` and
  `xtask`.
- The authenticated-table generator test and full `extract --check` pass.
- `staged --check` passes for all 15 modules, retaining these four portable
  modules because their PR-U1/PR-U2 APIs are absent from the pinned core.
- Focused rustfmt checks pass for all changed protocol and test sources.
- An independent read-only review against the pinned Python helpers and
  actual core event decoder found no evidence-backed correctness issues.

Sources are pinned at commit
`850f2755bb05a3851f948489f0ab27b0645cebce`. The receipt source SHA256 is
`8875778665d50a1d65afc057d07706f1a3bfc3d9e2928cd66422a2fe54bc8d6d`;
the error-map SHA256 is
`65efe42793da7201a0707f4648cd8801ea1206bebd47d75e4af74e6f90767db3`.

Historical builds and logs used a task-scoped temporary root. That root is
retired and absent; its scratch/log paths are historical provenance.
The four new staged probe directories were removed after process, open-file,
launchd, configuration, symlink and virtual-environment checks found no live
dependency. Their bounded staged-check log is not retained in the repository.
No Python interpreter, wallet writes, chain submission, commits, pushes,
external filing or PLAN checkbox edits were performed by this work.
The former task build/staged-probe/tool paths are retired and are not current
parent-task dependencies. Preserve shared global Cargo/toolchain caches.
