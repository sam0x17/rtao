# M2 SS58 input addresses

## Current source and scope

Current work follows [PLAN.md Q30](../../PLAN.md#current-execution-scope-q30-2026-10-14): client
source and bounded free `ubuntu-latest` checks. M2-S02 is fixed by the
combined core patch at fork rev `626f2a4f`, and M2-26 is complete. The record
below preserves earlier source snapshots and their measured results.

## Historical preparation and validation

`Ss58Address` separates an intent's validated input spelling from its binary
chain identity. It retains the caller's exact string for display, JSON and
summaries. `account()` returns the decoded `AccountId`; `IntoCoreValue` encodes
that account's 32 public-key bytes. Chain and storage `AccountId` behavior,
including its default prefix-42 JSON rendering, is unchanged. Balance behavior
remains checked native `u64` rao.

The carrier uses the pinned core's SS58 decoder. Pinned Python's `ss58_decode`
also calls `public_key_from_ss58`, which delegates to
`AccountId32::from_ss58check`. Accepted network prefixes 0, 2, 42 and 110 are
covered, including the two-byte prefix encoding. Custom prefix 1000 is rejected
by the pinned decoder even though its encoder can produce that string. Invalid
checksums, surrounding whitespace and non-string JSON are rejected.

The generator's scalar and plural address rules in
`codegen/intent_types.toml` select `Ss58Address` and `Vec<Ss58Address>`, retaining
optional wrappers. Generated call conversion explicitly selects decoded account
identity. The transfer implementation uses the same checked `IntoCoreValue`
conversion. The input carrier has no SCALE or wire-decoding traits.

The transfer regression covers 18 combinations: prefixes 0, 2 and 110; fixed
transfer, the all sentinel and `TransferAll`; and both keep-alive values. Each
case checks the original JSON and summary, a tool round trip and call bytes
equal to the corresponding binary-account constructor. The production executor
tool test also checks prefix-0 plan arguments and summary and verifies the
submitted call equals the reviewed call.

Validation on 2026-10-14 used `--locked` throughout:

| Check | Rust 1.99 | Rust 1.89 |
| --- | --- | --- |
| Primitive address-input tests, all features | 5 passed | 5 passed |
| Primitive address-input tests, no default features | 3 passed | 3 passed |
| Transfer tests, including the 18-case regression | 4 passed | 4 passed |
| TX executor and core-policy integration tests | 16 + 7 passed | 16 + 7 passed |
| Strict all-target Clippy, primitives/client/intents/TX | passed | passed |

Owned Rust formatting passed with edition 2024. Historical logs are not
retained in the repository. No separate disposable directories
were created. That task build/tmp root is retired and absent; shared global
Cargo/toolchain caches remain preserved. Oracle and localnet validation are
optional targeted hosted evidence under Q30, not development milestone gates.
