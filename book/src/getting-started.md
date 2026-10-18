# Getting started

rtao is under development and has no crates.io release yet. Use a reviewed
source revision and its matching core pin; release binaries and release tags
are not yet published. Check the hosted CI results for the exact commit you
use.

For an application beside a checked-out `rtao` directory, start with the read
features:

```toml
[dependencies]
rtao = { path = "../rtao/crates/rtao", default-features = false, features = ["client", "reads"] }

[patch.crates-io]
w3f-bls = { git = "https://github.com/RaoFoundation/subtensor.git", tag = "v478" }
```

Adjust the path to your checkout. For a git dependency, pin `rev` to the full
reviewed commit instead. Cargo applies `[patch.crates-io]` from your application's
workspace root. Its Git reference kind and value must match the core pin in
that revision's `upstream.toml`; the example above matches the current `v478`
pin. The CLI checks this without connecting to a node:

```sh
rtao doctor --manifest-path Cargo.toml
```

## Connect and read one block

Pass an existing endpoint or a named network such as `finney` or `test` to
`ClientConfig::for_network`. You do not need to operate a node. A snapshot pins
all its reads to one block hash; separate reads through a live client can see
different heads.

The generated Offline examples section includes the actual repository test
for a finalized balance read. It seeds encoded `System.Account` state through
`FakeChain`, reads with the public namespace and checks the snapshot hash.
That example uses no wallet or endpoint.

For an existing endpoint, connect through `Client::connect` with the resolved
`ClientConfig`, obtain a finalized snapshot with `Client::at`, then use
`snapshot.balances().balance(coldkey)`. The returned balance keeps exact
integer rao. Call `client.close().await` when finished, including when a read
fails. Client clones share the connection, so closing one closes that shared
client.

Balances use checked native `u64` rao. Keep TAO and subnet alpha units distinct;
negative amounts and values above the chain's balance range are rejected.

RPC subscription storage has separate aggregate byte limits: 8 MiB for early
notifications and 32 MiB for active streams by default, configured through
`SessionConfig::early_notification_bytes` and `subscription_buffer_bytes`.
Overflow drops the oldest queued updates and emits an overflow event. Retained
streams share the budget across reconnects. These limits cover queued data;
frames, parsing and application-owned results require additional memory.

Persisted metadata cache entries are capped at 64 MiB, and metagraph files at
256 MiB, including their headers. Readers validate the opened regular file and
also bound acquisition if a file grows after validation. Oversized metadata
cache entries are treated as misses. Invalid metagraph files return an error.

## Choose the client surface

Add `intents-all` and `wallet-encrypted` for the full intent catalog and
encrypted wallet files, or use the `owner`, `validator` or `miner` feature for
the corresponding framework. `Executor::plan` builds and reviews an intent
using public wallet information. Execution resolves a private signer only
after policy and preflight checks. A preview still makes chain reads and fee
queries; it does not submit a transaction. The [owner guide](owner.md) shows
that flow with a fee cap and subnet scope.

The current CLI includes `wallet`, `weights`, `axon`, `hparams`, `subnets`,
`stake`, `proxy` and `multisig`, plus generated `tx` and `query` commands,
`call`, `config`, `tools`, `explain` and `doctor`. Use each command's `--help`
for the enabled schema. `--dry-run` previews a supported mutation; `--json`
selects machine-readable output. The [transaction reference](tx-reference.md)
lists catalog operations and their feature requirements. The
[API index](api-index.md) maps Python names to Rust.

## Wallet sources

`WalletRef` inspects the public btcli layout; `Wallet` adds independent coldkey
and hotkey sources with lazy shared signer caches. `Wallet::default_wallet()`
uses `default/default` under the current `HOME`'s `.bittensor/wallets`; explicit
constructors take the names and root. The library does not read `BT_WALLET_PATH`.

Choose `CryptoType`, a single `RegenerateFrom` source and `WriteOptions` for
creation or recovery. `WriteOptions::coldkey()` requests encryption and
`WriteOptions::hotkey()` requests plaintext; both refuse overwrite. Supplying
a password to a role-specific wallet write forces encryption. New keys return
a `GeneratedKey` with a guarded mnemonic; the library never prints it. To
create both keys, call `create_new_coldkey` and `create_new_hotkey` and retain
both recovery values. The CLI's `wallet create` composes those operations;
publishing two keys is not one transaction.

Configured `PasswordSources` govern encrypted role-specific writes as well as
unlocking, unless `WriteOptions.password` overrides them. Coldkey sources can
include wallet environment values, password files and explicitly supplied
native services. Hotkeys use their own policy. A role write invokes its SDK
password callback once; that callback owns confirmation. CLI writing callbacks
confirm new passwords. Generic `Keyfile` writes use the separate explicit or
confirming-prompt policy described below. The SDK does not prompt by default.

Wallet methods omit Python's automatic `save_coldkey_to_env` and
`save_hotkey_to_env` flags. Use an explicit generic `Keyfile` save operation
over a caller-owned environment, or the CLI's `wallet save-password-env` export.
Neither makes a wallet automatically save a password or shares a new overlay
with its password sources.

Wallet inventory and natural sorting currently live in the CLI. There is no
`rtao_wallet::list_wallets` or `list_wallets_detailed`; the CLI's detailed JSON
uses formatted scheme labels. Library code selects a typed `CryptoType`
instead of Python's root string parsing/formatting helpers.

Signer clones share the upstream keypair until the final `Arc` owner drops.
The pinned core stores Ed25519 signing state in a wiping
`Zeroizing<SigningKey>` owner, so the final drop wipes the stored seed,
expanded scalar and nonce prefix. This does not cover
allocator or OS copies, stack copies left by moving the key, short-lived values
inside cryptographic dependencies and KDFs, or caller-owned inputs.

## Generic keyfiles

`rtao_wallet::Keyfile` reads or writes one explicit path, including a public-only
keyfile. It does not assign a signing role or create a companion file. Use
`ColdkeyFile` or `HotkeyFile` when an operation needs a role-checked signing
source. File reads are capped at 16 MiB; writes are atomic and owner-only, with
explicit overwrite permission instead of an implicit terminal confirmation.
Writes refuse symlink targets and symlinked parent paths. Reads support only
regular files, including regular symlink targets. Encrypted reads and writes
require the `rtao-wallet/encrypted` feature.

`Keyfile::get_keypair` selects a nonempty explicit password, then this file's
encoded `BT_PW_...` variable, then an enabled hidden-input callback. The public
`resolve_key_password` function accepts `KeyPasswordSources` for the same
resolution with a custom variable name, prompt message and confirmation policy.
The SDK never prompts by default. Wallet-wide variables, password files and
native dialogs belong to the separate `PasswordSources` wallet policy.

The generic `set_keypair` honors `WriteOptions.encrypt` even when a password is
present. `WriteOptions::coldkey()` requests encryption; `WriteOptions::hotkey()`
requests plaintext. A new encryption password comes from explicit input or an
enabled confirming prompt. Saving it to the environment happens only after a
successful file write.

Python changes its global process environment when saving a keyfile password.
The Rust SDK mutates an explicitly owned `KeyfileEnvironment` instead. Its
default `KeyfileEnvironmentOverlay` reads existing process variables, keeps
new values in memory and masks removed values. Clones share that overlay;
independently constructed keyfiles can share it through `KeyPasswordSources`.
This avoids unsafe process-environment mutation in a multithreaded application.
Saved values use Python's reversible XOR/base64 encoding and remain secrets.

## Development and coverage

Source review and formatting stay local. Substantial builds, tests, dependency
checks and documentation builds belong on free GitHub `ubuntu-latest` runners.
Ordinary client validation uses Rust tests, committed vectors and fake
transports. It does not require a local node, Docker, Python, a testnet wallet
or a validator soak. Optional live-chain and interoperability evidence is
reported separately when it exists. Documentation and source availability do
not establish that the latest hosted run passed; consult the checks for the
exact commit you use.
