# Chain concepts

These upstream raw Markdown pages explain chain concepts. Follow the Rust
guides alongside them for rtao's API choices. The upstream website can advance
independently of this checkout; [compatibility](compat.md) records the source
and metadata pins used here. Live state and runtime rules determine current
network values.

| Topic | Upstream raw Markdown | Rust guidance |
|---|---|---|
| Network, subnets and identities | [Network](https://www.bittensor.com/llms.mdx/docs/concepts/network/content.md) | [SDK](sdk.md), [validator](validator.md), [miner](miner.md) |
| Clients and pinned reads | [Client](https://www.bittensor.com/llms.mdx/docs/concepts/client/content.md) | [SDK connections and reads](sdk.md#connections-and-consistent-reads) |
| TAO, alpha and rao | [Money](https://www.bittensor.com/llms.mdx/docs/concepts/money/content.md) | [Units and intent arguments](sdk.md#units-and-intent-arguments) |
| Wallets, hotkeys and coldkeys | [Wallets](https://www.bittensor.com/llms.mdx/docs/concepts/wallets/content.md) | [Signers](signers.md), [Ledger](ledger.md) |
| Intents, fees and failures | [Transactions](https://www.bittensor.com/llms.mdx/docs/concepts/transactions/content.md) | [Agents](agents.md), [transaction reference](tx/index.md) |
| Epochs and emissions | [Emissions](https://www.bittensor.com/llms.mdx/docs/concepts/emissions/content.md) | [Validator cadence](validator.md), [Null consensus](null-consensus.md) |
| Proxy, multisig and shielded calls | [Advanced operations](https://www.bittensor.com/llms.mdx/docs/concepts/advanced/content.md) | [Multisig](multisig.md), [MEV shield](mev-shield.md) |
| Root basket shares | [Beta tokens](https://www.bittensor.com/llms.mdx/docs/concepts/beta-tokens/content.md) | [Query reference](query/index.md), `rtao root --help` |

The generated [reference](reference.md) links individual reads, transactions,
errors and hyperparameters to their upstream descriptions and chain sources.
The [migration guide](migration.md) and [divergence register](divergences.md)
describe deliberate differences in the Rust client.
