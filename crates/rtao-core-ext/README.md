# rtao-core-ext

Portable extensions staged for the pinned Bittensor core. Most consumers use
their curated reexports through `rtao-core`.

- `ir` exposes serializable runtime metadata through `RuntimeMetadataIr`.
- `ops` builds transport-independent RPC requests and decodes storage, runtime
  API, fee, receipt and dispatch-error responses.
- `value_ext` traverses decoded SCALE values.
- `keyfile_json` provides the plaintext keyfile codec and encryption-format
  detection; `keyfile_env` encodes and decodes environment password values.
- `epoch_schedule` predicts epoch scheduling from explicit state, while
  `timelock_commit` prepares weight commitments with an explicit clock.
- `verify_strict` verifies the supplied message bytes without the upstream
  verifier's additional wrapped-message attempt.

The crate declares no Cargo features. Encrypted keyfile operations and network
access belong to other layers. Epoch prediction assumes successful epoch
processing; consumers must follow the reveal epoch actually assigned on chain.
