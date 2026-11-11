# rtao

[![crates.io](https://img.shields.io/crates/v/rtao.svg?logo=rust)](https://crates.io/crates/rtao)
[![docs.rs](https://img.shields.io/docsrs/rtao?logo=docs.rs)](https://docs.rs/rtao)
[![rtao-cli](https://img.shields.io/crates/v/rtao-cli.svg?label=rtao-cli)](https://crates.io/crates/rtao-cli)
[![downloads](https://img.shields.io/crates/d/rtao.svg)](https://crates.io/crates/rtao)
[![MSRV](https://img.shields.io/crates/msrv/rtao.svg?label=MSRV)](https://github.com/sam0x17/rtao/blob/main/Cargo.toml)
[![license](https://img.shields.io/crates/l/rtao.svg)](#license)

rtao is a Rust SDK, operator framework and CLI for Bittensor subnets. Chain
encoding comes from the pinned `bittensor-core`; behavior follows the v11
Python SDK. rtao owns asynchronous I/O and checks parity with upstream
fixtures and CI oracles. Balances use native `u64` rao with checked
arithmetic; negative and arbitrary-size Python balance arithmetic is excluded.

## Status

rtao is under active development. Before 1.0, a minor release (0.x.0) may
break public APIs or raise a persisted format's `readable_by`; a patch release
does neither ([docs/releases/README.md](docs/releases/README.md)). Each release
is an `rtao-vX.Y.Z` tag with notes on the
[releases page](https://github.com/sam0x17/rtao/releases), and
[CHANGELOG.md](CHANGELOG.md) lists every change.
[COMPAT.md](COMPAT.md) records the core pin, metadata inputs and any validated
runtime range.

## Getting started

Rust 1.89 is the minimum supported version. Start with the read features:

```toml
[dependencies]
rtao = { version = "0.1", default-features = false, features = ["client", "reads"] }
```

or run `cargo add rtao --no-default-features --features client,reads`.

### From Git

To use a commit that is not released yet, depend on it from Git, pinned to a
reviewed commit, and check the hosted CI results for that commit:

```toml
[dependencies]
rtao = { git = "https://github.com/sam0x17/rtao", rev = "<full commit>", default-features = false, features = ["client", "reads"] }

[patch.crates-io]
w3f-bls = { git = "https://github.com/RaoFoundation/subtensor.git", tag = "v490" }
```

Replace `<full commit>` with the reviewed commit you use. Cargo applies
`[patch.crates-io]` only from the final workspace root, so the patch belongs in
your application's root `Cargo.toml`. Its Git reference kind and value must
match the core pin in that revision's [upstream.toml](upstream.toml); the
example matches the current `v490` pin. The upstream core uses a vendored BLS
implementation while `tle` requests its registry counterpart, and the patch
joins those sources so timelock code builds consistently. The CLI checks the
patch without connecting to a node and prints the exact line when it is
missing or mismatched:

```sh
rtao doctor --manifest-path Cargo.toml
```

### Downstream BLS patch

That `w3f-bls` line is the one `[patch]` a Git dependency needs to build.
rtao's own `Cargo.toml` carries more `[patch]` tables. Cargo never applies a
dependency's `[patch]`, so an application decides on each of them (D-115):

| rtao workspace patch | Why rtao carries it | Downstream |
|---|---|---|
| `[patch.crates-io] w3f-bls` at the core tag | joins the core's vendored BLS with the registry crate `tle` requests | required, as above |
| `[patch."https://github.com/RaoFoundation/subtensor.git"] bittensor-core` to a fork revision | two reviewed changes on the pinned core tag ([`docs/execution/core-patch.md`](docs/execution/core-patch.md)): the Ed25519 erasure repair, which keeps signing keys in a wiping owner ([RaoFoundation/subtensor#3251](https://github.com/RaoFoundation/subtensor/issues/3251), open upstream) and, since the v488 core, also hashed-account Ed25519 keys ([RaoFoundation/subtensor#3317](https://github.com/RaoFoundation/subtensor/issues/3317), D-139), and the injectable Ledger APDU transport rtao's emulator tests use ([RaoFoundation/subtensor#3242](https://github.com/RaoFoundation/subtensor/pull/3242)) | recommended. Without it the core builds from the pinned tag, but Ed25519 secret material is not wiped from memory. Copy it with the next row, at the revision in the same rtao revision's `Cargo.toml`; the subnet template carries both (D-146) |
| `[patch."https://github.com/sam0x17/subtensor.git"] fips204, subtensor-hashed, subtensor-macros, w3f-bls` | keeps the fork's own monorepo dependencies on the pinned tag | only with the previous row; copy both or neither, at the revisions in that rtao revision's `Cargo.toml` |
| `[patch.crates-io] ruint = { path = "patches/ruint" }` | `ruint` 1.20.1, the release with the RUSTSEC-2026-0220 fix, with only its `rust-version` lowered to rtao's 1.89 minimum and one const loop restored (`patches/ruint/UPSTREAM.json`) | reached only through the `evm` feature (alloy). On Rust 1.90 or newer, lock crates.io `ruint` 1.20.1 or newer (`cargo update -p ruint`); on 1.89, copy `patches/ruint` from the same rtao revision and patch it by path |

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
line tool (`cargo install --locked rtao-cli`), and `templates/subnet` is a
`cargo generate` template for a validator, miner and owner tool.

## Documentation

The [API documentation](https://docs.rs/rtao) is on docs.rs, one page per
crate. The [book](book/src/introduction.md) covers getting started, the SDK, the CLI,
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
