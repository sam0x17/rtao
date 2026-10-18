# M1 performance and robustness evidence

M1-20/Q30 currently requires review of the client benchmark/fuzz source and
ordinary fixture checks. Benchmarks, sanitizer/fuzz campaigns, full powersets
and nightly escalation are optional targeted hosted work. They are not local
or milestone prerequisites. The measurements below are historical results.

The Criterion suite contains 34 measurements and a committed
[Mac baseline](../../vendor/bench/baselines/aarch64-apple-darwin.json).
It measures raw decoding and full projection separately for all four metagraph
fixtures, V15 parsing, 10,000 storage keys, a 256-row map page including its free
keys and values, signing and assembling an extrinsic, and btauth signing,
authentication and replay commitment for both algorithms.

The baseline records Apple M5 Max, Rust 1.99.0, 30 samples, a one-second warmup
and one-second target measurement time. It was measured from a stable source
copy because the original working tree was changing concurrently. Its original
HEAD, dirty state and exact source snapshot hash are recorded; it is a measured
development snapshot, rather than a claim about the earlier committed HEAD.
Concurrent work on this shared host affected some sample times. These local
absolute timings must not be used to gate Ubuntu builds or presented as the
older prototype's 6.5-microsecond result.

`xtask bench` rejects source changes during measurement, missing benchmark IDs,
mismatched contexts, invalid estimates and regressions above 30%. Four focused
tests pass on current Rust and Rust 1.89, including the strict threshold and
invalid non-finite CLI settings. A repeated measurement of the same stable
source passed the actual 30% gate. An earlier comparison correctly rejected
large changes while other builds competed for CPU. The archived nightly design
compares reference/current revisions sequentially on one free Ubuntu runner;
its workflow is deferred and no schedule runs in current development.

The instrumented fuzz result (raw JSON not retained) recorded nightly 1.101.0,
cargo-fuzz 0.13.2 and address sanitizer. Each target ran for 15 requested seconds
(16 reported by libFuzzer), with all four passing:

| Target | Executions |
|---|---:|
| btauth headers | 306,744 |
| btauth verify | 82,989 |
| JSON-RPC frame | 88,107 |
| metagraph view | 39,926 |

The verifier seeds valid raw and `<Bytes>`-wrapped signatures for both algorithms,
asserts strict-policy rejection of wrapped signatures, mutates individual
headers/body/target/time, and checks that every accepted request is then rejected
as a replay. The JSON-RPC target drives the production inbound/dispatch path
with read, subscription, submit, cancelled, live and closed request states. It
checks binary/text equivalence and bounded notification queues; seven added
parser tests pass, including on Rust 1.89. The metagraph target receives arbitrary
bytes and mutations of the four committed real/synthetic payloads. The standard
decoder differential preflight also passed before measurement.

The facade's depth-two feature powerset completed all **306 configurations**.
Strict Clippy passed for all four benchmark crates and for xtask. Rust 1.89
compiled all four benchmark harnesses and passed the four benchmark-driver and
one fuzz-driver tests. Nightly YAML parses, all actions are pinned by SHA, and all
four jobs use free `ubuntu-latest` runners.

No fresh benchmark/fuzz/core-canary campaign is required for M1 acceptance.
The earlier core-canary and localnet extensions remain deferred reference work;
do not start a local replacement. If a client change needs a targeted hosted
measurement, record its exact source, inputs and limits without treating the
historical Mac baseline as an Ubuntu threshold. Current pin/BLS checks use the
approved Q26 built-dependency policy; no literal two-lock-entry gate is restored.

[Build weight](m1-build-weight.md) records the measured role-profile counts.
