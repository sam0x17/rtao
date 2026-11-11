# rtao-core-ext

Portable extensions staged for the pinned Bittensor core. Most consumers use
their curated reexports through `rtao-core`.

- `ir` exposes serializable runtime metadata through `RuntimeMetadataIr`.
- `ops` builds transport-independent RPC requests and decodes storage, runtime
  API, fee, receipt and dispatch-error responses.
- `value_ext` traverses decoded SCALE values.
- `keyfile_json` provides the plaintext keyfile codec and encryption-format
  detection.
- `epoch_schedule` predicts epoch scheduling from explicit state, while
  `timelock_commit` prepares weight commitments with an explicit clock.

The pure environment password codec (`keyfile_env`) and the strict verifier
(`keys::verify_strict`) were staged here until the core shipped them in v490;
`rtao-core` now re-exports the core's own.

The crate declares no Cargo features. Encrypted keyfile operations and network
access belong to other layers. Epoch prediction assumes successful epoch
processing; consumers must follow the reveal epoch actually assigned on chain.
