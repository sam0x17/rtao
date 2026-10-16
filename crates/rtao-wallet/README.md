# rtao-wallet

Btcli-compatible wallet paths, keyfiles and explicit password resolution.

`WalletRef` exposes wallet paths and public identities. `Wallet` adds key
creation, regeneration and role-specific unlocking. `ColdkeyFile` and
`HotkeyFile` can supply a single signing role; `Keyfile` handles an arbitrary
keyfile path. Creating a wallet handle does not unlock its keys.

Writes require an explicit overwrite decision and use atomic replacement.
Secret files have owner-only permissions on Unix. Public views read public
fields without decrypting an encrypted keyfile. Asynchronous unlocking and
key writes move blocking work onto the worker pool.

`PasswordSources` controls wallet password precedence: explicit input, wallet
and keyfile environment variables, a password file, then enabled native
providers and prompt callbacks. `KeyPasswordSources` gives generic keyfiles
their separate explicit/environment/prompt contract. Prompting is disabled
by default; applications supply any prompt or native password provider.

## Features

The crate has no default features. `encrypted` enables the pinned core's
encrypted-keyfile support and the `EncryptedMessage`/`encrypt_message` API.
`keychain`, `keyring` and `prompt` are declared integration flags; applications
still supply their providers and callbacks explicitly.

Native signing uses `rtao-signer`. The pinned core's opaque Ed25519 signing
object has no guaranteed final-drop erasure; the signer documentation records
that limit.
