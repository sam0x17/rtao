Malformed secret URIs can expose the complete phrase and derivation password
through `CoreError` output. Both branches of
[`Keypair::from_uri`](https://github.com/RaoFoundation/subtensor/blob/850f2755bb05a3851f948489f0ab27b0645cebce/sdk/bittensor-core/src/keys/mod.rs#L263)
format `sp_core::Pair::from_string` failures with `Debug`.

The pinned sp-core
[`InvalidCharacterInfo`](https://github.com/RaoFoundation/polkadot-sdk/blob/cacb4310f20c7cac83eb3ccd8ed5a5ad4212608a/substrate/primitives/core/src/address_uri.rs#L83)
owns a copy of the complete original URI and derives `Debug`. A valid public
test mnemonic followed by `///public-disposable-password` and a newline
causes that copied input to appear in the core's returned diagnostic for
both Sr25519 and Ed25519. Applications that log or display this ordinary
construction error can therefore expose the phrase and password.

A Rust integration test now reproduces this through the actual
`bittensor_core::keys::Keypair::from_uri` API for both algorithms, checking
the returned `CoreError::Crypto` category and both Display and Debug outputs.
The valid password URI also succeeds and matches derivation from the same
phrase/password; adding the newline triggers the disclosure. It passed on
rustc 1.99.0 and Rust 1.89.0/aarch64 macOS against the exact unchanged
upstream v478 lockfile. The original locked-dependency formatter probe
also passed. No user wallet or process secret was read.

Please replace input-bearing derivation diagnostics with fixed messages or
safe error categories, preserving the `Crypto` error category. Add Display
and Debug regressions for both algorithms, including the full error-source
chain. The fix should also cover mnemonic, seed, private-key and encrypted
JSON construction paths wherever a dependency can include secret input.

Rtao currently sanitizes these errors at its owned boundaries and wipes the
discarded core diagnostic String. Tests cover both algorithms, wallet
regeneration and error-source chains on current Rust and Rust 1.89.
This prevents the reported disclosure through those tested signer, wallet
and codec construction boundaries while leaving successful derivation and
wire behavior in the pinned core. Direct use of the re-exported upstream
Keypair API retains upstream diagnostics until this is fixed in the core.


Reproduce at v478 by saving this as `sdk/bittensor-core/tests/secret_uri_diagnostics_repro.rs`:

```rust
use bittensor_core::{
    keys::{Keypair, CRYPTO_ED25519, CRYPTO_SR25519},
    CoreError,
};

#[test]
fn malformed_secret_uri_error_includes_phrase_and_password() {
    // Published BIP39 test material and a disposable password.
    let phrase = "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about";
    let password = "public-disposable-password";
    let uri = format!("{phrase}///{password}\n");

    for (name, scheme) in [("ed25519", CRYPTO_ED25519), ("sr25519", CRYPTO_SR25519)] {
        let error = match Keypair::from_uri(&uri, scheme) {
            Ok(_) => panic!("{name}: malformed URI unexpectedly succeeded"),
            Err(error) => error,
        };
        assert!(matches!(&error, CoreError::Crypto(_)));

        let display = error.to_string();
        let debug = format!("{error:?}");
        for diagnostic in [&display, &debug] {
            assert!(diagnostic.contains(phrase), "{name}: phrase missing");
            assert!(diagnostic.contains(password), "{name}: password missing");
        }
        println!("{name}: CoreError Display and Debug contain phrase and password");
    }
}
```

```sh
PYO3_NO_PYTHON=1 cargo +1.89.0 test --locked -p bittensor-core \
  --no-default-features --test secret_uri_diagnostics_repro -- --nocapture
```
