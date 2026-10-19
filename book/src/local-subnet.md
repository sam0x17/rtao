# Build a subnet on a local chain

This walkthrough takes a new subnet from an empty directory to a validator and
miner exchanging authenticated requests and setting weights on a local
subtensor chain. Every step runs on one Linux or macOS machine with Docker.

## Prerequisites

- Rust 1.89 or newer and Docker.
- [`cargo-generate`](https://github.com/cargo-generate/cargo-generate) 0.23.12
  or newer: `cargo install cargo-generate`.
- The `rtao` command line tool:

  ```sh
  cargo install --locked --git https://github.com/sam0x17/rtao rtao-cli
  ```

## 1. Generate the project

```sh
cargo generate --git https://github.com/sam0x17/rtao templates/subnet --name my-subnet
cd my-subnet
cargo build --release
```

The workspace contains a hotkey-only validator, a btauth/1 echo miner and a
coldkey owner tool. Its `README.md` explains where to put your own protocol.

## 2. Start a local chain

Run the localnet image that rtao pins (the `pinned` row in
[`COMPAT.md`](https://github.com/sam0x17/rtao/blob/main/COMPAT.md)). `True`
selects fast blocks (about four per second):

```sh
docker run -d --name localnet -p 127.0.0.1:9944:9944 \
  ghcr.io/raofoundation/subtensor-localnet@sha256:c2eb81b1a39d89ad7bcca5e36a3e7f39f589a6fa422be66d5d03b59a8584957e True
```

Every development chain shares one genesis, so nodes on a shared Docker
network join each other's chains. Run one localnet per Docker network: give a
second chain its own network (`docker network create` and `--network`).
Removing the container discards the chain.

Point the CLI at it:

```sh
export BT_NETWORK=ws://127.0.0.1:9944 BT_FALLBACK_ENDPOINTS=none BT_ARCHIVE_ENDPOINTS=none
export BT_WALLET=owner
```

## 3. Create wallets

The localnet funds the well-known development account Alice. Use its public
development seed only on local chains:

```sh
rtao wallet regen-coldkey --no-password \
  --seed 0xe5be9a5092b81bca64be81d212e7f2f9eba183bb7a90954f7b76361f6edb5c0a
rtao wallet new-hotkey --wallet-hotkey default
rtao wallet new-hotkey --wallet-hotkey validator
rtao wallet new-hotkey --wallet-hotkey miner
```

`subnets create` registers the subnet with the wallet's `default` hotkey as the
owner hotkey.

## 4. Create and start the subnet

```sh
rtao subnets create --yes          # prints the new netuid; this guide uses 2
rtao hparams start --netuid 2 --yes
rtao subnets register --netuid 2 --wallet-hotkey validator --yes
rtao subnets register --netuid 2 --wallet-hotkey miner --yes
rtao stake add --netuid 2 --wallet-hotkey validator --amount 0.01 --yes
```

A new subnet's pool holds only its 1 TAO lock, so larger stakes exceed the
default 5% slippage protection (`SlippageTooHigh`). Stake small amounts, or
pass `--rate-tolerance`. Registration and staking are MEV-shielded by default;
on fast blocks the client waits for finality to leave room in the shield era
and re-seals a call whose encrypted inner extrinsic was dropped, so expect an
occasional extra second.

## 5. Run the miner and validator

```sh
common="--network local --chain-endpoint ws://127.0.0.1:9944 \
  --fallback-endpoints none --archive-endpoints none \
  --netuid 2 --wallet owner --log info"
./target/release/subnet-miner $common --wallet-hotkey miner --state-dir state/miner \
  --axon.external-ip 127.0.0.2 --axon.port 8091 &
./target/release/subnet-validator $common --wallet-hotkey validator --state-dir state/validator &
```

The chain refuses to publish an axon at exactly `127.0.0.1`; `127.0.0.2` is
also loopback and reaches the miner. Each neuron keeps its checkpoint in its
`--state-dir`; use a fresh directory when you change the netuid or hotkey.

The validator logs `btauth echo accepted` for each authenticated query and
`weights committed` once it holds a validator permit. The chain reveals the
weights after the reveal period, and the epoch after that pays incentive and
dividends:

```sh
rtao subnets metagraph 2
rtao query weights --netuid 2
rtao query blocks-since-last-update --netuid 2 --uid 1
```

`subnets metagraph --json` returns the runtime's `get_metagraph` record as is.
Current runtimes no longer compute `rank`, `trust` or `pruning_score`, so those
arrays are empty; use `consensus`, `incentives` and `dividends`. Consensus
follows stake: if the subnet owner's hotkey holds most of the stake weight (on
a public network its root stake counts), the owner must also set weights before
miners earn incentive.

## 6. Make it yours

Replace the challenge and scoring rule in `validator/src/main.rs` and the
`/echo` handler in `miner/src/main.rs`. `bash ci/check.sh --localnet`, run
with the variables in the template README, rebuilds both neuron images and
repeats this flow under Docker Compose against a scenario-registered subnet.
