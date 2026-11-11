# rtao-core

[![crates.io](https://img.shields.io/crates/v/rtao-core.svg)](https://crates.io/crates/rtao-core)
[![docs.rs](https://img.shields.io/docsrs/rtao-core)](https://docs.rs/rtao-core)
[![license](https://img.shields.io/crates/l/rtao-core.svg)](https://github.com/sam0x17/rtao/blob/main/NOTICE)

The curated interface to the pinned `bittensor-core`. This crate reexports
chain codecs and cryptography and exposes the portable extensions maintained
in `rtao-core-ext`.

Use `codec` and `runtime` for dynamic SCALE values and metadata, `extrinsic`
for era and multisig helpers, and `keys` for key derivation and verification.
`digest`, `mlkem` and `timelock` expose the corresponding upstream formats.
`commit_reveal` is owned by rtao: it chooses the version tag of a timelocked
weight commit and lists the runtime code reviewed for it. It has no I/O and
allocates nothing, so it builds for `wasm32-unknown-unknown` too.
`DynValue` provides JSON conversion, and `CoreError` is the owned error type.

| Feature | Effect |
| --- | --- |
| `offload` (default) | Adds `offload`, which runs blocking work through Tokio's blocking pool. An entered Tokio runtime is required. |
| `host` | Adds encrypted `keyfiles` and proof-of-work helpers in `pow`. |
| `ledger` | Adds the upstream `ledger` device API. |

Upstream secret constructors can include recovery material in errors. The
higher-level `rtao-signer` and `rtao-wallet` APIs redact those diagnostics at
their boundaries. Direct calls through `keys::Keypair` retain upstream behavior.

The core is `rtao-upstream-core`, rtao's republish of the reviewed
`bittensor-core` revision (sources byte for byte; its `MODIFICATIONS` lists what
the core patch changes), with the BLS fork and `tle` it builds against
([workspace README](https://github.com/sam0x17/rtao#the-republished-core)).
`UPSTREAM_CORE_VERSION` and `UPSTREAM_BLS_VERSION` name the versions this
crate depends on. No `[patch]` is needed downstream.
