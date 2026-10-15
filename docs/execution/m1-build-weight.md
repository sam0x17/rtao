# M1 build weight measurement

Measured on 2026-10-14, `aarch64-apple-darwin`, Apple M5 Max, Rust 1.99.0,
subtensor v478. The current working tree contains the M1 implementation. Counts
describe distinct package identities reachable through normal and build
dependency edges for one selected role profile. They exclude development
dependencies and include each package version/source once; they are neither
the workspace lockfile length nor elapsed build time.

| Profile | Reachable package identities | `bittensor-core/host` |
|---|---:|---|
| `rtao-core`, no default features | 225 | disabled |
| `rtao`, `validator`, no default features | 318 | disabled |
| `rtao`, `miner`, no default features | 318 | disabled |
| `rtao`, `owner`, no default features | 394 | enabled |

Commands:

```sh
cargo tree --locked -p rtao-core --no-default-features --edges normal,build --prefix none --format '{p}'
cargo tree --locked -p rtao --no-default-features --features validator --edges normal,build --prefix none --format '{p}'
cargo tree --locked -p rtao --no-default-features --features miner --edges normal,build --prefix none --format '{p}'
cargo tree --locked -p rtao --no-default-features --features owner --edges normal,build --prefix none --format '{p}'
cargo tree --locked -p rtao --no-default-features --features validator --edges features --invert bittensor-core
cargo tree --locked -p rtao --no-default-features --features owner --edges features --invert bittensor-core
```

Remove Cargo's ` (*)` repeated-node suffix, trim each line and count unique
package identities. Feature-tree inspection confirms that the validator tree
does not activate `host`, while the owner tree activates it through the
encrypted wallet. Counts are specific to this target and current implementation;
future profile additions and Linux conditional dependencies require a new
measurement. No clean-build or cold-fetch time was measured in this check.
