# Introduction

rtao is a Rust SDK for Bittensor. It tracks separate core and Python parity pins,
keeps chain balances in native unsigned rao, and checks metadata before using
the typed surface. The project is under development; the feature list and
`COMPAT.md` distinguish implemented features from future work.

Read the included `COMPAT.md` before selecting a runtime. A missing validated
range means validation is incomplete. The [Rust API docs](api/rtao/index.html)
describe the available crates and feature gates.

Start with [Getting started](getting-started.md), then use the
[SDK overview](sdk.md) for connections, reads, signing and transaction results.
The [agent guide](agents.md) covers tool discovery and approval, and
[migration](migration.md) maps existing Python and core-client applications.
Read [parity evidence](parity.md) alongside the source index when assessing
coverage for an operation.
