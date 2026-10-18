# M5-02 crowdloan and root intent source

The source ports all ten crowdloan operations and both root operations.
Generated dispatch, pending stubs and availability metadata were refreshed
in hosted CI and adopted unchanged. Combined compilation and fixture execution
remain pending. Source review and formatting do not establish execution or
milestone acceptance.

The reference is the immutable Python source at parity commit
`850f2755bb05a3851f948489f0ab27b0645cebce`. Both files were authenticated
against GitHub and the vendored bytes:

| File | Git blob | SHA-256 |
|---|---|---|
| `intents/crowdloan.py` | `a0e8fc6981a0aa63ad5d46aa60a37d6b657567a7` | `8bedf6e8bb0dbc1c12e5050702a08d999001f49bd8ffc4bdc3f836ca12f34599` |
| `intents/root.py` | `dc097fe4177bb426a24552cc0e7109c43e62b438` | `37bc621bffc7d6038655f59ee4cfe4426d5b47f7a60f0b0592616024b36b60ee` |

Crowdloan creation requires exactly one truthy target or nested call. Nested
construction retains the same effective public origin, clock, runtime and
injected view, uses the nested intent's own catalog specification, discards
its result extras and leaves executor wrappers out of the semantic call.
Borrowed views now forward every `View` method, including descriptor observers,
metagraph options and pinned identity, through dynamic dispatch.

Contributing `all` reads the effective coldkey's free balance at that view,
subtracts the existential deposit and 500,000 rao of fee headroom with checked
arithmetic, and refuses a nonpositive remainder with the pinned diagnostic.
Other amount fields require exact TAO. Native balances remain checked u64 rao;
zero remains available for chain validation. An omitted maximum contribution
clears the limit, while an explicit zero encodes `Some(0)`. Summaries, warnings,
spend and default effects retain the Python behavior.

The root threshold setter preserves native amount boundaries and delegates the
chain's policy cap to dispatch. Emission updates reject an empty subnet list,
retain order and duplicates, and atomically batch multiple calls. Both builds
leave the catalog's sudo wrapper to the executor. Their existing verification
reads remain separate from semantic construction.

D-046 records nested-call input limits and private serde carrier refusal.
The tool constructor checks the tree before deserialization; typed builds
check before cloning or nested execution. The scanner uses an iterative walk
and counts escaped JSON bytes without allocating another document. The
recursive-work estimate also limits retained subtree copies.

Fixture source covers every class, JSON round trips, native boundaries, decoded
and encoded calls, exact/all contribution behavior, public origin substitution,
nested semantic calls, input-budget boundaries and default preflight behavior.
The borrowed-view fixture checks exact requests, ordering, misses and cursors
through both concrete and dynamic references without repinning the head.
All use in-memory FakeChain transports; none starts a node or submits a real
transaction. No local build, test, Python process or temporary root was used.

Hosted generation
[38694967521](https://github.com/sam0x17/rtao/actions/runs/38694967521)
at source commit `70fca297b390a169c238b4ddb8a13eb2a5ca15ca` succeeded.
The generated archive is 54,260 bytes, SHA-256
`e57ce2393a61f1296f2008c1a5b215dc0a7b3bd1f8c5a2ad0972bcabec2133ed`.
The report archive is 40,190 bytes, SHA-256
`c238d1f5e9855e7ab6b3314bc209f5909b4914ac11d5bd4ce4f6da5dcf6bcd7a`.
Review checked exactly thirteen regular-file members, clean source identity,
all 23 dependency hashes, unchanged role package sets and counts
225/361/365/416, twelve pending-body removals, nine generated pass-through
additions and the single pre-serde constructor guard. The matrix changes only
the twelve intent statuses, renamed D-045 fixture reference and D-046 addition.
Unchanged read interfaces and other matrix families were checked byte for byte.
The bounded adoption proof is not retained in the repository.
Runtime validation remains empty, and the Apple-target dependency graph was
measured on Linux. These artifacts do not establish native Mac, oracle, hardware
or chain interoperability coverage.
