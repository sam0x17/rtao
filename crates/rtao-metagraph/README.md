# rtao-metagraph

Bittensor metagraph projections, borrowed views and synchronization.

`Metagraph` owns a decoded graph and its runtime response. `MetagraphView`
provides a borrowed view when the runtime shape permits it. `MetagraphOptions`
selects decoder preferences, and `SubnetsMetagraphExt` adds metagraph reads to
the subnet namespace. Balances retain native checked u64 rao semantics.

`MetagraphSync::spawn` fetches an initial finalized snapshot before returning
a `MetagraphHandle`. Successful refreshes publish complete graph replacements
and `MetagraphDiff` events. The handle exposes the last-good graph and retained
health; a lagging diff consumer should reload that graph. `shutdown` cancels
the refresh worker and waits for its owned reads to stop.

`SyncPolicy` supplies full, validator and miner presets, with cadence and
failure controls. Callers own the connected client and cancellation policy.
Synchronization starts only through the explicit spawn operation.

## Features

The crate has no default features. `persist` enables atomic graph save/load
through the `persist` module. Files retain the original runtime response and
chain provenance. Loading needs matching metadata in the supplied runtime
cache; separately fetched commitment maps are not stored. Synthetic graphs
created from dynamic values cannot be saved as fetched snapshots.
