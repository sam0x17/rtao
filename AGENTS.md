# Local development

Free GitHub `ubuntu-latest` CI is the authoritative full validation. Local
builds and tests are for development feedback.

- The checked-in `.cargo/config.toml` caps Cargo at `jobs = 2`. On a machine
  with more capacity, override it with `CARGO_BUILD_JOBS`.
- Use the single workspace `target/` directory; never create per-agent target
  directories or copy build caches. Run one Cargo build command at a time.
- Localnet, Docker-service and container checks need Docker. Run them
  locally when Docker is available; hosted CI remains authoritative. Python
  interop environments live under `.local/` (for example via `uv`), never in
  the system Python.
- Run local tests with `umask 022`, `RTAO_TEMP_ROOT` set to a directory under
  `.local/`, and `RTAO_TEST_TMPDIR` set to a directory beneath it. The
  signer's state-permission checks correctly refuse a group-writable umask
  such as `002`. The e2e localnet tests and the hosted-only xtask fixture
  test fail locally by design.
- Do not run long soaks, fuzz campaigns or benchmarks unless a plan step
  requires them.
- Preserve wallets, recovery journals, datasets and unrelated work.

# Runtime adoption policy

Bittensor upgrades on the chain's schedule. Adopt newly activated Finney
runtimes promptly: make a quick diff check focused on the interfaces and
behavior this SDK uses, update pins/generated artifacts and affected adapters,
then run focused local build and smoke checks. When these pass, proceed within
existing rollout authorization. A routine upgrade does not need a broad audit,
long soak or a wait for hosted CI. These focused checks govern runtime adoption;
the general full-validation guidance above remains for other development.

Expand the review only for a concrete relevant incompatibility or failed check,
record the blocker and fix it promptly. A specVersion bump alone must not
invalidate application work. Preserve runtime/metadata compatibility checks,
current signing parameters and finalized-evidence reconciliation of unresolved
transactions; prompt adoption never permits blind resubmission.

The hourly `runtime-adopt` workflow (`ci/runtime-adopt.sh`) performs routine
adoption on its own: it confirms the runtime from independent public endpoints
(`:code` and metadata digests, the release wasm when published), pushes a green
adoption to `main` and tags a snapshot. Changed hand-ported sources do not hold
the runtime back: they are listed in `parity/port_due.toml` and one
informational issue; port them and `cargo xtask parity bless` them (strict
parity names them until then). A red or blocked run leaves `main` unchanged and
opens one issue naming the failing stage, and the same failure is retried with
backoff (1 h doubling, at most 24 h); fix that stage as above, and the fix on
`main` retries at once and closes the issue.

# Workspace hygiene

- Keep temporary tests, builds, extraction and rehearsals under one recorded
  task-scoped root in `.local/`. Retain bounded results and unfinished
  source, then remove disposable output. Cleanup is part of completing the
  task.
- Before removal, check running processes, service units, configuration
  paths, symlinks and virtual-environment dependencies. A live dependency is
  not disposable merely because its name contains `test` or `rehearsal`.
- Use exact validated paths, never broad home-directory deletion globs.
  Report any retained live dependencies.
