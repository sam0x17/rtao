# Local resource limits

Development now runs on `workstation` (32 cores, 61 GB RAM, about 48 GB free
disk). Local builds and focused tests are allowed here; free GitHub
`ubuntu-latest` CI remains the authoritative full validation. PLAN.md Q30's
laptop restrictions no longer apply on this host.

- Use the single workspace `target/` directory; never create per-agent target
  directories or copy build caches. Run one Cargo build command at a time and
  watch free disk space.
- No sudo or Docker on this host: localnet, Docker-service and container
  checks run in hosted CI. Python interop environments live under `.local/`
  (for example via `uv`), never in the system Python.
- Keep temporary work under one recorded task root in `.local/`. Retain
  bounded results and unfinished source, then remove disposable output.
- This host's host CPU has unstable cores: Linux CPUs 4, 5, 12 and 13
  produce random segfaults under load (including in gcc). Run every local
  build and test under
  `taskset -c 0-3,6-11,14-31` until the hardware is repaired.
- Run local tests with `umask 022` and `RTAO_TEMP_ROOT` set to a directory
  under `.local/`; this host's default umask is `002`, which the signer's
  state-permission checks correctly refuse.
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
