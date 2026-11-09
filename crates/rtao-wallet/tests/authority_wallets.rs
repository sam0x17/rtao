// Derived from RaoFoundation/subtensor sdk/python/bittensor/wallet.py @ b6a81d144078eac18bd25c3af7cb31579bb670f3 (Apache-2.0); modified by rtao.
// Derived from RaoFoundation/subtensor sdk/python/bittensor/signing.py @ b6a81d144078eac18bd25c3af7cb31579bb670f3 (Apache-2.0); modified by rtao.
//! Hashed and post-quantum wallets: creation, public companions, unlock into
//! authority signers and the public KeyView hooks (D-150).
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod support;

use rtao_primitives::AuthorityType;
use rtao_signer::{AuthoritySigner, CryptoType, KeyView, Signer, SignerRole};
use rtao_wallet::{
    ExposeSecret, KeyScheme, Overwrite, Wallet, WriteOptions, local_descriptor, public_descriptor,
};
use support::TempRoot;

fn plain(options: WriteOptions) -> WriteOptions {
    WriteOptions {
        encrypt: false,
        password: None,
        ..options
    }
}

#[tokio::test]
async fn hashed_wallets_publish_descriptors_and_unlock_into_authority_signers() {
    let temp = TempRoot::new();
    let wallet = Wallet::new("hashed", "worker", temp.path()).unwrap();
    let cold = wallet
        .create_new_coldkey_as(
            None,
            KeyScheme::Authority(AuthorityType::HashedSr25519),
            plain(WriteOptions::coldkey()),
        )
        .await
        .unwrap();
    assert_eq!(
        cold.scheme,
        KeyScheme::Authority(AuthorityType::HashedSr25519)
    );
    let hot = wallet
        .create_new_hotkey_as(
            None,
            KeyScheme::Classical(CryptoType::Sr25519),
            plain(WriteOptions::hotkey()),
        )
        .await
        .unwrap();
    let info = wallet.public_info(SignerRole::Coldkey).unwrap();
    assert_eq!(info.account, cold.account);
    assert_eq!(info.authority, Some(AuthorityType::HashedSr25519));
    let descriptor = public_descriptor(&wallet.reference().coldkeypub_path())
        .unwrap()
        .expect("descriptor");
    assert_eq!(
        rtao_primitives::receiving::descriptor_account(&descriptor).unwrap(),
        cold.account
    );
    // Public hooks, without unlocking.
    assert_eq!(
        wallet.authority_descriptor(SignerRole::Coldkey),
        Some(descriptor.clone())
    );
    assert_eq!(wallet.authority_descriptor(SignerRole::Hotkey), None);
    assert_eq!(wallet.hotkey_descriptor(), None);
    assert_eq!(
        wallet.local_descriptor(&cold.account),
        Some(descriptor.clone())
    );
    assert_eq!(wallet.local_descriptor(&hot.account), None);
    assert_eq!(
        local_descriptor(temp.path(), &cold.account),
        Some(descriptor.clone())
    );
    // The unlocked key is an authority signer for the same public identity.
    let signer = wallet.unlock_coldkey().await.unwrap();
    assert_eq!(signer.account(), cold.account);
    let authority = signer.authority().expect("authority signer");
    assert_eq!(authority.authority_type(), AuthorityType::HashedSr25519);
    assert_eq!(authority.descriptor(), descriptor.as_slice());
    let hot_signer = wallet.unlock_hotkey().await.unwrap();
    assert!(hot_signer.authority().is_none());
}

#[tokio::test]
async fn post_quantum_wallets_need_24_words_and_stateless_keys_need_no_registration() {
    let temp = TempRoot::new();
    let wallet = Wallet::new("pq", "worker", temp.path()).unwrap();
    let scheme = KeyScheme::Authority(AuthorityType::MlDsa65Fixed);
    assert!(
        wallet
            .create_new_coldkey_as(Some(12), scheme, plain(WriteOptions::coldkey()))
            .await
            .is_err()
    );
    let created = wallet
        .create_new_hotkey_as(None, scheme, plain(WriteOptions::hotkey()))
        .await
        .unwrap();
    assert_eq!(
        created.mnemonic.expose_secret().split_whitespace().count(),
        24,
        "post-quantum phrases default to 24 words"
    );
    // A standard post-quantum hotkey signs but needs no registration.
    assert!(wallet.authority_descriptor(SignerRole::Hotkey).is_some());
    assert_eq!(wallet.hotkey_descriptor(), None);
    assert_eq!(wallet.local_descriptor(&created.account), None);
    let signer = wallet.unlock_hotkey().await.unwrap();
    assert_eq!(
        signer.authority().map(AuthoritySigner::authority_type),
        Some(AuthorityType::MlDsa65Fixed)
    );
    // Account types compose with the scheme as Python's account_crypto_type.
    assert_eq!(
        KeyScheme::with_account_type(1, Some("hashed")).unwrap(),
        KeyScheme::Authority(AuthorityType::HashedSr25519)
    );
    assert_eq!(
        KeyScheme::with_account_type(4, Some("standard")).unwrap(),
        KeyScheme::Classical(CryptoType::Sr25519)
    );
    assert!(KeyScheme::with_account_type(1, Some("other")).is_err());
}

#[tokio::test]
async fn watch_only_companions_restore_from_receiving_addresses_only() {
    let temp = TempRoot::new();
    let source = Wallet::new("source", "worker", temp.path()).unwrap();
    let created = source
        .create_new_coldkey_as(
            None,
            KeyScheme::Authority(AuthorityType::HashedEd25519),
            plain(WriteOptions::coldkey()),
        )
        .await
        .unwrap();
    let descriptor = public_descriptor(&source.reference().coldkeypub_path())
        .unwrap()
        .unwrap();
    let address = rtao_core::keys::encode_hashed_receiving_address(&descriptor).unwrap();
    let watch = Wallet::new("watch", "worker", temp.path()).unwrap();
    let account = watch
        .regenerate_coldkeypub_receiving(&address, Overwrite::No)
        .await
        .unwrap();
    assert_eq!(account, created.account);
    let info = watch.public_info(SignerRole::Coldkey).unwrap();
    assert_eq!(
        (info.account, info.authority),
        (created.account, Some(AuthorityType::HashedEd25519))
    );
    assert!(
        watch
            .regenerate_coldkeypub_receiving(&created.account.to_string(), Overwrite::Yes)
            .await
            .unwrap_err()
            .to_string()
            .contains("complete receiving address")
    );
}
