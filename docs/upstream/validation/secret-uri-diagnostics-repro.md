# Reproduction for subtensor #3252

Run this regression from a subtensor checkout at v478,
`850f2755bb05a3851f948489f0ab27b0645cebce`. Save the following as
`sdk/bittensor-core/tests/secret_uri_diagnostics_repro.rs`:

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

The final `\n` in the URI is an actual newline. At v478, this test demonstrates
the disclosure by asserting that both error renderings contain the complete
phrase and password. After sanitizing diagnostics, invert those containment
assertions to turn it into a regression against disclosure.

The exact core API and constants above were checked against the locked source:
[`Keypair::from_uri`](https://github.com/RaoFoundation/subtensor/blob/850f2755bb05a3851f948489f0ab27b0645cebce/sdk/bittensor-core/src/keys/mod.rs#L263)
formats both pinned `sp_core::Pair::from_string` errors with `Debug` and wraps
the resulting String in `CoreError::Crypto`. That error's Display adds the
`crypto error:` prefix; its derived Debug retains the same String.

Existing verification used this exact phrase, password and newline against
locked sp-core `cacb4310f20c7cac83eb3ccd8ed5a5ad4212608a`. The bounded Rust
probe applied the core's exact error formatter for both algorithms, and all
phrase/password containment assertions passed on rustc 1.99.0/aarch64 macOS:

```text
crypto=0: pinned-core URI error contains both recovery phrase and password
crypto=1: pinned-core URI error contains both recovery phrase and password
```

The probe and cleanup evidence are retained in
`target/execute-plan.6Ro0PC/tmp/m2-security-proof.txt` and described in
[the independent M2 review](../../security/m2.md). The integration test above
was prepared from that verified source and probe; it has not been compiled or
run as a new test during this documentation task.
