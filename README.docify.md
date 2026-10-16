# rtao

rtao is a Rust SDK, operator framework and CLI for Bittensor subnets. Chain
encoding comes from the pinned `bittensor-core`; behavior follows the v11
Python SDK. rtao owns asynchronous I/O and checks parity with upstream
fixtures and CI oracles.

Development follows [PLAN.md](PLAN.md). Implementation and validation are
in progress; [execution state](docs/execution/state.md) records coverage and
unfinished work. Balances use native u64 rao with checked arithmetic;
negative and arbitrary-size Python balance arithmetic is excluded. See the
[architecture](docs/plan/architecture.md) for the API contracts.

## Development checks

Rust 1.89 is the library minimum; `rust-toolchain.toml` pins the development
toolchain. The root is a Cargo workspace, with the public facade in
`crates/rtao`.

Substantial builds and tests run in the hosted `pr-gates` workflow on free
`ubuntu-latest` runners. Local work uses source inspection, editing and
formatting; full local gates, duplicate build caches, nodes, containers and
release rehearsals are outside the current scope (PLAN.md Q30).
See [CONTRIBUTING.md](CONTRIBUTING.md) for the resource policy and CI checks.

## Downstream BLS patch

Cargo applies patches from the final workspace root. A consumer must include
this line in its root `Cargo.toml`, using the same Git reference kind and
value as its `bittensor-core` pin:

```toml
[patch.crates-io]
w3f-bls = { git = "https://github.com/RaoFoundation/subtensor.git", tag = "v478" }
```

The upstream core uses a vendored BLS implementation while `tle` requests
its registry counterpart. The patch joins those sources so timelock code
builds consistently. `rtao doctor --manifest-path Cargo.toml` checks the
owning workspace and prints the exact patch line when it is missing or
mismatched. [upstream.toml](upstream.toml) records the core and behavior pins;
[the patch contract](PLAN.md#56-the-w3f-bls-patch-line) explains updates.

Cargo also records an unused optional BLS package from upstream `sp-core`.
The approved Q26 check requires one built implementation pinned to the core,
with that exact dormant lock entry guarded against activation. The evidence
is recorded in [bls-lock.md](docs/execution/bls-lock.md).

## Contributing and license

See [CONTRIBUTING.md](CONTRIBUTING.md) for verification, reconciliation and
attribution rules. Original code is licensed under
[MIT](LICENSE-MIT) or [Apache-2.0](LICENSE-APACHE). Derived upstream material
retains its Apache-2.0 obligations and is listed in [NOTICE](NOTICE).
