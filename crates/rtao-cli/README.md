# rtao-cli

[![crates.io](https://img.shields.io/crates/v/rtao-cli.svg)](https://crates.io/crates/rtao-cli)
[![docs.rs](https://img.shields.io/docsrs/rtao-cli)](https://docs.rs/rtao-cli)
[![license](https://img.shields.io/crates/l/rtao-cli.svg)](https://github.com/sam0x17/rtao/blob/main/NOTICE)

The library behind the `rtao` command-line application: its reusable command
machinery and the process entry point that the `rtao` crate's binary calls.
`application` builds the command tree; `context` resolves connection, wallet
and submission settings. `commands` combines generated catalog operations with
handwritten wallet, subnet, signing and diagnostic workflows.

This crate has no binary. Install `rtao` from the `rtao` crate, whose default
features build it:

```sh
cargo install --locked rtao
```

To try an unreleased commit, install from Git instead:
`cargo install --locked --git https://github.com/sam0x17/rtao rtao`. Before
0.1.1 the binary came from this crate; run `cargo uninstall rtao-cli` first if
you installed it that way.

Use a command's `--help` for its current arguments. JSON output preserves
structured results; human renderers and prompts share the command's reviewed
context. Public account discovery and planning precede private signer
acquisition. Confirmation and recovery journals remain relevant to submitted
operations, including unresolved outcomes.

The `ledger`, `vault`, `extension`, `remote-signer`, `evm` and `pow-gpu`
features enable their respective optional backends and command behavior; the
`rtao` crate's features of the same names pass them on, so
`cargo install --locked rtao --features ledger,vault` builds a binary with
those backends. Core client, wallet, owner and intent support comes from this
crate's existing dependencies. These flags do not start a node, device session
or service merely by enabling a build.

This library documents the injectable prompt, process, output, book and
account-resolution interfaces used by the actual handlers.
