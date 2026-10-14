# Plan execution state

The active goal is to execute PLAN.md through M9; M10 remains optional.
PLAN.md checkboxes and progress log record completed steps and decisions.

## Temporary workspace

This execution's disposable root is
`~/workspace/rtao/target/execute-plan.6Ro0PC`.
All task-created clones, build outputs, tests, extraction and rehearsals use
this root. Cargo builds use its `build/` subdirectory and temporary tools use
its `tmp/` subdirectory. The PR-U17 worker uses `pr-u17/` within this root.

Before removing the root, check processes, launchd units, configuration,
symlinks and virtual environments for live dependencies. Retain bounded
validation evidence in the repository and remove disposable directories when
finished. Never delete unrelated work, wallets, datasets, models or node state.

## Pending external prerequisites

M0-01 requested PLAN.md §12.13's testnet setup and soak host, GHCR namespace,
runner availability, Pages tag rule and manual Ledger device operations.
Work that does not depend on the response continues under execution rule 6.
PR-U17 is being prepared against current upstream main, using the recorded
prototype and its prior emulator evidence.
