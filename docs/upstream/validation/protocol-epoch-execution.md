# PR-U1, PR-U2 and PR-U5 execution record

All three filings use the authorized `sam0x17/subtensor` fork and Rust 1.89 on
macOS ARM64. Their clean committed HEADs passed the retained
[Rust gate](protocol-epoch-rust-gate.sh) through the
[pre-push hook](protocol-epoch-pre-push-hook.sh). No force push, history
rewrite, pushed-branch deletion or hook bypass was used. No rtao commit or
push was made by this worker.

The upstream clone's AGENTS.md requires the `unarbos` actor and the full
monorepo `scripts/preflight.sh`. The user's authorized GitHub identity and
instruction to keep Python in CI take precedence. The task-local hook checks
the exact actor, fork remote, authorized branch, clean committed HEAD and
changed scope (`sdk/bittensor-core/**` plus the expected Cargo.lock change),
then runs the scoped Rust tests, strict core/binding Clippy, wasm check and
formatting. The full upstream monorepo preflight was not run.

The nested clones could inherit rtao's consumer-specific Clippy configuration.
An empty task-local `CLIPPY_CONF_DIR` stopped that parent configuration from
being discovered while retaining upstream Cargo lint rules. Portable wasm
validation used the installed `1.89` toolchain alias; other commands used
`1.89.0`. Both are Rust 1.89.0.

The three clones shared the recorded task build cache. Copied source
timestamps initially allowed Cargo to reuse another clone's local package
artifact. The final sequential gates touched core and binding `lib.rs` before
each run to force compilation of the selected clone. The files' contents did
not change. The retained logs identify the compiled checkout paths.

The first U1 core/binding Clippy command omitted `PYO3_NO_PYTHON=1`. PyO3
invoked local Python 3.9 and failed because `abi3-py310` requires Python 3.10
or later. This violated the user's no-local-Python instruction. No Python
corpus, test or recorder ran, and no task Python process remained. Subsequent
Cargo commands and all successful pre-push gates explicitly set
`PYO3_NO_PYTHON=1`. The [incident report](pr-u1-python-discovery-incident.txt)
and [failure diagnostic](pr-u1-python-discovery.log) retain the evidence.

The PR descriptions originally called the ignored library test a drand
network test. The retained logs identify it as
`pow::tests::gpu_mines_and_revalidates_chain_compatible_work`, requiring
physical OpenCL GPUs. The three submitted descriptions were corrected to
match the logs on 2026-10-14.

Patches, submitted bodies, committed-HEAD logs, PR state, workflow summaries,
Rust run/job/check-suite snapshots and bounded CLI diagnostics are retained
under `docs/upstream/`. Vercel authorization URLs are omitted from the
retained check summaries. Local Rust checks passed; the remote workflows
failed before running jobs, so no hosted validation is claimed.

Cleanup completed on 2026-10-14. The exact owned
`/Users/sam/workspace/rtao/target/execute-plan.6Ro0PC/upstream-filings`
directory was removed after verifying all three clean review HEADs and no
remaining task processes or open files. Both launchd domains and
configuration searches had no path reference. The audit inspected 219
symlinks and 122 environment/Git dependency files, finding no external
dependency or audit error; the separate Cargo Git-cache check also found no
reference. `lsof` warned that an unrelated SMB backup mount was unavailable,
but its scoped local-tree query returned no open file.

Removal covered 12,344 files, 1,711 directories and three symlinks, about
569 MB on disk. Two worker-owned temporary handoff notes were also removed
after preserving their reports here. No wallet, validator/miner state, model
asset, private dataset or recovery journal was created by this work. The
shared pin cache, build cache, tools, temporary root and other agents'
directories remain live and were preserved. The
[cleanup record](protocol-epoch-cleanup.json) retains the counts and audit
results. Retained hook/gate scripts document the completed validation and
refer to the now-removed scratch paths; they are not installed hooks.
