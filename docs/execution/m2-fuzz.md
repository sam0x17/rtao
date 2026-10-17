# M2 robustness targets

## Current source and scope

Current work follows [PLAN.md Q30](../../PLAN.md#current-execution-scope-q30-2026-10-14): client
source and bounded free `ubuntu-latest` checks. M2-S02 is fixed by the
combined core patch at fork rev `626f2a4f`, and M2-26 is complete. The record
below preserves earlier source snapshots and their measured results.

## Historical preparation and validation

At the recorded historical snapshot, all seven targets passed a nightly coverage-instrumented libFuzzer
run with address sanitizer on the Mac Studio. Each requested 15 seconds of
fuzzing. The archived nightly design used 120-second defaults; no schedule is
active, and Q30 requires no new campaign. Machine results are
retained in `m2-fuzz.json`, including the compiler identity, timing and each
process exit status.

The run covered authentication headers, signature verification, production
event splitting, JSON-RPC frames, encrypted-keyfile format detection, the
portable plaintext keyfile codec and metagraph parsing. Keyfile fuzzing uses
public test data and does not decrypt wallet envelopes. Public mnemonic-to-key
derivation can run PBKDF2.

The event target invokes `Events::decode` with pinned spec-478 metadata and
compares the eager records with the production borrowed event ranges. It checks
names, indices, phases, extrinsic positions and topics. A committed nonempty
`System.NewAccount` seed must decode through the core at initialization, so
the harness fails if its positive fixture becomes invalid. Empty and malformed
length seeds cover the entry conditions. No production encoder was added.

The archived nightly workflow design discovers targets through the updated
`xtask fuzz` list. The standalone fuzz lock was refreshed and the three new
targets compile. Instrumented execution is stronger evidence than that compile
check; neither result establishes a hosted run or a milestone exit.

Historical outputs used `target/execute-plan.6Ro0PC/fuzz`. That root is retired
and absent; no instrumented build/tool output there is a current dependency. No local Python, user
wallet or Keychain access was involved.

After retaining the machine report and committed seeds, the exact disposable
corpus and artifact directories were audited for processes, open files,
launchd/configuration references, symlinks and virtual environments, then
removed. The former task instrumented build and tool paths are retired and
absent; shared global Cargo/toolchain caches remain preserved. Future fuzz
campaigns are optional targeted hosted work under Q30.
