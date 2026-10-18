# rtao-tx

Transaction planning, semantic policy checks and submission for Bittensor.
`Executor` uses an existing verified client, its runtime metadata and nonce
manager. Intent implementations supply typed roles, preflight observations and
encoded calls through `Intent` and `IntentInterface`.

Inspect a `Plan` and set `Policy` limits for fees, spend, subnets and supported
operation safety before signing. `ExecuteOptions` controls routing, while
`ExtrinsicResult` retains inclusion, dispatch and operation-specific outputs.
Unresolved submission fate requires reconciliation before repeating a call.

`offline` prepares bounded unsigned transactions for external signatures.
`multisig` handles approvals and opening-deposit funding checks. `timelock`
provides Quicknet envelopes and asynchronous beacon decryption. The operation
catalog and concrete intent families are supplied by `rtao-intents` or the
facade's executor; this crate's bare executor has no generated parser installed.

This crate declares no feature switches. Signer backends and intent families
are enabled on their owning crates. Chain balances use checked native `u64`
rao. An unsigned plan or approval receipt does not establish a runtime
mutation.
