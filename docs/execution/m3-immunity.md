# M3 immunity-period read

The validation and pending-read counts below describe the original preparation
checkpoint. Under Q30, substantial validation belongs on free `ubuntu-latest`
CI; no former task build or regeneration command is resumed locally.

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

Those historical Cargo invocations used `target/execute-plan.6Ro0PC`, with
its `build` target and `tmp` directory. This read work created no temporary
directories, wallets or validator state. Q30 cleanup retired that build tree
and its raw logs; it is no longer a live dependency and must not be recreated.
No Python ran locally. Fresh differential execution remains optional targeted
hosted evidence and is not established by these native results.
