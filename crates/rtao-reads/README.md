# rtao-reads

Typed Bittensor queries and namespace extensions over `rtao_client::View`.
Import `Namespaces` to access account, subnet and other catalog namespaces on
a client, snapshot or another view implementation.

Use a pinned `Snapshot` when several values must describe the same block.
Generated `ReadName` and catalog metadata support discovery and JSON dispatch;
typed methods project runtime data into native values. `ReadError` distinguishes
client failures, invalid parameters, commitment decoding, changed metadata
shapes and explicit not-yet-ported reads.

`commitments` decodes current and revealed commitment history, including
timelock-aware values. The subnet commitment scan retains deregistered
hotkeys rather than limiting results to the current neuron rows.

This crate declares no feature switches. Optional transport and client adapters
are configured on their owning crates. Balance projections retain checked
native `u64` rao and distinct TAO and alpha units.
