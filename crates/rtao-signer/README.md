# rtao-signer

Asynchronous signing contracts and public key sources for Bittensor.

`Signer` receives a `SigningRequest` containing the exact prepared payload,
call bytes, extension fields, JSON view and runtime context. `SignerCaps`
describes the formats and interactions a backend accepts. `MultiSignature`
retains the declared Ed25519 or Sr25519 algorithm with its 64-byte signature.

`KeypairSigner` wraps the pinned core's software keypair. `KeyView` and the
role-specific `KeySource` contracts let callers plan with public identities
before acquiring a signer. Hardware backends can refuse arbitrary raw-message
signing through the default `Signer::sign_raw` implementation.

## Features

The crate has no default features.

| Feature | API |
| --- | --- |
| `ledger` | `ledger` module and `LedgerSigner`, using the core device API and runtime metadata proofs. |
| `vault` | `vault` module and `VaultSigner`, with explicit offline-signing sessions and QR handling. |
| `extension` | `extension` module and `ExtensionSigner`, with an authenticated browser-extension bridge. |
| `test-util` | Injected backend access for supported signer fixtures. |

Constructing a Vault or extension signer starts no browser or server. Device,
browser and bridge operations require explicit calls.

`KeypairSigner` shares its keypair until its final owner drops. The core wipes
separately retained seed and mnemonic buffers, but its opaque Ed25519 signing
object has no guaranteed secret wipe on drop. Borrowed constructor inputs
remain the caller's responsibility.
