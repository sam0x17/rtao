# rtao

rtao is a Rust SDK, operator framework and CLI for Bittensor subnets. Chain
encoding comes from the pinned `bittensor-core`; behavior follows the v11
Python SDK. rtao owns asynchronous I/O and checks parity with upstream
fixtures and CI oracles. Balances use native `u64` rao with checked
arithmetic; negative and arbitrary-size Python balance arithmetic is excluded.

## Status

rtao is under active development and has not been published to crates.io.
There are no release tags or binaries yet. Depend on it from Git, pinned to a
reviewed commit, and check the hosted CI results for that commit.
[COMPAT.md](COMPAT.md) records the core pin, metadata inputs and any validated
runtime range.

## Getting started

Rust 1.89 is the minimum supported version. Start with the read features:

```toml
[dependencies]
rtao = { git = "https://github.com/sam0x17/rtao", rev = "<full commit>", default-features = false, features = ["client", "reads"] }

[patch.crates-io]
w3f-bls = { git = "https://github.com/RaoFoundation/subtensor.git", tag = "v478" }
```

Replace `<full commit>` with the reviewed commit you use. Cargo applies
`[patch.crates-io]` only from the final workspace root, so the patch belongs in
your application's root `Cargo.toml`. Its Git reference kind and value must
match the core pin in that revision's [upstream.toml](upstream.toml); the
example matches the current `v478` pin. The upstream core uses a vendored BLS
implementation while `tle` requests its registry counterpart, and the patch
joins those sources so timelock code builds consistently. The CLI checks the
patch without connecting to a node and prints the exact line when it is
missing or mismatched:

```sh
rtao doctor --manifest-path Cargo.toml
```

## Features

The `rtao` facade crate re-exports the workspace through Cargo features. The
defaults are `client`, `reads` and `tx`.

| Area | Features |
|---|---|
| Chain access | `client`, `reads`, `tx`, `metagraph`, `blocking`, `subxt` |
| Intents and execution | `intents`, `intents-all`, `pow`, `pow-gpu` |
| Wallets and signers | `wallet`, `wallet-encrypted`, `keychain`, `ledger`, `vault`, `extension`, `evm` |
| Neurons | `neuron`, `validator`, `miner`, `owner`, `btauth`, `btauth-server`, `btauth-client`, `redis`, `prometheus` |
| Compatibility | `compat-synapse`, `compat-epistula`, `transport-quic`, `remote-signer`, `remote-signer-server` |

`full` enables everything. The `rtao-cli` crate provides the `rtao` command
line tool, and `templates/subnet` is a `cargo generate` template for a
validator, miner and owner tool.

## Documentation

The [book](book/src/introduction.md) covers getting started, the SDK, the CLI,
validators, miners, owner operations, signers and migration from the Python
SDK. Several of its reference pages are generated; see
[book/README.md](book/README.md) to build the complete book with
`cargo xtask docs`. Hosted CI also retains the rendered book and Rust API docs
as a downloadable artifact. The [architecture](docs/plan/architecture.md)
describes the API contracts.

## Contributing and security

See [CONTRIBUTING.md](CONTRIBUTING.md) for verification, reconciliation and
attribution rules. Report vulnerabilities privately as described in
[SECURITY.md](SECURITY.md).

## License

Original rtao code is licensed under either of [MIT](LICENSE-MIT) or
[Apache-2.0](LICENSE-APACHE), at your option. Material derived from
RaoFoundation/subtensor retains its Apache-2.0 obligations, so packages that
contain it declare `(MIT OR Apache-2.0) AND Apache-2.0`. Some crates also carry
ported MIT, PSF-2.0 or Unicode-DFS-2016 material. [NOTICE](NOTICE) lists every
upstream source and its attribution.
