# {{project-name}}

This workspace contains a hotkey-only validator, a btauth/1 echo miner and a
coldkey owner tool. Replace the challenge and scoring rule in
`validator/src/main.rs` and the `/echo` handler
in `miner/src/main.rs` with your subnet's protocol. `VerifiedCaller` comes from
the runtime's authentication layer; `Json<T>` parses via `serde_json::Value` first
so the core's arbitrary-precision JSON feature preserves number decoding.

Both neuron binaries install ring as their process TLS provider before resolving
flags. They use `NeuronConfig` flags, environment and btcli configuration, and persist
application state on SIGTERM. They require an already registered hotkey. The
miner admits metagraph members with validator permits by default.

Use Rust 1.89 or newer. The pre-release SDK dependencies use `main`; Cargo.lock
records their exact commit. At the first rtao release, select the same `rtao-v*`
tag for all three SDK dependencies. Keep the single root `w3f-bls` patch at the
SDK's core pin, and commit Cargo.lock after generation. Never copy a core source
patch into this workspace.

The generated workflow validates client source on free GitHub `ubuntu-latest`.
It formats and lints the workspace, builds the validator, miner and owner tool,
tests those three members, compiles the optional integration harness without
running it, checks the two neuron dependency profiles and runs the SDK doctor.
Substantial validation stays on that disposable hosted runner:

```sh
export RTAO_TEMP_ROOT="$PWD/target/subnet-ci"
export CARGO_TARGET_DIR="$RTAO_TEMP_ROOT/build"
mkdir -p "$RTAO_TEMP_ROOT/tmp"
export TMPDIR="$RTAO_TEMP_ROOT/tmp" PYO3_NO_PYTHON=1
cargo generate-lockfile
cargo fetch --locked
# Install rtao-cli from the SDK checkout selected by Cargo, as ci.yml does.
bash ci/check.sh
```

The default check starts no live service. A manual hosted workflow dispatch
with `localnet` enabled additionally builds both images and runs the optional
compose integration. This check is useful only when a client change needs
chain evidence beyond the ordinary Rust fixtures. It is not required for
development acceptance. Both paths retain bounded results; cleanup is part of
finishing the hosted task. Run this validation campaign on a disposable hosted
runner rather than a shared workstation.

Compose uses Linux host networking for neuron requests to the published axon.
Its private address is permitted only by the explicit `--network local` flag.
Production operators must supply a public axon IP and port, their own subnet,
read-only hotkey mount and writable state mount. The runtime image defaults to
UID/GID 65532; CI runs it as the non-root runner user to access 0600 fixture
hotkeys. The optional systemd unit in `deploy/` uses a 90-second SIGTERM stop
window. Create the `subnet` user, put wallets under `/var/lib/subnet/wallets`, and
set `BT_NETWORK`, `BT_WALLET_PATH`, `BT_WALLET`, `BT_WALLET_HOTKEY`,
`RTAO_STATE_DIR` and `NETUID` in `/etc/subnet/validator.env` or `miner.env`.

The `owner-tool` member uses the public `rtao::OwnerOps` API through the facade's
`owner` feature. It requires an existing owned subnet and an explicit setting;
it never creates or funds a subnet. After building it in hosted CI, preview an
owner setting with the resulting binary:

```sh
subnet-owner-tool --network finney --netuid 42 --wallet my-owner --immunity-period 5000
```

The default prints public plans without unlocking the coldkey or submitting a
transaction. Add `--execute` to apply the requested settings. Coldkey passwords
use the wallet's standard `BT_PW_` variable or `--password-file`. The optional
`--mechanism-count` setting accepts 1 through 16; the owner API also checks the
current runtime's cap. `--max-fee-rao` limits each transaction's estimated fee,
and the policy permits no TAO spend and only the selected subnet.

Each operation has a 300-second timeout, configurable with
`--operation-timeout-secs`. The hyperparameter operation waits for its owner
window within that deadline and reviews the current head before signing. If
both settings are supplied, the hyperparameter completes first; the mechanism
setting is a separate transaction and can fail after the first succeeds. The
tool prints each receipt and stops on failure. Always check the preview and
receipts. The tool signs with the wallet's coldkey file and does not enable
rtao's `ledger` feature; to sign owner operations with a Ledger device, enable
that feature and adapt the signer setup as described in the rtao Ledger guide.
