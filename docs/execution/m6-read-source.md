# M6 read and blocking source

The remaining 62 catalog reads are authored in five modules: accounts/chain/
identity/neurons (14), delegation/leasing (12), locks/collateral (9), root/
subnets/weights (14) and staking/prices (13). All 102 reads now have source
implementations and reviewed output mappings. Hosted generation at `7953bea`
is verified and adopted: all 102 reads now have generated availability and
dispatch. No M6 step or milestone is accepted by generation alone.

The read fixtures use authenticated runtime metadata, real SCALE state and
fake transports. They check exact declared descriptor sets, immutable hashes,
head and historical reads, absent/default values, malformed payloads, native
amount boundaries, currencies, ordering and batching. Independent reviewers
checked all five modules against the pinned Python and runtime sources.

Review corrected revealed commitment bytes to the core's UTF-8-or-hex
convention. Conviction aggregation now preserves CPython 3.12.11's integer,
compensated-float and generic summation paths, with separate accumulators at
each Python sum. Lock projection retains exact integer comparisons, native
amount checks and wider block counters. Price conversion preserves Python
decimal rounding while refusing negative or oversized chain amounts.

`Client::block_info` and `Snapshot::block_info` add the missing public aggregate
read. Numbered selection handles absent blocks, resolves one hash, decodes
extrinsics with the parent-selected runtime, uses the timestamp inherent when
present and falls back to storage at that same hash. A snapshot requesting a
different explicit height retains its observer on the selected block. Strict
extrinsic errors follow the actual pinned transport behavior. Closed clients
refuse the request before RPC. Source fixtures cover these paths and upgrades.

Curated async aliases reuse the same read bodies. The blocking facade adds a
cold, lazy dedicated runtime and shared connection, owned future dispatch,
all 14 namespaces, immutable snapshots, dual-mode Subtensor, executor wrappers
and shared weight helpers. Generated adapters derive typed catalog methods
from the same read inputs. D-027 records the additional collateral namespace.
The frozen archive runtime API table already exists and is reused.

Runtime review corrected an unbounded join during thread-local teardown.
Worker shutdown now has one deadline, with cancellation replies and a bounded
reaper wait. Its fixture releases blocked teardown before assertions. Facade
review also found an unbounded coordinator join after acknowledgement: thread
local teardown can still be pending at that point. The facade now detaches the
coordinator after its bounded acknowledgement wait. Source review covers
concurrent and reentrant close; hosted execution remains required.

Hosted generation at `afd989a` failed on an unmatched brace in the new
blocking-accessor format string before producing outputs. The generator source
is corrected, with no generated artifact adopted from the failed run.

Only source inspection, rustfmt, Bash syntax checks and offline lock resolution
ran locally. The lock adds edges to existing packages without version changes.
No local compiler, tests, Python, node, container, service, scratch root or
toolchain installation ran. Target output remains 534 MB and private recovery
146 MB. All archives and logs are handled in memory, with bounded evidence
retained under the existing recovery directory.

Run `38715060741` is verified against its exact source commit, all 23 dependency
hashes, both archive digests and fifteen regular allowlisted members. Role
package inventories are unchanged. Review checks all 62 implementation bodies,
output metadata and namespace methods, four input overrides, 14 blocking
namespace structs, 28 accessors and 101 typed blocking forwarders. The intent
interfaces and existing 40 read bodies are unchanged. Matrix changes are only
the 62 read rows and D-027. Private adoption evidence is
`.local/recovery/2026-10-14/m6-reads-adoption.json`.

Ordinary CI must now compile and execute the combined fixtures,
doctests, docs and feature/profile checks. Fresh oracle, interoperability,
chain, native-platform and hardware execution remain absent. Existing goldens
and corpus are separate real fixtures; there is no committed Python oracle
result manifest and no fresh cross-language parity claim.

The acceptance audit also identified missing owner template and book chapters.
The owner tool and owner/shield guides are now authored. Hosted template checks
generate against the changed checkout, build/test each neuron and owner profile
in separate Cargo calls with one target, and only compile the live harness.
The default generated workflow starts no service; optional hosted localnet
requires explicit opt-in. Downloaded tooling is checksum/size pinned, the
sequence is bounded and disposable roots have ownership/cleanup guards.

Remaining acceptance work includes final-range security reports, unresolved
dependency zeroization findings, ordinary fixture mapping for the remaining
upstream test keys, generated per-operation documentation and full hosted
success. The 24 M6 CLI commands now have independently reviewed source and
authored parser, entrypoint and FakeChain fixtures. All four M6 upstream e2e
keys map to ordinary read fixtures; the neuron test compares typed records with
raw runtime output. Hosted execution and acceptance remain pending. The root
basket table is adopted from `c281803` generation: 45 samples and 171 baselines
match the authenticated extraction. The shared catalog exclusion and latest
matrix need the next hosted refresh. Later milestones still require work.
