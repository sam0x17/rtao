# M3 robustness targets

All eight committed targets passed coverage-instrumented nightly libFuzzer
execution with AddressSanitizer on the Mac Studio after the Unicode/keyfile
fixes and authentication middleware retention changes. Each requested 15
seconds and reported 16 seconds of fuzzing. `m3-fuzz.json` records compiler
identity, source and binary hashes, timing, execution counts and exit status.

| Target | Executions | Covered edges | Exit |
| --- | ---: | ---: | ---: |
| `btauth_headers` | 235,723 | 1552 | 0 |
| `btauth_server` | 86,974 | 3611 | 0 |
| `btauth_verify` | 113,212 | 2760 | 0 |
| `event_splitting` | 76,545 | 2055 | 0 |
| `json_rpc_frame` | 139,223 | 3686 | 0 |
| `keyfile_detection` | 397,206 | 32 | 0 |
| `keyfile_json` | 293,372 | 3062 | 0 |
| `metagraph_view` | 36,917 | 1085 | 0 |

The run used cargo-fuzz 0.13.2 and nightly 1.101.0 (080cc2714, 2026-10-11).
Locked offline metadata passed before execution, and the standalone fuzz lock
remained unchanged. Each target's log contains loaded inline 8-bit counters,
a PC table and a completed execution count. Binary symbol and linkage records
confirm sanitizer coverage functions and the AddressSanitizer runtime. These
results include instrumented execution after compilation.

The authentication middleware target mutates headers, body and encoded URI,
exercises policy denial and body limits, and checks immediate replay rejection
with both signature algorithms. Its clock is fixed, so this run does not
establish asynchronous expiry or retention race behavior. The keyfile targets
exercise format detection and plaintext codec round trips with public seeds.
They do not decrypt wallet envelopes. Public mnemonic-to-key derivation can
run PBKDF2. Event splitting, JSON-RPC frames and metagraph parsing also ran. No
local Python or Docker process was used.

Logs, toolchain records, source hashes, binary instrumentation records and the
preflight metadata remain under
`target/execute-plan.6Ro0PC/fuzz/integration-refresh`. After retaining the
results, the task-owned corpus and artifact directories were checked for
processes, open files, launchd/configuration references, symlinks and virtual
environments, then removed. Shared instrumented builds and tools remain live
for the parent goal's later checks. This local run establishes neither a
hosted workflow result nor a milestone exit.
