# Plan execution state

The active goal is to execute PLAN.md through M9; M10 remains optional.
PLAN.md checkboxes and progress log record completed steps and decisions.

## Temporary workspace

This execution's disposable root is
`/Users/sam/workspace/rtao/target/execute-plan.6Ro0PC`.
All task-created clones, build outputs, tests, extraction and rehearsals use
this root. Cargo builds use its `build/` subdirectory and temporary tools use
its `tmp/` subdirectory. The PR-U17 worker uses `pr-u17/` within this root.

Before removing the root, check processes, launchd units, configuration,
symlinks and virtual environments for live dependencies. Retain bounded
validation evidence in the repository and remove disposable directories when
finished. Never delete unrelated work, wallets, datasets, models or node state.

## External prerequisites and the user's 2026-10-14 response

M0-01 requested PLAN.md §12.13's prerequisites. The user answered:
"I can provide testnet tao to whatever wallet address you need; just use my
github; we will skip sophisticated github runners, I want to just use
ubuntu-latest free ones; I don't see why we need github pages, that seems
overkill; I have a ledger we can use" (PLAN.md Q23).

Use `sam0x17` for GitHub and `ghcr.io/sam0x17/rtao-oracle` for the oracle
namespace. All hosted jobs use free Linux x64 `ubuntu-latest`; no paid,
large, ARM-native or macOS hosted runners are required. Docs remain complete
and ship as downloadable rustdoc/book release assets without Pages.

Prepare a testnet funding address before asking the user to fund it; no funds,
commit-reveal subnet, registered/staked hotkey or completed soak are assumed.
The long-lived 24-hour soak host remains unidentified. The user's Ledger is
available for manual device runs when the checklist is ready; actual run
results and model/app compatibility remain to verify.

Preserve full parity and all release targets. Linux builds need a fixed,
digest-pinned Ubuntu 22.04 container for the glibc 2.35 floor; ARM cross tooling
and resources must be validated and never described as native test coverage.
Mac Studio builds/checks use the recorded temporary root. Keychain parity
requires a separate isolated harness that restores the default keychain and
search list on every exit, preserving user secrets; `cargo xtask gates` changes
no host settings. Equivalent Mac checks and trustworthy provenance for local
Mac artifacts remain unresolved until verified; drafts publish only after all
targets, checksums and required provenance pass. Mainnet-clone feasibility on
free runners also needs measurement; exceeding resources leaves the host
prerequisite pending without authorizing paid or large runners.

Independent execution continues under rule 6; no step is checked merely by
this decision, and no repeated namespace/runner/Pages permission is needed.
PR-U17 is being prepared against current upstream main, using the recorded
prototype and its prior emulator evidence.
