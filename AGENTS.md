# Local resource limits

Development now runs on `workstation` (32 threads, 61 GB RAM, about 3.4 TB
free disk). Make full use of this host: full workspace builds, full test
runs and parallel jobs are allowed. Free GitHub `ubuntu-latest` CI remains
the authoritative full validation. PLAN.md Q30's laptop restrictions no
longer apply on this host.

- The checked-in `.cargo/config.toml` caps Cargo at `jobs = 2` for shared
  machines. Override it here with `CARGO_BUILD_JOBS=32`.
- Use the single workspace `target/` directory; never create per-agent target
  directories or copy build caches. Run one Cargo build command at a time.
- No sudo or Docker on this host: localnet, Docker-service and container
  checks run in hosted CI. Python interop environments live under `.local/`
  (for example via `uv`), never in the system Python.
- Keep temporary work under one recorded task root in `.local/`. Retain
  bounded results and unfinished source, then remove disposable output.
- The host CPU's earlier random segfaults and machine-check errors stopped
  after the 2026-10-18 update to BIOS 1.O0 (microcode 0x137). Unpinned
  stress tests, two from-scratch polkadot-sdk release builds and three full
  test runs then passed with no kernel faults, so CPU pinning is no longer
  needed. If segfaults reappear, check
  `journalctl -k -b | grep -iE 'segfault|hardware error|machine check'` and
  report the affected CPUs.
- Run local tests with `umask 022`, `RTAO_TEMP_ROOT` set to a directory under
  `.local/`, and `RTAO_TEST_TMPDIR` set to a directory beneath it. This host's
  default umask is `002`, which the signer's state-permission checks correctly
  refuse. The e2e localnet tests and the hosted-only xtask fixture test fail
  locally by design.
- Do not run long soaks, fuzz campaigns or benchmarks unless a plan step
  requires them.
- Preserve wallets, recovery journals, datasets and unrelated work.

# Mac Studio workspace hygiene

- Clean up task-created ephemeral Mac Studio directories when finished.
  Cleanup is part of completing the task.
- Keep temporary tests, builds, extraction and rehearsals under one recorded
  task-scoped temporary root. Do not scatter them across `~`.
- Before removal, check running processes, launchd units, configuration paths,
  symlinks and virtual-environment dependencies. A live dependency is not
  disposable merely because its name contains `test` or `rehearsal`.
- Use exact validated paths, never broad home-directory deletion globs.
  Report any retained live dependencies.

# GitHub restrictions

- Never add or re-add the `CI` label, including toggling it or using automation
  to trigger or retry CI. Investigate or report stuck CI without that label.
- Never request or re-request review from
  `anza-xyz/community-pr-subscribers`, including toggling the review request.

# Progress communication

- Use ordinary commentary for progress updates. Never use user-input tools
  for filler, status notices or messages saying no response is needed.
- Ask a question only when its answer would materially affect the work.
