# rtao-core

The curated interface to the pinned `bittensor-core`. This crate reexports
chain codecs and cryptography and exposes the portable extensions maintained
in `rtao-core-ext`.

Use `codec` and `runtime` for dynamic SCALE values and metadata, `extrinsic`
for era and multisig helpers, and `keys` for key derivation and verification.
`digest`, `mlkem` and `timelock` expose the corresponding upstream formats.
`DynValue` provides JSON conversion, and `CoreError` is the owned error type.

| Feature | Effect |
| --- | --- |
| `offload` (default) | Adds `offload`, which runs blocking work through Tokio's blocking pool. An entered Tokio runtime is required. |
| `host` | Adds encrypted `keyfiles` and proof-of-work helpers in `pow`. |
| `ledger` | Adds the upstream `ledger` device API. |

Upstream secret constructors can include recovery material in errors. The
higher-level `rtao-signer` and `rtao-wallet` APIs redact those diagnostics at
their boundaries. Direct calls through `keys::Keypair` retain upstream behavior.

Downstream workspaces must carry the matching `w3f-bls` patch described in the
[workspace README](https://github.com/sam0x17/rtao#downstream-bls-patch).
