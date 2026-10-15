# EVM accounts and commands

Enable the `evm` feature for Ethereum keys, precompiles, HTTP JSON-RPC and
the CLI's `evm` group. EVM keys are separate from native wallet keys. Their
encrypted V3 files live in the selected wallet's `evmkeys` directory. Key
creation, import and unlock are synchronous filesystem/KDF operations; async
applications should run them on a blocking worker. Secret inputs and password
interaction remain the application's responsibility.

`evm key new`, `import`, `export`, `list` and `show` manage these files.
Imports support private keys, V3 keystores and BIP-39 phrases. Exports contain
the encrypted keystore. `mirror`, `pubkey` and `deposit-address` inspect public
address mappings without unlocking. The public mirror uses
`blake2_256("evm:" || h160)`; it differs from truncating a native public key to
twenty bytes. Shared helpers live in `rtao::primitives::evm`.

Native balances use checked `u64` rao. EVM wire values use eighteen-decimal
wei and retain their full Solidity integer width where appropriate. Converting
to a native balance truncates sub-rao dust and refuses overflow. Precompile
calls use `Precompile::encode_call` to enforce native amount limits; generic
contract ABI encoding can represent unrelated full-width Solidity values.

`fund`, `claim-deposit` and `associate` use native transaction planning and
submission. The other transaction path uses a stored EVM key and the selected
HTTP endpoint. `send`, `send-to-ss58`, writable `call` and `deploy` show a
public preview before unlocking. `--dry-run` does not authorize a signature.
An association obtains proof consent before key acquisition, then applies the
ordinary review to the complete native transaction.

`precompiles` lists the pinned catalog; `abi` preserves its original JSON
entries. `call` handles view functions without a private key. Contract
deployment accepts bounded ABI/bytecode artifacts. `networks`, `config` and
`doctor` expose endpoint and public network configuration.
`setup-localnet` configures an already available local endpoint through native
calls; it does not deploy or start a node. Its dry-run starts no native client.

`EvmRpc` sends a transaction once. Observation or transport errors after
transmission retain `unknown_fate_hash()`; use that hash to reconcile the
receipt before retrying. A timeout does not prove rejection. Hosted fixture
results must be checked for the chosen revision; live-chain and Python
interoperability coverage remain separately recorded.
