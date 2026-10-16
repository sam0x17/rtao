`rtao-reads` implements the commitment status and decoders from pinned
`sdk/python/bittensor/metagraph.py:37-125,258-291,406-571`, plus the raw
`metagraph` and `token_symbols` reads from `reads/subnets.py:170-190,217-226`.
The reference commit is `850f2755bb05a3851f948489f0ab27b0645cebce` (v478).

Commitment scans pin the actual public `View` before reading `CommitmentOf`,
`RevealedCommitments` and optional `Uids`. Revealed history preserves entries
whose live registration was removed. Pending rounds, terminal failures and
reveals retain Python's precedence, including readable plain fields beside a
sealed or failed payload. Raw fields concatenate before UTF-8 decoding; invalid
UTF-8 remains hex. Reveal decoding skips all four compact prefix forms with
Python's behavior on truncated prefixes. `reveals_at` calls the pure primitives
drand helper. Deposits use checked native u64 balances and Python display JSON.

The source test `test_failed_timelock_commitment_is_terminal_and_not_revealed`
is ported with its original name and provenance. The locks test in the same
Python source belongs to M6 and is outside this change.

Validation on the Mac Studio used the recorded execution temporary root and
did not execute Python:

```text
cargo test -p rtao-reads --all-features
  10 unit tests passed; 2 commitment integration tests passed;
  2 subnet read integration tests passed; no ignored tests.
cargo clippy -p rtao-reads --all-targets --all-features -- -D warnings
  passed
cargo +1.89.0 check -p rtao-reads --all-features
  passed
```

The integration tests use `FakeChain` through `Client::from_rpc`, pinned v478
metadata and real SCALE storage seeds. They verify core account rendering,
registered and deregistered hotkeys, a single block hash across all maps, and
snapshot retention after the head advances. The first reads also exercise the
runtime API and compact UTF-8 name/symbol fields. Large map decoding and
commitment projection run through `offload`.

`Subnets::view()` exposes the underlying view to the curated metagraph trait.
At that checkpoint, the namespace type/accessor was a hand-written integration
contract. The current namespace is category-generated; its earlier integration
requirement is historical. Dispatch belongs to M2. This evidence does not claim
M0 or M1 exit.
No additional disposable directory was created. The former execution build
root is retired and absent; it is not a current live dependency. Current
substantial checks use free `ubuntu-latest` CI under Q30, preserving shared
global Cargo/toolchain caches.
