#![allow(clippy::unwrap_used, clippy::expect_used)]

use super::*;
use std::{
    fs::File,
    sync::atomic::{AtomicUsize, Ordering},
};

const PRIVATE_ONE: &str = "0000000000000000000000000000000000000000000000000000000000000001";
const ADDRESS_ONE: &str = "0x7e5f4552091a69125d5dfcb7b8c2659029395bdf";

// Published independent Ethereum V3 test vectors:
// ethereum/tests, KeyStoreTests/basic_tests.json, test1/test2/evilnonce/mycrypto.
// The encrypted bytes, MACs and expected scalars are copied verbatim.
fn vectors() -> [(Value, &'static str, &'static str); 4] {
    [
        (
            json!({"version":3,"crypto":{
                "cipher":"aes-128-ctr","cipherparams":{"iv":"6087dab2f9fdbbfaddc31a909735c1e6"},
                "ciphertext":"5318b4d5bcd28de64ee5559e671353e16f075ecae9f99c7a79a38af5f869aa46",
                "kdf":"pbkdf2","kdfparams":{"c":262144,"dklen":32,"prf":"hmac-sha256","salt":"ae3cd4e7013836a3df6bd7241b12db061dbe2c6785853cce422d148a624ce0bd"},
                "mac":"517ead924a9d0dc3124507e3393d175ce3ff7c1e96529c6c555ce9e51205e9b2"
            }}),
            "testpassword",
            "7a28b5ba57c53603b0b07b56bba752f7784bf506fa95edc395f5cf6c7514fe9d",
        ),
        (
            json!({"version":3,"crypto":{
                "cipher":"aes-128-ctr","cipherparams":{"iv":"83dbcc02d8ccb40e466191a123791e0e"},
                "ciphertext":"d172bf743a674da9cdad04534d56926ef8358534d458fffccd4e6ad2fbde479c",
                "kdf":"scrypt","kdfparams":{"n":262144,"r":1,"p":8,"dklen":32,"salt":"ab0c7876052600dd703518d6fc3fe8984592145b591fc8fb5c6d43190334ba19"},
                "mac":"2103ac29920d71da29f15d75b4a16dbe95cfd7ff8faea1056c33131d846e3097"
            }}),
            "testpassword",
            "7a28b5ba57c53603b0b07b56bba752f7784bf506fa95edc395f5cf6c7514fe9d",
        ),
        (
            json!({"version":3,"crypto":{
                "cipher":"aes-128-ctr","cipherparams":{"iv":"ffffffffffffffffffffffffffffffff"},
                "ciphertext":"d69313b6470ac1942f75d72ebf8818a0d484ac78478a132ee081cd954d6bd7a9",
                "kdf":"pbkdf2","kdfparams":{"c":262144,"dklen":32,"prf":"hmac-sha256","salt":"c82ef14476014cbf438081a42709e2ed"},
                "mac":"cf6bfbcc77142a22c4a908784b4a16f1023a1d0e2aff404c20158fa4f1587177"
            }}),
            "bar",
            "0202020202020202020202020202020202020202020202020202020202020202",
        ),
        (
            json!({"version":3,"address":"460121576cc7df020759730751f92bd62fd78dd6","crypto":{
                "cipher":"aes-128-ctr","cipherparams":{"iv":"681679cdb125bba9495d068b002816a4"},
                "ciphertext":"54ae683c6287fa3d58321f09d56e26d94e58a00d4f90bdd95782ae0e4aab618b",
                "kdf":"scrypt","kdfparams":{"n":8192,"r":8,"p":1,"dklen":32,"salt":"c3407f363fce02a66e3c4bf4a8f6b7da1c1f54266cef66381f0625c251c32785"},
                "mac":"dea6bdf22a2f522166ed82808c22a6311e84c355f4bbe100d4260483ff675a46"
            }}),
            "foobartest121",
            "05a4d3eb46c742cb8850440145ce70cbc80b59f891cf5f50fd3e9c280b50c4e4",
        ),
    ]
}
fn account_one() -> EvmAccount {
    account_from_private_key(PRIVATE_ONE).unwrap()
}
fn cheap_document() -> Value {
    encrypt_keystore(&account_one(), "test-password", 1024).unwrap()
}

#[test]
fn decrypts_independent_ethereum_v3_scrypt_pbkdf2_and_wrapping_iv_vectors() {
    for (document, password, expected) in vectors() {
        let account = decrypt_keystore(&document, password).unwrap();
        assert_eq!(hex::encode(account.to_field_bytes()), expected);
    }
}

#[test]
fn private_key_one_and_bip44_mnemonic_have_independent_known_addresses() {
    assert_eq!(account_address(&account_one()), ADDRESS_ONE);
    assert_eq!(
        account_address(&account_from_private_key(&format!(" 0x{PRIVATE_ONE} \n")).unwrap()),
        ADDRESS_ONE
    );
    let phrase = "test test test test test test test test test test test junk";
    assert_eq!(
        account_address(&account_from_mnemonic(phrase, ETH_DERIVATION_PATH).unwrap()),
        "0xf39fd6e51aad88f6f4ce6ab8827279cfffb92266"
    );
    assert_eq!(
        account_address(&account_from_mnemonic(phrase, "m/44'/60'/0'/0/1").unwrap()),
        "0x70997970c51812dc3a010c7d01b50e0d17dc79c8"
    );
}

#[test]
fn japanese_mnemonics_normalize_composed_characters_and_ideographic_separators() {
    // The BIP-39 zero-entropy Japanese phrase has the same word indices as
    // eleven English `abandon` words followed by `about` (index 3).
    let words = ["あいこくしん"; 11].join(" ");
    let canonical = format!("{words} あおそ\u{3099}ら");
    let composed = format!("{words} あおぞら").replace(' ', "\u{3000}");
    let first = account_from_mnemonic(&canonical, ETH_DERIVATION_PATH).unwrap();
    let second = account_from_mnemonic(&composed, ETH_DERIVATION_PATH).unwrap();
    assert_eq!(first.address(), second.address());
    let english = account_from_mnemonic("abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about", ETH_DERIVATION_PATH).unwrap();
    assert_ne!(first.address(), english.address());
}

#[test]
fn invalid_recovery_inputs_have_fixed_diagnostics_without_secrets() {
    for value in ["00", &"00".repeat(32), &"ff".repeat(32), &"zz".repeat(32)] {
        assert!(account_from_private_key(value).is_err());
    }
    let phrase = "unique-secret-phrase that must never appear in errors";
    let message = account_from_mnemonic(phrase, ETH_DERIVATION_PATH)
        .unwrap_err()
        .to_string();
    assert!(!message.contains("unique-secret"));
    assert!(
        account_from_mnemonic(
            "test test test test test test test test test test test junk",
            "not-a-path"
        )
        .is_err()
    );
    assert!(account_from_mnemonic(&"x".repeat(4097), ETH_DERIVATION_PATH).is_err());
}

#[test]
fn mnemonic_paths_validate_indices_before_upstream_arithmetic_and_normalize_python_notation() {
    let phrase = "test test test test test test test test test test test junk";
    let expected = account_from_mnemonic(phrase, ETH_DERIVATION_PATH).unwrap();
    assert_eq!(
        account_from_mnemonic(phrase, "M/44H/60h/0'/0/0")
            .unwrap()
            .address(),
        expected.address()
    );
    assert!(account_from_mnemonic(phrase, "m/2147483647'").is_ok());
    for path in [
        "m/2147483648",
        "m/2147483648'",
        "m/4294967295'",
        "m/4294967296'",
        "m/999999999999999999999999999999",
        "m/m/0",
        "M/M/0",
        "m//0",
        "m/0/",
        "m/-1",
        "m/+1",
        "m/0hh",
        "n/0",
    ] {
        assert!(
            account_from_mnemonic(phrase, path).is_err(),
            "accepted {path}"
        );
    }
}

#[test]
fn kdf_limits_reject_work_memory_overflows_and_unsupported_algorithms_before_deriving() {
    let source = cheap_document();
    for (name, value) in [
        ("n", json!(3)),
        ("n", json!(0)),
        ("n", json!(u64::MAX)),
        ("n", json!(1u64 << 31)),
        ("r", json!(0)),
        ("r", json!(u64::MAX)),
        ("p", json!(0)),
        ("p", json!(u64::MAX)),
        ("dklen", json!(u64::MAX)),
        ("salt", json!("")),
        ("salt", json!("ff".repeat(1025))),
    ] {
        let mut document = source.clone();
        document["crypto"]["kdfparams"][name] = value;
        assert!(validate_keystore(&document).is_err(), "accepted {name}");
    }
    let mut over_work = source.clone();
    over_work["crypto"]["kdfparams"]["n"] = json!(1u64 << 18);
    over_work["crypto"]["kdfparams"]["p"] = json!(16);
    assert!(validate_keystore(&over_work).is_err());
    for rounds in [0, MAX_PBKDF2_ROUNDS + 1, u64::MAX] {
        let mut document = vectors()[0].0.clone();
        document["crypto"]["kdfparams"]["c"] = json!(rounds);
        assert!(validate_keystore(&document).is_err());
    }
    let mut document = source;
    document["crypto"]["kdf"] = json!("argon2");
    assert!(validate_keystore(&document).is_err());
}

#[test]
fn authentication_failures_and_misleading_address_metadata_are_refused() {
    let document = cheap_document();
    let mut longer_derived_key = document.clone();
    longer_derived_key["crypto"]["kdfparams"]["dklen"] = json!(64);
    assert_eq!(
        account_address(&decrypt_keystore(&longer_derived_key, "test-password").unwrap()),
        ADDRESS_ONE
    );
    assert!(decrypt_keystore(&document, "wrong-password").is_err());
    let mut tampered = document.clone();
    tampered["crypto"]["ciphertext"] = json!("00".repeat(32));
    assert!(decrypt_keystore(&tampered, "test-password").is_err());
    tampered = document;
    tampered["address"] = json!("00".repeat(20));
    assert!(
        decrypt_keystore(&tampered, "test-password")
            .unwrap_err()
            .to_string()
            .contains("does not match")
    );
}

#[test]
fn empty_explicit_password_falls_through_environment_then_explicit_resolver() {
    let calls = AtomicUsize::new(0);
    let resolver = |confirm: bool| {
        assert!(confirm);
        calls.fetch_add(1, Ordering::SeqCst);
        Ok(Zeroizing::new("resolved-secret".to_owned()))
    };
    let mut options = EvmKeyOptions {
        password: Some(Zeroizing::new("explicit-secret".into())),
        ..Default::default()
    };
    let password = resolve_password_with(&options, Some(&resolver), true, || {
        Some("environment-secret".into())
    })
    .unwrap();
    assert_eq!(&*password, "explicit-secret");
    options.password = Some(Zeroizing::new(String::new()));
    let password = resolve_password_with(&options, Some(&resolver), true, || {
        Some("environment-secret".into())
    })
    .unwrap();
    assert_eq!(&*password, "environment-secret");
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    let password =
        resolve_password_with(&options, Some(&resolver), true, || Some(String::new())).unwrap();
    assert_eq!(&*password, "resolved-secret");
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert!(resolve_password_with(&options, None, false, || None).is_err());
    assert!(!format!("{options:?}").contains("explicit-secret"));
    for source in [
        ImportSource::PrivateKey("private-secret"),
        ImportSource::Mnemonic {
            phrase: "mnemonic-secret",
            derivation_path: None,
        },
        ImportSource::Keystore {
            json: "json-secret",
            password: Some("password-secret"),
        },
    ] {
        assert!(!format!("{source:?}").contains("-secret"));
    }
}

#[test]
fn paths_reject_escape_and_list_sorted_metadata_without_unlocking() {
    let root = tempfile::tempdir().unwrap();
    for name in [
        "",
        ".",
        "..",
        "/tmp/outside",
        "../escape",
        "a/b",
        "a\\b",
        "a\0b",
    ] {
        assert!(keyfile_path(name, "wallet", root.path()).is_err());
        assert!(keyfile_path("key", name, root.path()).is_err());
    }
    assert!(list_evm_keys("missing", root.path()).unwrap().is_empty());
    let document = cheap_document();
    let a = keyfile_path("a", "wallet", root.path()).unwrap();
    let z = keyfile_path("z", "wallet", root.path()).unwrap();
    write_keystore_file(&z, &document, false).unwrap();
    write_keystore_file(&a, &document, false).unwrap();
    fs::write(a.parent().unwrap().join("broken"), b"{broken").unwrap();
    fs::create_dir(a.parent().unwrap().join("directory")).unwrap();
    let infos = list_evm_keys("wallet", root.path()).unwrap();
    assert_eq!(
        infos
            .iter()
            .map(|info| info.name.as_str())
            .collect::<Vec<_>>(),
        ["a", "z"]
    );
    assert!(infos.iter().all(|info| info.address == ADDRESS_ONE));
    assert_eq!(infos[0].ss58_mirror, h160_to_ss58(ADDRESS_ONE).unwrap());
    assert!(
        get_evm_key_info("missing", "wallet", root.path())
            .unwrap_err()
            .to_string()
            .contains("Available: a, z")
    );
    assert_eq!(
        export_evm_key("a", "wallet", root.path()).unwrap(),
        document
    );
}

#[test]
fn atomic_write_never_clobbers_without_permission_and_removes_temporary_files() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("nested/evmkeys/account");
    let original = cheap_document();
    write_keystore_file(&path, &original, false).unwrap();
    let original_bytes = fs::read(&path).unwrap();
    let mut replacement = original.clone();
    replacement["id"] = json!("new-public-metadata");
    assert!(write_keystore_file(&path, &replacement, false).is_err());
    assert_eq!(fs::read(&path).unwrap(), original_bytes);
    write_keystore_file(&path, &replacement, true).unwrap();
    assert_eq!(read_document(&path).unwrap(), replacement);
    assert_eq!(fs::read_dir(path.parent().unwrap()).unwrap().count(), 1);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
        assert_eq!(
            fs::metadata(path.parent().unwrap())
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o700
        );
    }
}

#[test]
fn concurrent_nonoverwrite_writers_publish_exactly_one_complete_keystore() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("account");
    let first = cheap_document();
    let mut second = first.clone();
    second["id"] = json!("second-writer");
    let barrier = std::sync::Arc::new(std::sync::Barrier::new(2));
    let spawn_writer = |document: Value| {
        let path = path.clone();
        let barrier = barrier.clone();
        std::thread::spawn(move || {
            barrier.wait();
            write_keystore_file(&path, &document, false)
        })
    };
    let left = spawn_writer(first.clone());
    let right = spawn_writer(second.clone());
    let successes =
        usize::from(left.join().unwrap().is_ok()) + usize::from(right.join().unwrap().is_ok());
    assert_eq!(successes, 1);
    let published = read_document(&path).unwrap();
    assert!(published == first || published == second);
    assert_eq!(fs::read_dir(root.path()).unwrap().count(), 1);
}

#[cfg(unix)]
#[test]
fn writes_refuse_destination_and_ancestor_symlinks_without_touching_targets() {
    use std::os::unix::fs::symlink;
    let root = tempfile::tempdir().unwrap();
    let outside = root.path().join("outside");
    fs::write(&outside, b"preserved").unwrap();
    let link = root.path().join("linked-key");
    symlink(&outside, &link).unwrap();
    assert!(write_keystore_file(&link, &cheap_document(), true).is_err());
    assert_eq!(fs::read(&outside).unwrap(), b"preserved");
    let real = root.path().join("real");
    fs::create_dir(&real).unwrap();
    let alias = root.path().join("alias");
    symlink(&real, &alias).unwrap();
    assert!(write_keystore_file(&alias.join("nested/key"), &cheap_document(), false).is_err());
    assert!(fs::read_dir(&real).unwrap().next().is_none());
}

#[test]
fn sparse_oversized_keyfiles_and_inline_sources_are_refused() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("large");
    File::create(&path)
        .unwrap()
        .set_len(MAX_KEYSTORE_BYTES as u64 + 1)
        .unwrap();
    assert!(read_document(&path).is_err());
    assert!(parse_keystore(&vec![b' '; MAX_KEYSTORE_BYTES + 1]).is_err());
    let mut too_large = cheap_document();
    too_large["unknown_metadata"] = json!("x".repeat(MAX_KEYSTORE_BYTES));
    let destination = root.path().join("bounded-output");
    assert!(write_keystore_file(&destination, &too_large, false).is_err());
    assert!(!destination.exists());
}

#[test]
fn public_import_encrypts_and_unlocks_without_exposing_plaintext_on_disk() {
    let root = tempfile::tempdir().unwrap();
    let options = EvmKeyOptions {
        wallet_path: root.path().into(),
        password: Some(Zeroizing::new("storage-password".into())),
        ..Default::default()
    };
    let info = import_evm_key(ImportSource::PrivateKey(PRIVATE_ONE), &options, None).unwrap();
    assert_eq!(info.address, ADDRESS_ONE);
    let encrypted = fs::read_to_string(&info.path).unwrap();
    assert!(!encrypted.contains(PRIVATE_ONE));
    assert!(!encrypted.contains("storage-password"));
    let account = unlock_evm_key(&options, None).unwrap();
    assert_eq!(account_address(&account), ADDRESS_ONE);
    assert!(!format!("{account:?}").contains(PRIVATE_ONE));
    assert!(import_evm_key(ImportSource::PrivateKey(PRIVATE_ONE), &options, None).is_err());
}

#[test]
fn public_creation_stores_fresh_independent_encrypted_accounts_and_preserves_native_keys() {
    let root = tempfile::tempdir().unwrap();
    let wallet = root.path().join("native-wallet");
    fs::create_dir_all(wallet.join("hotkeys")).unwrap();
    let coldkey = wallet.join("coldkey");
    let hotkey = wallet.join("hotkeys/validator");
    // These deliberately are not decodable native keys: EVM creation must
    // neither read native secret material nor depend on it being unlocked.
    fs::write(&coldkey, b"native-coldkey-preserved").unwrap();
    fs::write(&hotkey, b"native-hotkey-preserved").unwrap();
    let options = EvmKeyOptions {
        name: "generated".into(),
        wallet_name: "native-wallet".into(),
        wallet_path: root.path().into(),
        password: Some(Zeroizing::new("creation-password".into())),
        ..Default::default()
    };
    let info = create_evm_key(&options, None).unwrap();
    assert_eq!(info.name, "generated");
    assert_eq!(info.path, wallet.join("evmkeys/generated"));
    assert_eq!(info.address.len(), 42);
    assert_eq!(info.ss58_mirror, h160_to_ss58(&info.address).unwrap());
    assert_eq!(
        get_evm_key_info(&options.name, &options.wallet_name, &options.wallet_path).unwrap(),
        info
    );
    let encrypted_bytes = fs::read(&info.path).unwrap();
    let encrypted_text = std::str::from_utf8(&encrypted_bytes).unwrap();
    let document: Value = serde_json::from_slice(&encrypted_bytes).unwrap();
    assert_eq!(document["version"], 3);
    assert_eq!(document["crypto"]["kdf"], "scrypt");
    assert_eq!(document["crypto"]["kdfparams"]["n"], 1u64 << 18);
    let account = unlock_evm_key(&options, None).unwrap();
    assert_eq!(account_address(&account), info.address);
    assert!(!encrypted_text.contains(&hex::encode(account.to_field_bytes())));
    assert!(!encrypted_text.contains("creation-password"));
    assert!(!encrypted_text.contains("native-coldkey-preserved"));

    // A refused second creation must leave the first account intact.
    assert!(create_evm_key(&options, None).is_err());
    assert_eq!(fs::read(&info.path).unwrap(), encrypted_bytes);
    let second = create_evm_key(
        &EvmKeyOptions {
            name: "generated-2".into(),
            ..options.clone()
        },
        None,
    )
    .unwrap();
    assert_ne!(second.address, info.address);
    assert_ne!(second.path, info.path);
    assert_eq!(
        list_evm_keys(&options.wallet_name, &options.wallet_path)
            .unwrap()
            .len(),
        2
    );
    assert_eq!(fs::read(&coldkey).unwrap(), b"native-coldkey-preserved");
    assert_eq!(fs::read(&hotkey).unwrap(), b"native-hotkey-preserved");
    assert_eq!(fs::read_dir(wallet.join("evmkeys")).unwrap().count(), 2);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            fs::metadata(&info.path).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }
}

#[test]
fn imported_keystore_authentication_precedes_any_destination_write() {
    let root = tempfile::tempdir().unwrap();
    let options = EvmKeyOptions {
        wallet_path: root.path().into(),
        password: Some(Zeroizing::new("new-password".into())),
        ..Default::default()
    };
    let document = cheap_document().to_string();
    assert!(
        import_evm_key(
            ImportSource::Keystore {
                json: &document,
                password: Some("incorrect")
            },
            &options,
            None
        )
        .is_err()
    );
    assert!(fs::read_dir(root.path()).unwrap().next().is_none());
}

#[test]
fn importing_keystore_uses_distinct_unlock_and_storage_passwords() {
    let root = tempfile::tempdir().unwrap();
    let options = EvmKeyOptions {
        wallet_path: root.path().into(),
        password: Some(Zeroizing::new("new-password".into())),
        ..Default::default()
    };
    let original = cheap_document();
    let original_json = original.to_string();
    let info = import_evm_key(
        ImportSource::Keystore {
            json: &original_json,
            password: Some("test-password"),
        },
        &options,
        None,
    )
    .unwrap();
    assert_eq!(info.address, ADDRESS_ONE);
    let exported =
        export_evm_key(&options.name, &options.wallet_name, &options.wallet_path).unwrap();
    assert_ne!(exported["crypto"], original["crypto"]);
    assert_eq!(
        account_address(&unlock_evm_key(&options, None).unwrap()),
        ADDRESS_ONE
    );
}
