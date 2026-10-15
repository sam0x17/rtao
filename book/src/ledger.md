# Ledger signing

Enable the `ledger` feature and use
`rtao::signer::LedgerSigner::open(LedgerOptions::default()).await`. Opening
discovers the device and derives its public ed25519 account. The default path
is `m/44'/354'/0'/0'/0'`; `account` and `index` choose its hardened components.
The CLI exposes `--ledger-account` and `--ledger-index` with `--signer ledger`.

`app_version().await` reads the app version, and `confirm_address().await`
asks the device to display its address. Signing requires a complete prepared
transaction context. The client uses the runtime's name/version and the
signer's token display constants for the metadata digest and proof. Ledger
receives the unhashed concatenation of call and signed-extension parts.
Arbitrary message signing is refused.

Complete device operations share one process-wide lock inside their blocking
worker. Dropping an async waiter cannot interrupt an APDU exchange or release
that lock halfway through it. A driver operation already in progress can
continue after cancellation. Plan application deadlines accordingly.

Affected Polkadot app versions display amounts typed `TaoBalance` as raw
integers. A released formatting fix has not been verified here. The
`limit_price` values in `swap_stake_limit` and `move_stake_limit` represent
price ratios scaled by 1e9; they require separate interpretation even if an
app formats that type as TAO. Check units before approving on the device.

The implementation has fake-backend and metadata-proof fixture sources.
Physical HID/device behavior, firmware/app versions and a fresh emulator
comparison remain outside the current hosted client acceptance. Development
does not require installing or rehearsing those tools locally.
