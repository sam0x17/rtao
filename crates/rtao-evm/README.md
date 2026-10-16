# rtao-evm

Ethereum wallet, transaction and precompile helpers for Bittensor.

`EvmRpc` provides bounded HTTP JSON-RPC calls, transaction submission and
receipt waits. `RpcOptions` controls request and response limits.
`prepare_transaction` produces an `EvmTxPreview`; signing and submission are
explicit operations. `ContractArtifact` loads user contract ABI/bytecode,
while the pinned precompile catalog supplies known addresses and ABI helpers.

EVM keys use a separate signing domain from native coldkeys and hotkeys.
`create_evm_key`, `import_evm_key` and `unlock_evm_key` use encrypted Ethereum
keystore V3 files under a wallet's `evmkeys` directory. Password resolution is
supplied through `PasswordResolver`. Native account/address mappings and
hotkey association proofs are separate helpers.

Ethereum RPC amounts use `U256` wei. `balance_to_wei` and `wei_to_balance`
convert through the SDK's checked native u64 rao model. Wei conversion
truncates sub-rao dust and checks the range; native-to-wei conversion refuses
subnet alpha balances.

This crate has no optional Cargo features. Network requests and keyfile
operations occur through explicit APIs; importing the crate starts no service.
