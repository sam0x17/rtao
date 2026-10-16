# M2 transaction policy equivalence

## Current source and scope

Current work follows [PLAN.md Q30](../../PLAN.md#current-execution-scope-q30-2026-10-14): client
source and bounded free `ubuntu-latest` checks. M2-S02 remains open and
unaccepted, so M2-26 and the M2 milestone exit remain open. The record below
preserves earlier source snapshots and their measured results.

## Historical preparation and validation

`crates/rtao-tx/tests/policy_equivalence.rs` checks the three M2 trusted
constructors from `bittensor-core/host` at upstream `v478`, commit
`850f2755bb05a3851f948489f0ab27b0645cebce`.

| Core trusted constructor | Production rtao intent |
| --- | --- |
| `IntentCall::transfer` | `Transfer { keep_alive: true }` |
| `IntentCall::transfer_allow_death` | `Transfer { keep_alive: false }` |
| `IntentCall::transfer_all` | `TransferAll`, plus `Transfer { amount_tao: Amount::All }` |

The harness invokes each implementation's actual `Policy::check`. It compares
violation categories in order and the execution verdict. It fails on an unknown
diagnostic instead of silently dropping it. Separate assertions retain Python's
exact fee, spend, sweep, and raw-call messages from
`sdk/python/bittensor/intents/plan.py` at the same pin.

The 73,728 comparisons vary the amount, fee estimate, fee cap, spend cap, raw
opt-in, and subnet allowlist. Cases include unavailable estimates, zero, exact
caps, one rao over a cap, `2^53 - 1`, `2^53`, `2^53 + 1`, `u64::MAX - 1`, and
`u64::MAX`. Both sweep entry points are unbounded under every configured spend
cap. Transfers retain Python's empty subnet scope, including when the allowlist
is empty. Additional tests reject alpha as a TAO transfer amount and reject
negative or overflowing external native balances (Q25 / D-035).

Using the pinned V478 runtime served by `FakeChain`, all 24 paired intent
instances also build exactly the bytes returned by the trusted core
constructor's real `Runtime::compose_call`. The check uses only public account
identities and asserts that the fake chain receives no submission.

Validation on 2026-10-14, with all artifacts under
`target/execute-plan.6Ro0PC/build` and temporary files under its `tmp` directory:

```text
cargo test --locked -p rtao-tx --test policy_equivalence
7 passed; 0 failed; 0 ignored; 0.62s

cargo +1.89 test --locked -p rtao-tx --test policy_equivalence
7 passed; 0 failed; 0 ignored; 0.68s

cargo clippy --locked -p rtao-tx --test policy_equivalence -- -D warnings
passed (Rust 1.99)
```

This evidence covers the three M2 constructors. The M4/M5 constructors and live
localnet core-oracle lane remain separate evidence. The localnet comparison
is optional targeted hosted work under Q30. Raw-call opt-in is tested
against Python's separate gate; arbitrary core calls have broader conservative
semantics and are outside the trusted-constructor equivalence claim.

The production dependency graph is unchanged. `rtao-intents/transfer` and
`bittensor-core/host` are dev-dependencies of the TX harness. No task-specific
scratch directories were created. The former task root and build directory
are retired and absent; preserve shared global Cargo/toolchain caches.
