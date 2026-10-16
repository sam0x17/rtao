# rtao-cli

The `rtao` command-line application and its reusable command machinery.
`application` builds the command tree; `context` resolves connection, wallet
and submission settings. `commands` combines generated catalog operations with
handwritten wallet, subnet, signing and diagnostic workflows.

Use a command's `--help` for its current arguments. JSON output preserves
structured results; human renderers and prompts share the command's reviewed
context. Public account discovery and planning precede private signer
acquisition. Confirmation and recovery journals remain relevant to submitted
operations, including unresolved outcomes.

The `ledger`, `vault`, `extension`, `evm` and `pow-gpu` features enable their
respective optional backends and command behavior. Core client, wallet, owner
and intent support comes from this crate's existing dependencies. These flags
do not start a node, device session or service merely by enabling a build.

The binary has rustdoc disabled to avoid sharing the facade library's output
directory. This library documents the injectable prompt, process, output,
book and account-resolution interfaces used by the actual handlers.
Substantial checks run in hosted CI with fixtures and fake transports; local
nodes, containers and hardware rehearsals are unnecessary for client development.
