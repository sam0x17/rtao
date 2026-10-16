# External signers and timelocks

Enable `ledger`, `vault` or `extension` on the `rtao` facade or CLI to select
that backend. Libraries start no device, page server or daemon at import or
construction. Network, device and browser operations occur through explicit
async methods. The caller owns client and signer lifetimes.

## Review and account selection

The CLI accepts `--signer ledger|vault|extension`. An explicit
`--signer-address` binds the transaction to a public account; account-book and
wallet references are supported. Ledger and extension commands can discover
an unpinned public account. Vault needs a known public address, supplied by a
flag, selected multisig member, configuration or wallet companion file.

Public account discovery can contact the selected device or extension, but
does not authorize a transaction. Signing runs after the transaction review,
confirmation and durable preparation hook. The acquired account must match
the reviewed account. A pinned `--dry-run` performs no device or browser I/O;
an unpinned preview asks for a public address instead of starting discovery.

External signers receive the prepared transaction views through `Signer`.
Ledger and Vault use runtime metadata proofs. The client verifies the exact
account, payload and signature algorithm before attaching a signature.
Failures after transmission can retain an unknown-outcome hash. Reconcile
that hash before deciding whether to submit again.

## Vault and browser extension

`rtao::signer::VaultSigner::new(address)` validates an offline public identity.
Its signing operation starts a loopback page with embedded upstream assets,
displays UOS QR frames and verifies the phone's answer. `VaultOptions` controls
the algorithm, browser opening and deadlines. `close().await` releases the
page server. Shield submission calls `Signer::prepare_shielded` before choosing
the mortal-era anchor or reserving paired nonces. Vault uses this hook to wait
for the page connection and request camera startup. Phone unlocking and both
signature scans still need to finish within the signed era.
`vault::scan_vault_address` supports address scans separately.

Custom interactive signers can implement `prepare_shielded` for setup that
should precede the short era. Its default does nothing, and the submission
deadline also bounds preparation. This hook must not sign or broadcast a
transaction. Keep its account and signature algorithm fixed throughout
preparation; the client refuses either identity change before reserving nonces.

Vault and extension browser opening share the explicit browser choice,
environment fallback and executable aliases. Launching returns after spawning;
closing a signing session leaves the browser application open. Unix uses a
separate process group and the system launcher when no chosen executable is
available. Python's additional `webbrowser` backends and `BROWSER` handling
are outside this native launcher API.

The QR helper selects one numeric, alphanumeric or byte segment with error
correction M and a four-module quiet zone. Binary UOS frames use byte mode.
The helper preserves decoded bytes; SVG serialization and masks may differ
between encoders.

The `extension` library exposes `BridgeServer`, `BridgeClient`, account
selection and `ExtensionSigner`. Daemon startup is explicit. CLI commands
`misc extension bridge`, `accounts` and `stop` use compatible token/PID paths.
Tokens authorize the loopback protocol and must remain private. Reusing a
foreground bridge does not authorize killing an arbitrary process from a PID
file. Signer sessions release their UI and transports on completion or drop.

## Timelocks

`rtao::tx::timelock::seal(data, Reveal::In(duration)).await` creates the core's
portable Quicknet envelope without contacting a beacon. Other reveal forms
are `At`, `Round` and `InBlocks`; the last estimates twelve-second blocks by
default and has an explicit block-duration override. Beacon time continues
independently of chain progress.

`open(&sealed, &DrandClient::new()?, OpenOptions { ... }).await` fetches the
requested beacon round and decrypts through the core. Set a total timeout for
an application deadline. `OpenOptions::default()` waits until reveal; CLI
`misc timelock decrypt` waits only when `--wait` is supplied. `show` inspects
an envelope offline. CLI encryption limits plaintext to slightly under 8 MiB
so its emitted hexadecimal envelope fits the 16 MiB input limit. Plaintext
is omitted from replay history. Applications own plaintext returned by `open`.

Source review and fixture presence do not establish physical device, browser,
camera, native Keychain or cross-language execution. Consult the execution
state and CI results for the checked revision's actual coverage.
