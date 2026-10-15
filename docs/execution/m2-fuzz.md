# M2 robustness targets

All seven committed targets passed a nightly coverage-instrumented libFuzzer
run with address sanitizer on the Mac Studio. Each requested 15 seconds of
fuzzing; hosted nightly defaults remain 120 seconds. Machine results are
retained in `m2-fuzz.json`, including the compiler identity, timing and each
process exit status.

The run covered authentication headers, signature verification, production
event splitting, JSON-RPC frames, encrypted-keyfile format detection, the
portable plaintext keyfile codec and metagraph parsing. Keyfile fuzzing never
decrypts files or runs a KDF. Its committed seeds are public test data.

The event target invokes `Events::decode` with pinned spec-478 metadata and
compares the eager records with the production borrowed event ranges. It checks
names, indices, phases, extrinsic positions and topics. A committed nonempty
`System.NewAccount` seed must decode through the core at initialization, so
the harness fails if its positive fixture becomes invalid. Empty and malformed
length seeds cover the entry conditions. No production encoder was added.

The existing nightly workflow discovers all targets through the updated
`xtask fuzz` list. The standalone fuzz lock was refreshed and the three new
targets compile. Instrumented execution is stronger evidence than that compile
check; neither result establishes a hosted run or a milestone exit.

All outputs are under `target/execute-plan.6Ro0PC/fuzz`. Shared instrumented
builds and tool caches remain needed for later checks. No local Python, user
wallet or Keychain access was involved.

After retaining the machine report and committed seeds, the exact disposable
corpus and artifact directories were audited for processes, open files,
launchd/configuration references, symlinks and virtual environments, then
removed. The instrumented build cache and tool installation remain live for
the parent goal's later checks.
