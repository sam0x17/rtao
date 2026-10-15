//! Disposable btcli-layout tests; no real wallet paths or process env mutations.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod support;
use rtao_core::keys::Keypair;
use rtao_signer::{CryptoType, DynKeySource, KeyView, SignerRole};
#[cfg(not(feature = "encrypted"))]
use rtao_wallet::ColdkeyFile;
#[cfg(feature = "encrypted")]
use rtao_wallet::PasswordSources;
use rtao_wallet::{
    ExposeSecret, HotkeyFile, Overwrite, RegenerateFrom, SecretString, Wallet, WalletError,
    WalletRef, WriteOptions,
};
use std::{fs, sync::Arc};
use support::TempRoot;
use zeroize::Zeroizing;

fn uri(value: &str) -> RegenerateFrom {
    RegenerateFrom::Uri(SecretString::new(value.into()))
}
fn alice() -> rtao_signer::AccountId {
    rtao_signer::AccountId(Keypair::from_uri("//Alice", 1).unwrap().public_key_bytes())
}

fn assert_secret_error_is_redacted(error: &dyn std::error::Error, secrets: &[&str]) {
    let mut current = Some(error);
    while let Some(error) = current {
        for rendered in [error.to_string(), format!("{error:?}")] {
            for secret in secrets {
                assert!(
                    !rendered.contains(secret),
                    "wallet error retained public test sentinel"
                );
            }
        }
        current = error.source();
    }
}

#[tokio::test]
async fn regeneration_error_chains_redact_disposable_secret_input_sentinels() {
    let temp = TempRoot::new();
    let wallet = Wallet::new("redaction", "worker", temp.path()).unwrap();
    let phrase = "bottom drive obey lake curtain smoke basket hold race lonely fit walk";
    let password = "rtao-public-disposable-wallet-password-sentinel";
    let malformed_uri = format!("{phrase}///{password}\n");
    let malformed_phrase = "rtao-public-disposable-wallet-mnemonic-sentinel";
    let malformed_private = "rtao-public-disposable-wallet-private-key-sentinel";
    let malformed_seed = b"rtao-public-disposable-wallet-invalid-seed-sentinel";
    for crypto in [CryptoType::Ed25519, CryptoType::Sr25519] {
        for (source, kind, sentinels) in [
            (
                uri(&malformed_uri),
                "secret URI",
                vec![phrase, password, malformed_uri.as_str()],
            ),
            (
                RegenerateFrom::Mnemonic(SecretString::new(malformed_phrase.into())),
                "mnemonic",
                vec![malformed_phrase],
            ),
            (
                RegenerateFrom::PrivateKey(SecretString::new(malformed_private.into())),
                "private key",
                vec![malformed_private],
            ),
            (
                RegenerateFrom::Seed(Zeroizing::new(malformed_seed.to_vec())),
                "seed",
                vec!["rtao-public-disposable-wallet-invalid-seed-sentinel"],
            ),
        ] {
            let error = wallet
                .regenerate_hotkey(source, crypto, WriteOptions::hotkey())
                .await
                .unwrap_err();
            assert_eq!(
                error.to_string(),
                format!("crypto error: could not derive keypair from {kind}")
            );
            assert_secret_error_is_redacted(&error, &sentinels);
            assert!(!wallet.reference().hotkey_path().exists());
            assert!(!wallet.reference().hotkeypub_path().exists());
        }
    }
}

#[tokio::test]
async fn valid_disposable_recovery_inputs_keep_pinned_derivation_and_codec_bytes() {
    let temp = TempRoot::new();
    let wallet = Wallet::new("derivation", "worker", temp.path()).unwrap();
    let phrase = "bottom drive obey lake curtain smoke basket hold race lonely fit walk";
    let uri_text = format!("{phrase}//42///rtao-public-disposable-wallet-derivation-password");
    for crypto in [CryptoType::Ed25519, CryptoType::Sr25519] {
        let upstream = Keypair::from_uri(&uri_text, u8::from(crypto)).unwrap();
        let account = wallet
            .regenerate_hotkey(
                uri(&uri_text),
                crypto,
                WriteOptions {
                    overwrite: Overwrite::Yes,
                    ..WriteOptions::hotkey()
                },
            )
            .await
            .unwrap();
        assert_eq!(account.as_bytes(), &upstream.public_key_bytes());
        let restored = rtao_core::keyfile_json::deserialize_keypair_from_keyfile_data(
            &fs::read(wallet.reference().hotkey_path()).unwrap(),
        )
        .unwrap();
        assert_eq!(restored.public_key_bytes(), upstream.public_key_bytes());
        assert_eq!(restored.private_key_bytes(), upstream.private_key_bytes());
        assert_eq!(restored.crypto_type(), upstream.crypto_type());
    }
}

#[cfg(feature = "encrypted")]
#[tokio::test]
async fn encrypted_import_error_chains_redact_json_and_passphrase_sentinels() {
    let temp = TempRoot::new();
    let wallet = Wallet::new("encrypted-redaction", "worker", temp.path()).unwrap();
    let json = "rtao-public-disposable-encrypted-json-sentinel";
    let password = "rtao-public-disposable-encrypted-password-sentinel";
    let source = RegenerateFrom::EncryptedJson {
        json: SecretString::new(json.into()),
        passphrase: SecretString::new(password.into()),
    };
    let error = wallet
        .regenerate_coldkey(source, CryptoType::Sr25519, WriteOptions::hotkey())
        .await
        .unwrap_err();
    assert_eq!(
        error.to_string(),
        "crypto error: could not derive keypair from encrypted JSON"
    );
    assert_secret_error_is_redacted(&error, &[json, password]);
    assert!(!wallet.reference().coldkey_path().exists());
    assert!(!wallet.reference().coldkeypub_path().exists());
}
#[test]
fn compound_layout_and_python_validation_messages() {
    let temp = TempRoot::new();
    let reference = WalletRef::new("cold/hot", "default", temp.path()).unwrap();
    assert_eq!(reference.name(), "cold");
    assert_eq!(reference.hotkey_name(), "hot");
    assert_eq!(reference.coldkey_path(), temp.path().join("cold/coldkey"));
    assert_eq!(
        reference.hotkeypub_path(),
        temp.path().join("cold/hotkeys/hotpub.txt")
    );
    let error = WalletRef::new("cold/hot", "different", temp.path()).unwrap_err();
    assert_eq!(
        error.to_string(),
        "give the hotkey either in the name ('cold/hot') or as the hotkey= argument, not both"
    );
    for compound in ["/hot", "cold/", "cold/hot/extra"] {
        assert_eq!(
            WalletRef::new(compound, "default", temp.path())
                .unwrap_err()
                .to_string(),
            format!(
                "invalid wallet name '{compound}': expected 'wallet/hotkey' with exactly one slash and both parts non-empty"
            )
        );
    }
}
#[tokio::test]
async fn plaintext_create_regenerate_public_views_and_role_signing() {
    let temp = TempRoot::new();
    let wallet = Wallet::new("test", "worker", temp.path()).unwrap();
    wallet
        .regenerate_coldkey(uri("//Alice"), CryptoType::Sr25519, WriteOptions::hotkey())
        .await
        .unwrap();
    let generated = wallet
        .create_new_hotkey(12, CryptoType::Ed25519, WriteOptions::hotkey())
        .await
        .unwrap();
    assert_eq!(
        generated
            .mnemonic
            .expose_secret()
            .split_whitespace()
            .count(),
        12
    );
    assert_eq!(wallet.coldkey_address().unwrap(), alice());
    assert_eq!(wallet.hotkey_address().unwrap(), generated.account);
    assert_eq!(
        wallet.public_info(SignerRole::Hotkey).unwrap().crypto_type,
        CryptoType::Ed25519
    );
    assert_eq!(
        wallet
            .reference()
            .public_info(SignerRole::Coldkey)
            .unwrap()
            .crypto_type,
        CryptoType::Sr25519
    );
    assert_eq!(
        wallet.reference().hotkey_public_key().unwrap(),
        generated.account.0
    );
    for (role, account) in [
        (SignerRole::Coldkey, alice()),
        (SignerRole::Hotkey, generated.account),
    ] {
        let signer = wallet.signer_for(role).await.unwrap();
        let signature = signer.sign_raw(b"wallet test").await.unwrap();
        assert_eq!(signer.account(), account);
        assert!(
            rtao_core::keys::verify_strict(
                b"wallet test",
                &signature,
                &account.to_string(),
                u8::from(signer.crypto_type())
            )
            .unwrap()
        );
    }
    let public: serde_json::Value =
        serde_json::from_slice(&fs::read(wallet.reference().coldkeypub_path()).unwrap()).unwrap();
    for name in ["secretPhrase", "secretSeed", "privateKey"] {
        assert!(public.get(name).is_none());
    }
    assert!(matches!(
        wallet.hotkey_only().signer_for(SignerRole::Coldkey).await,
        Err(rtao_signer::SignerError::RoleUnavailable(
            SignerRole::Coldkey
        ))
    ));
    assert!(matches!(
        wallet.coldkey_only().signer_for(SignerRole::Hotkey).await,
        Err(rtao_signer::SignerError::RoleUnavailable(
            SignerRole::Hotkey
        ))
    ));
}
#[tokio::test]
async fn public_views_never_read_or_unlock_secret_files() {
    let temp = TempRoot::new();
    let wallet = Wallet::new("public", "worker", temp.path()).unwrap();
    wallet
        .regenerate_coldkeypub(alice(), CryptoType::Sr25519, Overwrite::No)
        .await
        .unwrap();
    wallet
        .regenerate_hotkeypub(alice(), CryptoType::Sr25519, Overwrite::No)
        .await
        .unwrap();
    fs::write(wallet.coldkey_file().path(), b"$NACLgarbage").unwrap();
    fs::write(wallet.hotkey_file().path(), b"$ANSIBLE_VAULTgarbage").unwrap();
    for view in [
        &wallet as &dyn KeyView,
        wallet.reference(),
        wallet.hotkey_file(),
    ] {
        assert_eq!(view.coldkey_address().unwrap(), alice());
        assert_eq!(view.hotkey_address().unwrap(), alice());
    }
    fs::remove_file(wallet.coldkey_file().path()).unwrap();
    fs::remove_file(wallet.hotkey_file().path()).unwrap();
    assert_eq!(wallet.coldkey_address().unwrap(), alice());
    assert_eq!(wallet.hotkey_address().unwrap(), alice());
    assert_eq!(
        wallet.public_info(SignerRole::Hotkey).unwrap().account,
        alice()
    );
    assert_eq!(
        wallet.public_info(SignerRole::Coldkey).unwrap().account,
        alice()
    );
}
#[tokio::test]
async fn hotkey_view_falls_back_only_to_public_json_fields() {
    let temp = TempRoot::new();
    let path = temp.path().join("legacy-hot");
    fs::write(&path,serde_json::to_vec(&serde_json::json!({"ss58Address":alice().to_string(),"secretPhrase":"invalid private data"})).unwrap()).unwrap();
    let file = HotkeyFile::new(&path);
    assert_eq!(file.hotkey_address().unwrap(), alice());
    assert!(file.unlock().await.is_err());
    fs::write(&path, b"$NACLprivate").unwrap();
    assert!(file.hotkey_address().is_err());
}
#[tokio::test]
async fn clones_and_concurrent_unlocks_share_one_cached_signer() {
    let temp = TempRoot::new();
    let wallet = Wallet::new("cache", "worker", temp.path()).unwrap();
    wallet
        .regenerate_hotkey(uri("//Alice"), CryptoType::Sr25519, WriteOptions::hotkey())
        .await
        .unwrap();
    let a = wallet.hotkey_only();
    let b = a.clone();
    let (first, second) = tokio::join!(a.unlock(), b.unlock());
    let first = first.unwrap();
    let second = second.unwrap();
    assert!(Arc::ptr_eq(&first, &second));
    fs::remove_file(a.path()).unwrap();
    assert!(Arc::ptr_eq(&first, &a.unlock().await.unwrap()));
}
#[tokio::test]
async fn overwrite_refusal_preserves_both_files_and_regeneration_invalidates_cache() {
    let temp = TempRoot::new();
    let wallet = Wallet::new("overwrite", "worker", temp.path()).unwrap();
    wallet
        .regenerate_hotkey(uri("//Alice"), CryptoType::Sr25519, WriteOptions::hotkey())
        .await
        .unwrap();
    let first = wallet.hotkey_file().unlock().await.unwrap();
    let original = fs::read(wallet.hotkey_file().path()).unwrap();
    assert!(matches!(
        wallet
            .regenerate_hotkey(uri("//Bob"), CryptoType::Sr25519, WriteOptions::hotkey())
            .await,
        Err(WalletError::AlreadyExists(_))
    ));
    assert_eq!(fs::read(wallet.hotkey_file().path()).unwrap(), original);
    wallet
        .regenerate_hotkey(
            uri("//Bob"),
            CryptoType::Sr25519,
            WriteOptions {
                overwrite: Overwrite::Yes,
                ..WriteOptions::hotkey()
            },
        )
        .await
        .unwrap();
    assert_ne!(
        wallet.hotkey_file().unlock().await.unwrap().account(),
        first.account()
    );
    assert_eq!(
        wallet.hotkey_address().unwrap(),
        wallet.hotkey_file().unlock().await.unwrap().account()
    );
    assert!(
        !fs::read_dir(wallet.reference().hotkey_path().parent().unwrap())
            .unwrap()
            .any(|entry| entry
                .unwrap()
                .file_name()
                .to_string_lossy()
                .ends_with(".tmp"))
    );
}

#[tokio::test]
async fn independent_wallets_racing_with_no_overwrite_install_one_complete_key() {
    let temp = TempRoot::new();
    let first = Wallet::new("race", "worker", temp.path()).unwrap();
    let second = Wallet::new("race", "worker", temp.path()).unwrap();
    let (a, b) = tokio::join!(
        first.regenerate_hotkey(uri("//Alice"), CryptoType::Sr25519, WriteOptions::hotkey()),
        second.regenerate_hotkey(uri("//Bob"), CryptoType::Sr25519, WriteOptions::hotkey())
    );
    let account = match (a, b) {
        (Ok(account), Err(WalletError::AlreadyExists(_)))
        | (Err(WalletError::AlreadyExists(_)), Ok(account)) => account,
        outcomes => panic!("exactly one creator must succeed: {outcomes:?}"),
    };
    assert_eq!(first.hotkey_address().unwrap(), account);
    assert_eq!(
        first.hotkey_file().unlock().await.unwrap().account(),
        account
    );
}

#[tokio::test]
async fn mnemonic_seed_and_private_key_regeneration_preserve_core_identity() {
    let temp = TempRoot::new();
    let phrase = "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about";
    for crypto in [CryptoType::Ed25519, CryptoType::Sr25519] {
        let wallet = Wallet::new(
            format!("sources-{}", u8::from(crypto)),
            "worker",
            temp.path(),
        )
        .unwrap();
        let expected = Keypair::from_mnemonic(phrase, u8::from(crypto), None).unwrap();
        let account = wallet
            .regenerate_hotkey(
                RegenerateFrom::Mnemonic(SecretString::new(phrase.into())),
                crypto,
                WriteOptions::hotkey(),
            )
            .await
            .unwrap();
        assert_eq!(account.0, expected.public_key_bytes());
        let seed = vec![7; 32];
        let expected = Keypair::from_seed(&seed, u8::from(crypto)).unwrap();
        let replace = WriteOptions {
            overwrite: Overwrite::Yes,
            ..WriteOptions::hotkey()
        };
        let account = wallet
            .regenerate_hotkey(
                RegenerateFrom::Seed(Zeroizing::new(seed)),
                crypto,
                replace.clone(),
            )
            .await
            .unwrap();
        assert_eq!(account.0, expected.public_key_bytes());
        let private = SecretString::new(format!(
            "0x{}",
            hex::encode(expected.private_key_bytes().unwrap())
        ));
        assert_eq!(
            wallet
                .regenerate_hotkey(RegenerateFrom::PrivateKey(private), crypto, replace)
                .await
                .unwrap(),
            account
        );
    }
}
#[cfg(unix)]
#[tokio::test]
async fn owner_only_modes_and_symlink_writes_preserve_targets() {
    use std::os::unix::fs::{PermissionsExt, symlink};
    let temp = TempRoot::new();
    let wallet = Wallet::new("modes", "worker", temp.path()).unwrap();
    wallet
        .regenerate_hotkey(uri("//Alice"), CryptoType::Sr25519, WriteOptions::hotkey())
        .await
        .unwrap();
    assert_eq!(
        fs::metadata(wallet.hotkey_file().path())
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o600
    );
    assert_eq!(
        fs::metadata(wallet.reference().wallet_dir())
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o700
    );
    assert_eq!(
        fs::metadata(wallet.hotkey_file().path().parent().unwrap())
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o700
    );
    let victim = temp.path().join("victim");
    fs::write(&victim, b"preserve").unwrap();
    fs::remove_file(wallet.hotkey_file().path()).unwrap();
    symlink(&victim, wallet.hotkey_file().path()).unwrap();
    assert!(matches!(
        wallet
            .regenerate_hotkey(
                uri("//Bob"),
                CryptoType::Sr25519,
                WriteOptions {
                    overwrite: Overwrite::Yes,
                    ..WriteOptions::hotkey()
                }
            )
            .await,
        Err(WalletError::Symlink(_))
    ));
    assert_eq!(fs::read(&victim).unwrap(), b"preserve");
    fs::remove_file(wallet.hotkey_file().path()).unwrap();
}
#[cfg(not(feature = "encrypted"))]
#[tokio::test]
async fn encrypted_files_require_feature_without_password_resolution_or_plaintext_write() {
    let temp = TempRoot::new();
    let path = temp.path().join("coldkey");
    fs::write(&path, b"$NACLinvalid").unwrap();
    assert!(matches!(
        ColdkeyFile::new(&path).unlock().await,
        Err(WalletError::FeatureRequired("encrypted"))
    ));
    fs::remove_file(&path).unwrap();
    let wallet = Wallet::new("disabled", "worker", temp.path()).unwrap();
    assert!(matches!(
        wallet
            .regenerate_coldkey(uri("//Alice"), CryptoType::Sr25519, WriteOptions::coldkey())
            .await,
        Err(WalletError::FeatureRequired("encrypted"))
    ));
    assert!(!wallet.coldkey_file().path().exists());
}
#[cfg(feature = "encrypted")]
#[tokio::test]
async fn nacl_coldkey_unlock_wrong_password_and_cache_follow_core() {
    let temp = TempRoot::new();
    let wallet = Wallet::new("encrypted", "worker", temp.path()).unwrap();
    let password = SecretString::new("disposable-wallet-password".into());
    wallet
        .regenerate_coldkey(
            uri("//Alice"),
            CryptoType::Sr25519,
            WriteOptions {
                password: Some(password.clone()),
                ..WriteOptions::coldkey()
            },
        )
        .await
        .unwrap();
    assert!(rtao_core::keyfile_json::keyfile_data_is_encrypted_nacl(
        &fs::read(wallet.coldkey_file().path()).unwrap()
    ));
    assert_eq!(wallet.coldkey_address().unwrap(), alice());
    let correct = wallet.clone().with_password_sources(PasswordSources {
        password: Some(password),
        ..PasswordSources::default()
    });
    let first = correct.coldkey_file().unlock().await.unwrap();
    assert_eq!(first.account(), alice());
    assert!(Arc::ptr_eq(
        &first,
        &correct.coldkey_file().unlock().await.unwrap()
    ));
    let wrong = wallet.with_password_sources(PasswordSources {
        password: Some(SecretString::new("wrong-test-password".into())),
        ..PasswordSources::default()
    });
    assert!(matches!(
        wrong.coldkey_file().unlock().await,
        Err(WalletError::WrongPassword {
            source: rtao_wallet::PasswordSource::Explicit
        })
    ));
}
