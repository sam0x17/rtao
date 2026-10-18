# Local disk cleanup, 2026-10-14

The repeated isolated Rust builds consumed excessive local disk space. That
execution was stopped and client development moved to free GitHub CI for
substantial tests. PLAN.md Q30 records that decision.

## Removed

The entire previous local execution root was removed. A single size
scan measured 335,709,460 KiB (320.16 GiB) before removal. Its shared Cargo
cache accounted for 275,206,152 KiB (262.46 GiB); separate CLI, Keychain,
lint/MSRV, proxy, offline, fuzz and staged-probe targets accounted for most
of the rest. Repeated feature/toolchain builds and copied caches caused the
growth. No full node or local testnet had run.

Removal used exact validated paths and one low-priority deletion process at
a time. Checks found no running Cargo/rustc/nextest/node jobs, open task
files, launchd services, external configuration/symlink references or virtual
environments requiring the removed root. Git alternates pointed from
disposable snapshots to the canonical repository; the canonical repository
had no reverse dependency. Review worktree registrations stayed inside the
deleted task root.

## Preserved

Canonical source, including uncommitted and untracked implementation, stays
in place. Unfinished candidates and 172 patches are retained in a private
archive that is not part of the repository. It is 152,583,783 bytes
(145.5 MiB) and contains 18,888 verified members. Source candidates remain
subject to review and CI before adoption.

Archive SHA-256:
`770d9d50a9c855e61a4e7476c91caef52b6d0161c297c996a9fdf46c0852969b`.

The pre-existing `target/debug`, global caches/toolchains, other projects,
wallets and recovery data were preserved. The complete remaining target
directory is 547,192 KiB (534 MiB). The durable wallet/recovery directory was
not inspected for secrets or removed.

## Prevention and verification

AGENTS.md and CONTRIBUTING.md direct substantial checks to hosted CI and
prohibit the previous local build/infrastructure campaign. Cargo defaults
limit jobs to two, disable incremental compilation and omit dev/test debug
information. The normal hosted checks remain active, with cancellation of
superseded runs and 45-minute job timeouts. Eleven infrastructure, scheduled
and release workflow drafts were moved outside the active workflows directory.

The plan audit covered execution rules, test strategy, every M0-M9 step and
exit list, prerequisites, platform/hardware requirements, release/upkeep
handover, repeated summaries and the architecture's workflow appendix.
Active acceptance now uses hosted client checks and fixtures; costly
infrastructure is optional hosted reference or deferred. Historical evidence
and technical API requirements remain identified separately from active work.

Cleanup ran no Rust builds, tests, nodes, containers or hosted jobs. The
source archive was verified before deleting scratch. Documentation,
configuration and workflow edits receive source-only validation; current
product test success still requires hosted CI.
