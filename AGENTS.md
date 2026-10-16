# Local resource limits

This is a shared laptop with other active projects. Keep work to client
development and use free GitHub `ubuntu-latest` CI for substantial validation.
PLAN.md Q30 supersedes earlier requirements for costly local work.

- Do not run full nodes, local testnets, Docker services, Python oracles,
  soaks, hardware rehearsals, full workspace gates, feature matrices, MSRV
  builds, fuzz campaigns, benchmarks or release rehearsals locally or on the
  Mac Studio. Use fixtures and fake transports for client tests.
- Do not start cold builds or install validation tools locally. Prefer source
  inspection and formatting. Put substantial tests and builds in GitHub CI.
- Any small, focused local Rust check uses one target directory, at most two
  build jobs, no incremental output and no debug information. Run one Cargo
  command at a time. Never copy or APFS-clone build caches or create per-agent
  target directories. Stop growing local output and move validation to CI.
- Keep temporary work under one recorded task root. Retain bounded results
  and unfinished source, then remove disposable output. Do not recreate the
  retired `target/execute-plan.6Ro0PC` workspace or resume its queued commands.
- Preserve wallets, recovery journals, datasets, models, miner/validator state,
  unrelated projects and shared Cargo/toolchain caches.

# Mac Studio workspace hygiene

- Clean up task-created ephemeral Mac Studio directories when finished.
  Cleanup is part of completing the task.
- Keep temporary tests, builds, extraction and rehearsals under one recorded
  task-scoped temporary root. Do not scatter them across `/Users/sam`.
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
