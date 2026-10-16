// Derived from RaoFoundation/subtensor sdk/python/tests/unit/test_keyfiles_golden.py @ 850f2755bb05a3851f948489f0ab27b0645cebce (Apache-2.0); modified by rtao.
//! Generic native keyfile contracts, using only disposable source fixtures.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod support;

use rtao_core::keys::Keypair;
use rtao_wallet::{
    ExposeSecret, KeyPasswordSources, Keyfile, KeyfileEnvironment, KeyfileEnvironmentOverlay,
    Overwrite, PasswordPrompt, PromptPolicy, SecretString, WalletError, WriteOptions,
    resolve_key_password,
};
use std::{
    collections::{BTreeMap, VecDeque},
    fs,
    sync::{Arc, Mutex},
};
use support::TempRoot;

#[derive(Default)]
struct Environment {
    values: Mutex<BTreeMap<String, SecretString>>,
    calls: Mutex<Vec<String>>,
}
impl Environment {
    fn put_password(&self, name: &str, password: &str) {
        let encoded =
            rtao_core::keyfile_env::encode_password_for_environment(name, password).unwrap();
        self.values
            .lock()
            .unwrap()
            .insert(name.to_owned(), SecretString::new(encoded));
    }
}
impl KeyfileEnvironment for Environment {
    fn get(&self, name: &str) -> Result<Option<SecretString>, WalletError> {
        self.calls.lock().unwrap().push(format!("get {name}"));
        Ok(self.values.lock().unwrap().get(name).cloned())
    }
    fn set(&self, name: &str, value: SecretString) -> Result<(), WalletError> {
        self.calls.lock().unwrap().push(format!("set {name}"));
        self.values.lock().unwrap().insert(name.to_owned(), value);
        Ok(())
    }
    fn remove(&self, name: &str) -> Result<(), WalletError> {
        self.calls.lock().unwrap().push(format!("remove {name}"));
        self.values.lock().unwrap().remove(name);
        Ok(())
    }
}

struct Prompt {
    tty: bool,
    entries: Mutex<VecDeque<SecretString>>,
    messages: Mutex<Vec<String>>,
    diagnostics: Mutex<Vec<&'static str>>,
}
impl Prompt {
    fn new(tty: bool, entries: &[&str]) -> Self {
        Self {
            tty,
            entries: Mutex::new(
                entries
                    .iter()
                    .map(|entry| SecretString::new((*entry).into()))
                    .collect(),
            ),
            messages: Mutex::new(Vec::new()),
            diagnostics: Mutex::new(Vec::new()),
        }
    }
}
impl PasswordPrompt for Prompt {
    fn is_terminal(&self) -> bool {
        self.tty
    }
    fn prompt(&self, message: &str) -> Result<SecretString, WalletError> {
        self.messages.lock().unwrap().push(message.to_owned());
        Ok(self.entries.lock().unwrap().pop_front().unwrap())
    }
    fn invalid_password(&self, message: &'static str) -> Result<(), WalletError> {
        self.diagnostics.lock().unwrap().push(message);
        Ok(())
    }
}

#[tokio::test]
async fn generic_password_order_uses_only_explicit_chosen_environment_and_enabled_prompt() {
    let environment = Arc::new(Environment::default());
    let name = "RTAO_DISPOSABLE_CUSTOM_KEY_PASSWORD";
    environment.put_password(name, "environment-disposable-password");
    // These unrelated wallet variables must not be queried by generic resolution.
    environment.put_password("BT_WALLET_PASSWORD", "wallet-global-sentinel");
    environment.put_password("BT_WALLET_PASSWORD_FILE", "password-file-sentinel");
    let prompt = Arc::new(Prompt::new(false, &["prompt-disposable-password"]));
    let mut sources = KeyPasswordSources {
        password: Some(SecretString::new("explicit-disposable-password".into())),
        env_var_name: Some(name.into()),
        confirm: true,
        prompt: PromptPolicy::Always,
        prompt_callback: Some(prompt.clone()),
        environment: environment.clone(),
        ..Default::default()
    };
    assert_eq!(
        resolve_key_password(&sources)
            .await
            .unwrap()
            .unwrap()
            .expose_secret(),
        "explicit-disposable-password"
    );
    assert!(environment.calls.lock().unwrap().is_empty());
    sources.password = Some(SecretString::new(String::new()));
    assert_eq!(
        resolve_key_password(&sources)
            .await
            .unwrap()
            .unwrap()
            .expose_secret(),
        "environment-disposable-password"
    );
    environment
        .values
        .lock()
        .unwrap()
        .insert(name.into(), SecretString::new("malformed-base64!!!".into()));
    let error = resolve_key_password(&sources).await.unwrap_err();
    assert!(error.to_string().contains("invalid base64"));
    assert!(prompt.messages.lock().unwrap().is_empty());
    environment.put_password(name, "");
    sources.confirm = false;
    assert_eq!(
        resolve_key_password(&sources)
            .await
            .unwrap()
            .unwrap()
            .expose_secret(),
        "prompt-disposable-password"
    );
    assert!(
        environment
            .calls
            .lock()
            .unwrap()
            .iter()
            .all(|call| call == &format!("get {name}"))
    );
    sources.env_var_name = Some(String::new());
    sources.prompt = PromptPolicy::Never;
    assert!(resolve_key_password(&sources).await.unwrap().is_none());
    sources.password = Some(SecretString::new("configuration-secret-sentinel".into()));
    assert!(!format!("{sources:?}").contains("configuration-secret-sentinel"));
}

#[tokio::test]
async fn generic_confirming_prompt_retries_only_on_tty_and_keeps_diagnostics_secret_free() {
    let prompt = Arc::new(Prompt::new(
        true,
        &[
            "",
            "first-secret-sentinel",
            "different-secret-sentinel",
            "confirmed-secret-sentinel",
            "confirmed-secret-sentinel",
        ],
    ));
    let sources = KeyPasswordSources {
        prompt_message: "custom hidden input: ".into(),
        confirm: true,
        prompt: PromptPolicy::IfTty,
        prompt_callback: Some(prompt.clone()),
        ..Default::default()
    };
    assert_eq!(
        resolve_key_password(&sources)
            .await
            .unwrap()
            .unwrap()
            .expose_secret(),
        "confirmed-secret-sentinel"
    );
    assert_eq!(
        *prompt.messages.lock().unwrap(),
        [
            "custom hidden input: ",
            "custom hidden input: ",
            "Retype password: ",
            "custom hidden input: ",
            "Retype password: ",
        ]
    );
    assert_eq!(
        *prompt.diagnostics.lock().unwrap(),
        ["password cannot be empty", "passwords do not match"]
    );
    let nonterminal = Arc::new(Prompt::new(false, &["first", "different"]));
    let mut sources = KeyPasswordSources {
        confirm: true,
        prompt: PromptPolicy::Always,
        prompt_callback: Some(nonterminal.clone()),
        ..Default::default()
    };
    let error = resolve_key_password(&sources).await.unwrap_err();
    assert!(matches!(error, WalletError::PasswordMismatch));
    assert_eq!(error.to_string(), "passwords do not match");
    assert_eq!(nonterminal.messages.lock().unwrap().len(), 2);
    assert!(nonterminal.diagnostics.lock().unwrap().is_empty());
    sources.prompt = PromptPolicy::IfTty;
    assert!(matches!(
        resolve_key_password(&sources).await,
        Err(WalletError::PromptUnavailable)
    ));
    sources.prompt = PromptPolicy::Never;
    assert!(resolve_key_password(&sources).await.unwrap().is_none());
    assert_eq!(nonterminal.messages.lock().unwrap().len(), 2);
    sources.prompt = PromptPolicy::Always;
    sources.prompt_callback = Some(Arc::new(Prompt::new(false, &[""])));
    let error = resolve_key_password(&sources).await.unwrap_err();
    assert!(matches!(error, WalletError::PasswordEmpty));
    assert_eq!(error.to_string(), "password cannot be empty");
}

#[tokio::test]
async fn generic_plaintext_and_public_files_use_exact_codec_without_companion_side_effects() {
    let root = TempRoot::new();
    let path = root.path().join("nested/generic-key");
    let file = Keyfile::new(&path);
    assert_eq!(file.name(), "generic-key");
    assert!(!file.exists_on_device());
    assert!(!file.is_readable());
    assert!(!file.is_writable());
    assert!(!file.is_encrypted().unwrap());
    file.make_dirs().unwrap();
    assert!(file.is_writable());
    let file = file.with_name("explicit-label").with_name("");
    assert_eq!(file.name(), "explicit-label");
    let keypair = Keypair::from_uri("//Alice", 1).unwrap();
    let expected_json: serde_json::Value = serde_json::from_slice(
        &rtao_core::keyfile_json::serialized_keypair_to_keyfile_data(&keypair).unwrap(),
    )
    .unwrap();
    file.set_keypair(
        keypair,
        WriteOptions {
            encrypt: false,
            password: Some(SecretString::new("unused-plaintext-password".into())),
            overwrite: Overwrite::No,
        },
    )
    .await
    .unwrap();
    assert!(file.exists_on_device());
    assert!(file.is_readable());
    assert!(file.is_writable());
    assert!(!file.is_encrypted().unwrap());
    let expected = fs::read(&path).unwrap();
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&expected).unwrap(),
        expected_json
    );
    assert_eq!(fs::read_dir(path.parent().unwrap()).unwrap().count(), 1);
    let restored = file.get_keypair(None).await.unwrap();
    assert_eq!(
        restored.public_key_bytes(),
        Keypair::from_uri("//Alice", 1).unwrap().public_key_bytes()
    );
    assert_eq!(restored.crypto_type(), 1);
    assert_eq!(
        restored.private_key_bytes(),
        Keypair::from_uri("//Alice", 1).unwrap().private_key_bytes()
    );
    let error = file
        .set_keypair(
            Keypair::from_uri("//Bob", 1).unwrap(),
            WriteOptions::hotkey(),
        )
        .await
        .unwrap_err();
    assert!(matches!(error, WalletError::AlreadyExists(_)));
    assert_eq!(fs::read(&path).unwrap(), expected);
    let public = Keypair::new(None, Some(&[7; 32]), 1, 42).unwrap();
    file.set_keypair(
        public,
        WriteOptions {
            overwrite: Overwrite::Yes,
            ..WriteOptions::hotkey()
        },
    )
    .await
    .unwrap();
    let restored = file.get_keypair(None).await.unwrap();
    assert_eq!(restored.public_key_bytes(), [7; 32]);
    assert!(restored.private_key_bytes().is_none());
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

#[cfg(unix)]
#[tokio::test]
async fn generic_overwrite_clamps_loose_file_mode_and_preserves_key_identity()
-> Result<(), Box<dyn std::error::Error>> {
    // pyt: unit/test_keyfiles_golden.py::test_keyfile_written_owner_only
    // The process-wide umask is deliberately untouched. This independently
    // exercises new-directory modes and replacement of an existing 0644 file.
    use std::os::unix::fs::PermissionsExt;
    let root = TempRoot::new();
    let path = root.path().join("wallets/golden/hotkeys/hotkey");
    let file = Keyfile::new(&path);
    let write = WriteOptions {
        encrypt: false,
        password: None,
        overwrite: Overwrite::Yes,
    };
    file.set_keypair(Keypair::from_uri("//Alice", 1)?, write.clone())
        .await?;
    assert_eq!(fs::metadata(&path)?.permissions().mode() & 0o777, 0o600);
    let parent = path.parent().ok_or("keyfile parent")?;
    assert_eq!(fs::metadata(parent)?.permissions().mode() & 0o777, 0o700);
    fs::set_permissions(&path, fs::Permissions::from_mode(0o644))?;
    assert_eq!(fs::metadata(&path)?.permissions().mode() & 0o777, 0o644);
    file.set_keypair(Keypair::from_uri("//Alice", 1)?, write)
        .await?;
    assert_eq!(fs::metadata(&path)?.permissions().mode() & 0o777, 0o600);
    assert_eq!(
        file.get_keypair(None).await?.ss58_address(),
        "5GrwvaEF5zXb26Fz9rcQpDWS57CtERHpNehXCPcNoHGKutQY"
    );
    Ok(())
}

#[tokio::test]
async fn generic_keyfile_rejects_oversize_nonregular_and_symlink_writes() {
    let root = TempRoot::new();
    let path = root.path().join("oversize");
    let descriptor = fs::File::create(&path).unwrap();
    descriptor.set_len(16 * 1024 * 1024 + 1).unwrap();
    drop(descriptor);
    let file = Keyfile::new(&path);
    let error = file.get_keypair(None).await.err().unwrap();
    assert_eq!(
        error.to_string(),
        "keyfile error: keyfile exceeds 16 MiB limit"
    );
    assert!(file.is_encrypted().is_err());
    assert!(Keyfile::new(root.path()).get_keypair(None).await.is_err());
    #[cfg(unix)]
    {
        let link = root.path().join("symlink");
        std::os::unix::fs::symlink(&path, &link).unwrap();
        let linked = Keyfile::new(&link);
        let error = linked
            .set_keypair(
                Keypair::from_uri("//Alice", 1).unwrap(),
                WriteOptions {
                    overwrite: Overwrite::Yes,
                    ..WriteOptions::hotkey()
                },
            )
            .await
            .unwrap_err();
        assert!(matches!(error, WalletError::Symlink(_)));
        assert_eq!(fs::metadata(&path).unwrap().len(), 16 * 1024 * 1024 + 1);
        let actual_parent = root.path().join("actual-parent");
        fs::create_dir_all(actual_parent.join("existing")).unwrap();
        let parent_link = root.path().join("parent-link");
        std::os::unix::fs::symlink(&actual_parent, &parent_link).unwrap();
        let redirected = Keyfile::new(parent_link.join("existing/not-created/key"));
        assert!(matches!(
            redirected.make_dirs(),
            Err(WalletError::Symlink(_))
        ));
        assert!(!actual_parent.join("existing/not-created").exists());
    }
}

#[tokio::test]
async fn generic_empty_path_inspection_uses_current_directory_and_rejects_nonregular_reads() {
    let file = Keyfile::new("");
    let current = Keyfile::new(".");
    assert_eq!(file.path(), std::path::Path::new(""));
    assert_eq!(file.name(), "");
    assert_eq!(file.env_var_name(), "BT_PW_");
    assert!(file.exists_on_device());
    assert_eq!(file.is_readable(), current.is_readable());
    assert_eq!(file.is_writable(), current.is_writable());
    assert!(file.get_keypair(None).await.is_err());
    assert!(file.is_encrypted().is_err());
}

#[tokio::test]
async fn generic_relative_basename_write_finishes_and_keeps_the_exact_env_name() {
    const CHILD_ROOT: &str = "RTAO_GENERIC_KEYFILE_RELATIVE_CHILD";
    if let Some(root) = std::env::var_os(CHILD_ROOT) {
        let root = std::path::PathBuf::from(root);
        assert!(root.is_absolute());
        assert_eq!(
            std::env::current_dir().unwrap(),
            fs::canonicalize(&root).unwrap()
        );
        let file = Keyfile::new("coldkey");
        assert_eq!(file.path(), std::path::Path::new("coldkey"));
        assert_eq!(file.env_var_name(), "BT_PW_COLDKEY");
        assert!(file.is_writable());
        file.make_dirs().unwrap();
        file.set_keypair(
            Keypair::from_uri("//Alice", 1).unwrap(),
            WriteOptions::hotkey(),
        )
        .await
        .unwrap();
        assert_eq!(
            file.get_keypair(None).await.unwrap().public_key_bytes(),
            Keypair::from_uri("//Alice", 1).unwrap().public_key_bytes()
        );
        assert_eq!(fs::read_dir(root).unwrap().count(), 1);
        return;
    }
    // Isolate the relative working directory without mutating this test
    // process's cwd or environment while other fixtures may be running.
    let root = TempRoot::new();
    let output = std::process::Command::new(std::env::current_exe().unwrap())
        .arg("--exact")
        .arg("generic_relative_basename_write_finishes_and_keeps_the_exact_env_name")
        .arg("--nocapture")
        .current_dir(root.path())
        .env(CHILD_ROOT, root.path())
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(root.path().join("coldkey").is_file());
}

#[tokio::test]
async fn generic_environment_save_remove_mutates_only_the_owned_environment() {
    let root = TempRoot::new();
    let environment = Arc::new(Environment::default());
    let file =
        Keyfile::new(root.path().join("cold.key")).with_password_sources(KeyPasswordSources {
            environment: environment.clone(),
            ..Default::default()
        });
    let name = file.env_var_name();
    let before = std::env::var_os(&name);
    assert_eq!(
        file.save_password_to_env(Some(SecretString::new(
            "environment-secret-sentinel".into()
        )))
        .await
        .unwrap(),
        name
    );
    let encoded = environment
        .values
        .lock()
        .unwrap()
        .get(&name)
        .cloned()
        .unwrap();
    assert_eq!(
        rtao_core::keyfile_env::decode_password_from_environment(&name, encoded.expose_secret())
            .unwrap(),
        "environment-secret-sentinel"
    );
    assert!(!file.exists_on_device());
    assert!(std::env::var_os(&name) == before);
    file.clone().remove_password_from_env().unwrap();
    assert!(!environment.values.lock().unwrap().contains_key(&name));
    assert_eq!(
        *environment.calls.lock().unwrap(),
        [format!("set {name}"), format!("remove {name}")]
    );

    // The built-in overlay reads through without mutating global process state.
    let overlay = KeyfileEnvironmentOverlay::default();
    let path_before = std::env::var_os("PATH");
    assert!(overlay.get("PATH").unwrap().is_some() == path_before.is_some());
    overlay
        .set("PATH", SecretString::new("overlay-secret-sentinel".into()))
        .unwrap();
    assert_eq!(
        overlay.get("PATH").unwrap().unwrap().expose_secret(),
        "overlay-secret-sentinel"
    );
    overlay.remove("PATH").unwrap();
    assert!(overlay.get("PATH").unwrap().is_none());
    assert!(std::env::var_os("PATH") == path_before);
    assert!(!format!("{overlay:?}").contains("overlay-secret-sentinel"));
    let invalid_process_name = Keyfile::new("a=b");
    invalid_process_name.remove_password_from_env().unwrap();
    assert!(
        invalid_process_name
            .password_sources()
            .environment
            .get(&invalid_process_name.env_var_name())
            .unwrap()
            .is_none()
    );
}

#[cfg(feature = "encrypted")]
#[tokio::test]
async fn generic_encrypted_write_saves_only_after_success_and_uses_first_password_source() {
    let root = TempRoot::new();
    let environment = Arc::new(Environment::default());
    let prompt = Arc::new(Prompt::new(false, &[]));
    let file = Keyfile::new(root.path().join("coldkey"))
        .with_save_to_env(true)
        .with_password_sources(KeyPasswordSources {
            prompt: PromptPolicy::Always,
            prompt_callback: Some(prompt.clone()),
            environment: environment.clone(),
            ..Default::default()
        });
    let password = "generic-encryption-disposable-password";
    // Environment lookup is not a password source for encrypting a new file.
    environment.put_password(&file.env_var_name(), "stale-unrelated-password");
    fs::write(file.path(), b"preserve-existing-content").unwrap();
    let error = file
        .set_keypair(
            Keypair::from_uri("//Alice", 1).unwrap(),
            WriteOptions {
                password: Some(SecretString::new(password.into())),
                ..WriteOptions::coldkey()
            },
        )
        .await
        .unwrap_err();
    assert!(matches!(error, WalletError::AlreadyExists(_)));
    assert_eq!(fs::read(file.path()).unwrap(), b"preserve-existing-content");
    assert!(environment.calls.lock().unwrap().is_empty());
    file.set_keypair(
        Keypair::from_uri("//Alice", 1).unwrap(),
        WriteOptions {
            password: Some(SecretString::new(password.into())),
            overwrite: Overwrite::Yes,
            ..WriteOptions::coldkey()
        },
    )
    .await
    .unwrap();
    assert!(file.should_save_to_env());
    assert!(file.is_encrypted().unwrap());
    // pyt: unit/test_keyfiles_golden.py::test_nacl_encrypt_decrypt_roundtrip
    assert!(rtao_core::keyfile_json::keyfile_data_is_encrypted_nacl(
        &fs::read(file.path()).unwrap()
    ));
    assert_eq!(
        *environment.calls.lock().unwrap(),
        [format!("set {}", file.env_var_name())]
    );
    let restored = file.clone().get_keypair(None).await.unwrap();
    assert_eq!(
        restored.ss58_address(),
        "5GrwvaEF5zXb26Fz9rcQpDWS57CtERHpNehXCPcNoHGKutQY"
    );
    let signature = restored.sign(b"after-decrypt").unwrap();
    assert_eq!(signature.len(), 64);
    assert!(restored.verify(b"after-decrypt", &signature).unwrap());
    assert_eq!(
        restored.public_key_bytes(),
        Keypair::from_uri("//Alice", 1).unwrap().public_key_bytes()
    );
    let error = file
        .get_keypair(Some(SecretString::new("wrong-password-sentinel".into())))
        .await
        .err()
        .unwrap();
    assert!(matches!(
        error,
        WalletError::WrongPassword {
            source: rtao_wallet::PasswordSource::Explicit
        }
    ));
    assert!(!error.to_string().contains("wrong-password-sentinel"));
    assert!(!format!("{file:?}").contains(password));
    assert!(prompt.messages.lock().unwrap().is_empty());
    assert_eq!(fs::read_dir(root.path()).unwrap().count(), 1);
    file.remove_password_from_env().unwrap();
    let unlock_prompt = Arc::new(Prompt::new(false, &[password]));
    let file = file.with_password_sources(KeyPasswordSources {
        confirm: true,
        prompt: PromptPolicy::Always,
        prompt_callback: Some(unlock_prompt.clone()),
        environment,
        ..Default::default()
    });
    // Unlocking never confirms, even if the caller's generic policy was
    // configured for an encryption/password-creation prompt.
    assert_eq!(
        file.get_keypair(None).await.unwrap().public_key_bytes(),
        Keypair::from_uri("//Alice", 1).unwrap().public_key_bytes()
    );
    assert_eq!(
        *unlock_prompt.messages.lock().unwrap(),
        [format!(
            "Enter password to unlock key {}: ",
            file.path().display()
        )]
    );
    assert!(unlock_prompt.diagnostics.lock().unwrap().is_empty());
}

#[cfg(not(feature = "encrypted"))]
#[tokio::test]
async fn generic_encryption_feature_refusal_preserves_existing_bytes_and_environment() {
    let root = TempRoot::new();
    let environment = Arc::new(Environment::default());
    let file = Keyfile::new(root.path().join("coldkey"))
        .with_save_to_env(true)
        .with_password_sources(KeyPasswordSources {
            environment: environment.clone(),
            ..Default::default()
        });
    fs::write(file.path(), b"$NACLinvalid-disposable-fixture").unwrap();
    assert!(file.is_encrypted().unwrap());
    assert!(matches!(
        file.get_keypair(None).await,
        Err(WalletError::FeatureRequired("encrypted"))
    ));
    assert!(matches!(
        file.set_keypair(
            Keypair::from_uri("//Alice", 1).unwrap(),
            WriteOptions::coldkey()
        )
        .await,
        Err(WalletError::FeatureRequired("encrypted"))
    ));
    assert_eq!(
        fs::read(file.path()).unwrap(),
        b"$NACLinvalid-disposable-fixture"
    );
    assert!(environment.calls.lock().unwrap().is_empty());
}
