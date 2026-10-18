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
