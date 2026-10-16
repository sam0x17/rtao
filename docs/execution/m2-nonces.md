# M2 shared nonce leases

## Current source and scope

Current work follows [PLAN.md Q30](../../PLAN.md#current-execution-scope-q30-2026-10-14): client
source and bounded free `ubuntu-latest` checks. M2-S02 remains open and
unaccepted, so M2-26 and the M2 milestone exit remain open. The record below
preserves earlier source snapshots and their measured results.

## Historical preparation and validation

`NonceManager` now reserves typed `NonceLease` values for ordinary or paired submissions. Every client clone shares the same account cells. Each cell serializes allocations with a Tokio mutex while its cached value and generation use a synchronous mutex, so an unsettled lease clears the account immediately in `Drop` without awaiting or scheduling work.

The first allocation queries the pool-aware `account_nextIndex`; later allocations increment locally. `fresh()` queries without changing the cache, `pin()` records an explicit account nonce, and `lease_pair()` reserves consecutive inner/carrier values atomically. Valid SS58 aliases normalize to the same account cell. The earlier arbitrary-string source-test helpers remain available; their asynchronous pin wrapper is now named `pin_for_address()`.

A cache generation prevents an old RPC response from overwriting a clear or explicit pin that occurred while the request was pending. Cancelled first queries release their account's allocation lock without consuming a nonce. An account waiting on its RPC does not block another account. Dropping either unsettled half of a pair clears that account; settlement consumes its lease after acceptance or inclusion.

Eight production tests pass: 64 concurrent allocations over cloned managers, fresh-read purity and unsettled drop, atomic pairs and overflow, SS58 aliases, explicit-pin races, cancellation, synchronous-clear races, and independent-account progress. The integrated submission tests now verify settlement, failures, interruption and reconciliation; see [M2 submission evidence](m2-submission.md).

No Python, user wallet or Keychain state was accessed. No fixture directories were created. The former task execution root and its build/log paths are retired and absent; no current dependency remains there. Preserve shared global Cargo/toolchain caches.
