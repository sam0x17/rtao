# {{project-name}}

This workspace starts a hotkey-only validator and a btauth/1 echo miner. Replace
the challenge and scoring rule in `validator/src/main.rs` and the `/echo` handler
in `miner/src/main.rs` with your subnet's protocol. `VerifiedCaller` comes from
the runtime's authentication layer; `Json<T>` parses via `serde_json::Value` first
so the core's arbitrary-precision JSON feature preserves number decoding.

Both binaries install ring as their process TLS provider before resolving flags.
They use `NeuronConfig` flags, environment and btcli configuration, and persist
application state on SIGTERM. They require an already registered hotkey. The
miner admits metagraph members with validator permits by default.

Use Rust 1.89 or newer. The pre-release SDK dependencies use `main`; Cargo.lock
records their exact commit. At the first rtao release, select the same `rtao-v*`
tag for all three SDK dependencies. Keep the single root `w3f-bls` patch at the
SDK's core pin, and commit Cargo.lock after generation. Never copy a core source
patch into this workspace.

For development on a Linux x64 Docker host:

```sh
export RTAO_TEMP_ROOT="$PWD/target/subnet-localnet"
export CARGO_TARGET_DIR="$RTAO_TEMP_ROOT/build"
mkdir -p "$RTAO_TEMP_ROOT/tmp"
export TMPDIR="$RTAO_TEMP_ROOT/tmp" PYO3_NO_PYTHON=1
cargo generate-lockfile
cargo fetch --locked
# Install rtao-cli from the SDK checkout selected by Cargo, as ci.yml does.
bash ci/check.sh
```

The check script runs formatting, strict Clippy, the native binaries and doctor,
then builds both no-host images with the resolved SDK as a BuildKit named
context. It starts the digest-pinned compose localnet. The integration test uses
`rtao-testkit::Scenario` to register and stake a validator and register a miner,
creates deterministic development hotkeys only below the task root, then runs
both actual images. It requires an echo reply to a btauth/1-signed request,
a matching `TimelockedWeightsCommitted` / `TimelockedWeightsRevealed` pair, the exact miner
weight row, and the miner's persisted request count. The script removes its
compose containers on success and failure. Retain the bounded JSON/log evidence,
then audit and remove disposable development wallets/state when finished.

Compose uses Linux host networking for neuron requests to the published axon.
Its private address is permitted only by the explicit `--network local` flag.
Production operators must supply a public axon IP and port, their own subnet,
read-only hotkey mount and writable state mount. The runtime image defaults to
UID/GID 65532; CI runs it as the non-root runner user to access 0600 fixture
hotkeys. The optional systemd unit in `deploy/` uses a 90-second SIGTERM stop
window. Create the `subnet` user, put wallets under `/var/lib/subnet/wallets`, and
set `BT_NETWORK`, `BT_WALLET_PATH`, `BT_WALLET`, `BT_WALLET_HOTKEY`,
`RTAO_STATE_DIR` and `NETUID` in `/etc/subnet/validator.env` or `miner.env`.

The owner tool joins this workspace in M4; Ledger support joins in M7.
