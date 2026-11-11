# rtao-cli

[![crates.io](https://img.shields.io/crates/v/rtao-cli.svg)](https://crates.io/crates/rtao-cli)
[![docs.rs](https://img.shields.io/docsrs/rtao-cli)](https://docs.rs/rtao-cli)
[![license](https://img.shields.io/crates/l/rtao-cli.svg)](https://github.com/sam0x17/rtao/blob/main/NOTICE)

The `rtao` command-line application and its reusable command machinery.
`application` builds the command tree; `context` resolves connection, wallet
and submission settings. `commands` combines generated catalog operations with
handwritten wallet, subnet, signing and diagnostic workflows.

Install the `rtao` binary from crates.io:

```sh
cargo install --locked rtao-cli
```

To try an unreleased commit, install from Git instead:
`cargo install --locked --git https://github.com/sam0x17/rtao rtao-cli`.

Use a command's `--help` for its current arguments. JSON output preserves
structured results; human renderers and prompts share the command's reviewed
context. Public account discovery and planning precede private signer
acquisition. Confirmation and recovery journals remain relevant to submitted
operations, including unresolved outcomes.

The `ledger`, `vault`, `extension`, `remote-signer`, `evm` and `pow-gpu`
features enable their respective optional backends and command behavior. Core client, wallet, owner
and intent support comes from this crate's existing dependencies. These flags
do not start a node, device session or service merely by enabling a build.

The binary has rustdoc disabled to avoid sharing the facade library's output
directory. This library documents the injectable prompt, process, output,
book and account-resolution interfaces used by the actual handlers.
