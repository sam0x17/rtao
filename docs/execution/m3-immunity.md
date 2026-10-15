# M3 immunity-period read

The `immunity_period` read now returns the chain's native `u16` block count.
Its scalar body is generated from the reviewed
`SubtensorModule.ImmunityPeriod` descriptor mapping. The free typed wrapper and
`hyperparameters().immunity_period(NetUid)` namespace method return the same
type. Dynamic dispatch preserves the authenticated optional signature default,
`netuid = null`, and rejects a missing or null required chain key before I/O.
The one pending-read entry was removed; all other pending reads remain listed.

The pinned Python registration delegates to `scalar_read`, whose behavior is
one storage query followed by `int`. The port returns an integer JSON token
without normalization or arithmetic. It retains native identifier bounds.

Three metadata-backed FakeChain tests cover zero and maximum `u16` counts,
zero and maximum native subnet identifiers, runtime metadata fallback of 4096
blocks, typed and dynamic entry points, default filling, invalid keys without
I/O, and distinct historical snapshots. RecordingView verifies that every
sample touches only `SubtensorModule.ImmunityPeriod`, the sample union equals
the authenticated descriptor set H, and storage RPCs retain the selected hash.

Current Rust and Rust 1.89 pass all three tests. Strict Clippy passes for the
read crate and the new test target. The updated M2 pending-read regression also
passes, proving that the remaining 86 reads fail without I/O. Canonical
`cargo xtask regen --reads-only` and its `--check` pass.

Regeneration initially needed the existing xtask binary because xtask depends
on the read crate, while the new typed wrapper needs the regenerated return
type. That binary generated the read outputs from the reviewed inputs and
passed `regen --reads-only --check`; canonical regeneration then rebuilt from
the current sources. No generated file was edited by hand.

Every Cargo invocation uses `target/execute-plan.6Ro0PC` as the recorded task
root, with its `build` target and `tmp` directory. This read work created no
temporary directories, wallets or validator state. Shared builds remain live
for the parent task. No Python ran locally; hosted differential validation
remains separate.
