// Derived from RaoFoundation/subtensor sdk/python/tests/unit/test_wallet_compat.py @ 850f2755bb05a3851f948489f0ab27b0645cebce (Apache-2.0); modified by rtao.
//! Wallet compatibility through explicit native byte inputs and public views.

mod support;

use rtao_core::{keyfile_json, keys::Keypair};
use rtao_signer::{CryptoType, KeyView, SignerRole};
use rtao_wallet::{Overwrite, RegenerateFrom, Wallet, WriteOptions, read_keyfile_data};
use std::error::Error;
use support::TempRoot;
use zeroize::Zeroizing;

type TestResult = Result<(), Box<dyn Error>>;

// pyt: unit/test_wallet_compat.py::test_sp_core_backwards_compat_aliases
// Native API adaptation: Python function-object alias identity has no Rust
// counterpart. The actual APIs are rtao_core::keys::{verify, ss58_from_public,
// public_key_from_ss58}. Do not invent aliases here to manufacture this test.

#[test]
fn test_keypair_sign_accepts_str_and_hex() -> TestResult {
    // pyt: unit/test_wallet_compat.py::test_keypair_sign_accepts_str_and_hex
    let keypair = Keypair::from_uri("//Alice", 1)?;
    let message = b"hello";
    // Native API adaptation: sign accepts &[u8]. Callers explicitly encode
    // UTF-8 or decode hex; a Rust string is never implicitly hex-decoded.
    let sig_bytes = keypair.sign(message)?;
    let sig_str = keypair.sign("hello".as_bytes())?;
    let sig_hex = keypair.sign(&hex::decode("68656c6c6f")?)?;
    assert!(keypair.verify(message, &sig_bytes)?);
    assert!(keypair.verify(message, &sig_str)?);
    assert!(keypair.verify(message, &sig_hex)?);
    Ok(())
}

fn plaintext_overwrite() -> WriteOptions {
    WriteOptions {
        encrypt: false,
        password: None,
        overwrite: Overwrite::Yes,
    }
}

#[tokio::test]
async fn test_regenerate_coldkey_accepts_bytes_seed() -> TestResult {
    // pyt: unit/test_wallet_compat.py::test_regenerate_coldkey_accepts_bytes_seed
    let root = TempRoot::new();
    let seed = Zeroizing::new((0u8..32).collect::<Vec<_>>());
    let expected = Keypair::from_seed(&seed, 1)?;
    let wallet = Wallet::new("bytes-seed", "default", root.path())?;
    let account = wallet
        .regenerate_coldkey(
            RegenerateFrom::Seed(seed),
            CryptoType::Sr25519,
            plaintext_overwrite(),
        )
        .await?;

    // Compare the real wallet's returned, persisted, public and signing
    // identities with the core key independently derived from bytes(range(32)).
    assert_eq!(account.0, expected.public_key_bytes());
    assert_eq!(account.to_string(), expected.ss58_address());
    assert!(account.to_string().starts_with('5'));
    assert_eq!(wallet.coldkey_address()?, account);
    let public = wallet.public_info(SignerRole::Coldkey)?;
    assert_eq!(public.account, account);
    assert_eq!(public.crypto_type, CryptoType::Sr25519);
    let data = read_keyfile_data(wallet.coldkey_file().path())?;
    let restored = keyfile_json::deserialize_keypair_from_keyfile_data(&data)?;
    assert_eq!(restored.public_key_bytes(), expected.public_key_bytes());
    assert_eq!(
        restored.seed_bytes(),
        Some((0u8..32).collect::<Vec<_>>().as_slice())
    );
    let signer = wallet.unlock_coldkey().await?;
    assert_eq!(signer.account(), account);
    let signature = signer.sign_raw(b"bytes-seed").await?;
    assert!(expected.verify(b"bytes-seed", &signature)?);
    Ok(())
}

#[tokio::test]
async fn test_wallet_coldkey_and_hotkeypub_properties() -> TestResult {
    // pyt: unit/test_wallet_compat.py::test_wallet_coldkey_and_hotkeypub_properties
    let root = TempRoot::new();
    let wallet = Wallet::new("props", "default", root.path())?;
    let cold = wallet
        .create_new_coldkey(12, CryptoType::Sr25519, plaintext_overwrite())
        .await?;
    let hot = wallet
        .create_new_hotkey(12, CryptoType::Sr25519, plaintext_overwrite())
        .await?;

    // Native API adaptation: private Python Keypair properties become lazy
    // Signers. PublicKeyInfo/KeyView exposes the public companion identity.
    // Exercise both actual private signers instead of constructing aliases.
    for (role, generated, signer) in [
        (
            SignerRole::Coldkey,
            cold.account,
            wallet.unlock_coldkey().await?,
        ),
        (
            SignerRole::Hotkey,
            hot.account,
            wallet.unlock_hotkey().await?,
        ),
    ] {
        let public = wallet.public_info(role)?;
        let reference = wallet.reference().public_info(role)?;
        assert_eq!(signer.account(), public.account);
        assert_eq!(signer.account().to_string(), public.account.to_string());
        assert_eq!(public, reference);
        assert_eq!(public.account, generated);
        assert_eq!(public.crypto_type, CryptoType::Sr25519);
        let signature = signer.sign_raw(b"wallet-compat-properties").await?;
        assert!(rtao_core::keys::verify(
            b"wallet-compat-properties",
            &signature,
            &public.account.to_string(),
            1,
        )?);
    }
    Ok(())
}
