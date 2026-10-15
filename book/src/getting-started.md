# Getting started

rtao is under development. Use a reviewed source revision and its matching
core pin; release binaries and release tags are outside the current development
scope. The repository's [execution state](https://github.com/sam0x17/rtao/blob/main/docs/execution/state.md)
records implemented features, hosted results and remaining work.

For an application beside a checked-out `rtao` directory, start with the read
features:

```toml
[dependencies]
rtao = { path = "../rtao/crates/rtao", default-features = false, features = ["client", "reads"] }

[patch.crates-io]
w3f-bls = { git = "https://github.com/RaoFoundation/subtensor.git", tag = "v478" }
```

Adjust the path to your checkout. For a git dependency, pin `rev` to the full
reviewed commit instead. Cargo applies `[patch.crates-io]` from your application's
workspace root. Its Git reference kind and value must match the core pin in
that revision's `upstream.toml`; the example above matches the current `v478`
pin. The CLI checks this without connecting to a node:

```sh
rtao doctor --manifest-path Cargo.toml
```

## Connect and read one block

Pass an existing endpoint or a named network such as `finney` or `test` to
`ClientConfig::for_network`. You do not need to operate a node. A snapshot pins
all its reads to one block hash; separate reads through a live client can see
different heads.

This helper takes a connected client and a public coldkey address. It needs no
wallet or private key:

```rust
use rtao::prelude::*;

async fn finalized_free_rao(
    client: &Client,
    coldkey: AccountId,
) -> anyhow::Result<u64> {
    let snapshot = client.at(BlockRef::Finalized).await?;
    let balance = snapshot.balances().balance(coldkey).await?;
    Ok(balance.rao)
}
```

Create the client with
`Client::connect(ClientConfig::for_network("finney")?).await?` and call
`client.close().await` when the application is finished. Client clones share
the connection, so closing one closes that shared client. Applications using
the helper need an async runtime and their chosen error type; this example
uses `anyhow`.

Balances use checked native `u64` rao. Keep TAO and subnet alpha units distinct;
negative amounts and values above the chain's balance range are rejected.

## Choose the client surface

Add `intents-all` and `wallet-encrypted` for the full intent catalog and
encrypted wallet files, or use the `owner`, `validator` or `miner` feature for
the corresponding framework. `Executor::plan` builds and reviews an intent
using public wallet information. Execution resolves a private signer only
after policy and preflight checks. A preview still makes chain reads and fee
queries; it does not submit a transaction. The [owner guide](owner.md) shows
that flow with a fee cap and subnet scope.

The current CLI includes `wallet`, `weights`, `axon`, `hparams`, `subnets`,
`stake`, `proxy` and `multisig`, plus generated `tx` and `query` commands,
`call`, `config`, `tools`, `explain` and `doctor`. Use each command's `--help`
for the enabled schema. `--dry-run` previews a supported mutation; `--json`
selects machine-readable output. The [transaction reference](tx-reference.md)
lists catalog operations and their feature requirements. The
[API index](api-index.md) maps Python names to Rust.

## Development and coverage

Source review and formatting stay local. Substantial builds, tests, dependency
checks and documentation builds belong on free GitHub `ubuntu-latest` runners.
Ordinary client validation uses Rust tests, committed vectors and fake
transports. It does not require a local node, Docker, Python, a testnet wallet
or a validator soak. Optional live-chain and interoperability evidence is
reported separately when it exists. Documentation and source availability do
not establish that the latest hosted run passed; consult execution state and
the checks for the exact commit you use.
