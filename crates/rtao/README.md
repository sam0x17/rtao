# rtao

[![crates.io](https://img.shields.io/crates/v/rtao.svg)](https://crates.io/crates/rtao)
[![docs.rs](https://img.shields.io/docsrs/rtao)](https://docs.rs/rtao)
[![license](https://img.shields.io/crates/l/rtao.svg)](https://github.com/sam0x17/rtao/blob/main/NOTICE)

The public Rust SDK facade for Bittensor. Use `prelude` for common client,
namespace, identifier and transaction traits. `core` and `primitives` expose
the curated chain types even when default features are disabled.

The default features are `client`, `reads` and `tx`. `Client` connects to an
existing chain endpoint; `Namespaces` adds typed reads to clients and pinned
snapshots. `Executor` shares that client's nonce manager and policy checks.
With `intents`, it also installs the enabled catalog's JSON tool dispatcher.
Catalog availability does not establish that every operation is implemented.

| Feature | Public capability |
| --- | --- |
| `intents`, `intents-all` | Transaction intent interfaces; `intents-all` enables the operation families. |
| `metagraph` | Metagraph projections and synchronization. |
| `wallet`, `wallet-encrypted` | Wallet files; the latter enables the upstream encrypted-keyfile implementation. |
| `btauth`, `btauth-client`, `btauth-server` | btauth/1 protocol primitives, signed HTTP requests and authenticated axum handlers. |
| `neuron` | The `neuron` runtime framework reexport without a role; the role features enable it. |
| `validator`, `miner`, `owner` | Operator runtimes and their required intent families. |
| `blocking` | Blocking client and namespace adapters backed by a private runtime thread. |
| `evm` | EVM accounts, signing and precompile helpers. |
| `ledger`, `vault`, `extension`, `keychain` | Optional signing and password backends. |
| `pow`, `pow-gpu`, `prometheus`, `subxt`, `redis` | PoW helpers, telemetry, Subxt interoperability and shared replay storage. |
| `compat-synapse`, `compat-epistula` | `compat::synapse` (Synapse, Axon and Dendrite wire compatibility) and `compat::epistula` (Epistula v2 signed headers). |
| `transport-quic` | `quic`, a btlightning-compatible QUIC client and server transport. |
| `remote-signer`, `remote-signer-server` | `remote_signer` client; the latter adds the policy-checked signing server with an online runtime source. |

Choose the named operator feature when using its API. The `full` bundle enables
the broad SDK and underlying role implementations; the named facade features
still control some root reexports. `pow-gpu` and `subxt` are separate switches.

Balances use checked native `u64` rao. Keep TAO and subnet alpha units distinct.
Before signing, inspect plans and set fee, spend and subnet limits appropriate
to the application. A successful client check or plan does not prove a live
runtime effect.

rtao depends on the core through its republished `rtao-upstream-*` packages,
so consumers need no `[patch]`. The
[repository README](https://github.com/sam0x17/rtao#the-republished-core)
describes them, and `rtao doctor` checks a workspace's lock against them.
