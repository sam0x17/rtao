// Derived from RaoFoundation/subtensor sdk/python/bittensor/wallets.py and keyfiles.py @ 850f2755bb05a3851f948489f0ab27b0645cebce (Apache-2.0); modified by rtao.
//! Password precedence, with all interaction supplied by the calling CLI.

use crate::{Wallet, WalletError, io_error};
use secrecy::{ExposeSecret, SecretString};
use std::{
    fmt,
    io::IsTerminal,
    path::{Path, PathBuf},
    sync::Arc,
};
use zeroize::Zeroizing;

/// Whether a caller-provided password prompt may run.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum PromptPolicy {
    /// Never prompt; the library default.
    #[default]
    Never,
    /// Permit the CLI callback only when stdin is a terminal.
    IfTty,
    /// Permit the explicitly supplied callback without a terminal check.
    Always,
}
/// The first source chosen by the password resolver.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PasswordSource {
    /// An explicit SDK argument.
    Explicit,
    /// `BT_WALLET_PASSWORD`.
    WalletEnvironment,
    /// The encoded `BT_PW_...` keyfile variable.
    KeyfileEnvironment,
    /// An explicit file or `BT_WALLET_PASSWORD_FILE`.
    PasswordFile,
    /// A callback supplied by the CLI.
    Prompt,
}
impl fmt::Display for PasswordSource {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Explicit => "explicit argument",
            Self::WalletEnvironment => "BT_WALLET_PASSWORD",
            Self::KeyfileEnvironment => "keyfile environment",
            Self::PasswordFile => "password file",
            Self::Prompt => "prompt",
        })
    }
}
/// A hidden-entry callback implemented by a CLI; the library reads no input.
pub trait PasswordPrompt: Send + Sync {
    /// Ask the user for a secret; do not print or log the returned value.
    fn prompt(&self, message: &str) -> Result<SecretString, WalletError>;
}
/// Password inputs in Python precedence order, with library prompting disabled.
#[derive(Clone, Default)]
pub struct PasswordSources {
    /// Explicit input; an empty secret falls through as in Python.
    pub password: Option<SecretString>,
    /// A password file; its entire text is trimmed, matching Python's code.
    pub password_file: Option<PathBuf>,
    /// Request the macOS Keychain source, implemented in M4.
    pub keychain: bool,
    /// Request the native macOS dialog, implemented in M4.
    pub macos_prompt: bool,
    /// Permission for an explicitly supplied CLI callback.
    pub prompt: PromptPolicy,
    /// The caller's hidden password-entry implementation.
    pub prompt_callback: Option<Arc<dyn PasswordPrompt>>,
}
impl fmt::Debug for PasswordSources {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PasswordSources")
            .field("password", &self.password.as_ref().map(|_| "[REDACTED]"))
            .field("password_file", &self.password_file)
            .field("keychain", &self.keychain)
            .field("macos_prompt", &self.macos_prompt)
            .field("prompt", &self.prompt)
            .field(
                "prompt_callback",
                &self.prompt_callback.as_ref().map(|_| "configured"),
            )
            .finish()
    }
}
pub(crate) struct ResolvedPassword {
    pub secret: SecretString,
    #[cfg_attr(not(feature = "encrypted"), allow(dead_code))]
    pub source: PasswordSource,
}

/// Btcli's environment variable name for a keyfile's expanded path.
pub fn env_var_name(path: &Path) -> String {
    format!(
        "BT_PW_{}",
        path.to_string_lossy()
            .replace([std::path::MAIN_SEPARATOR, '.'], "_")
            .to_uppercase()
    )
}
/// Resolve a coldkey password, without unlocking or implicitly prompting.
pub async fn resolve_wallet_password(
    wallet: &Wallet,
    sources: &PasswordSources,
) -> Result<Option<SecretString>, WalletError> {
    Ok(
        resolve_password_for_keyfile(wallet.coldkey_file().path(), sources, true)
            .await?
            .map(|resolved| resolved.secret),
    )
}
pub(crate) async fn resolve_password_for_keyfile(
    path: &Path,
    sources: &PasswordSources,
    wallet_environment: bool,
) -> Result<Option<ResolvedPassword>, WalletError> {
    resolve_with(path, sources, wallet_environment, |name| {
        std::env::var(name).ok()
    })
    .await
}
pub(crate) async fn resolve_with(
    path: &Path,
    sources: &PasswordSources,
    wallet_environment: bool,
    env: impl Fn(&str) -> Option<String>,
) -> Result<Option<ResolvedPassword>, WalletError> {
    let resolved = |secret: String, source| {
        Some(ResolvedPassword {
            secret: SecretString::new(secret),
            source,
        })
    };
    if let Some(secret) = &sources.password
        && !secret.expose_secret().is_empty()
    {
        return Ok(Some(ResolvedPassword {
            secret: secret.clone(),
            source: PasswordSource::Explicit,
        }));
    }
    if wallet_environment
        && let Some(secret) = env("BT_WALLET_PASSWORD").filter(|value| !value.is_empty())
    {
        return Ok(resolved(secret, PasswordSource::WalletEnvironment));
    }
    let name = env_var_name(path);
    if let Some(value) = env(&name) {
        let value = Zeroizing::new(value);
        let secret = rtao_core::keyfile_env::decode_password_from_environment(&name, &value)?;
        if !secret.is_empty() {
            return Ok(resolved(secret, PasswordSource::KeyfileEnvironment));
        }
    }
    if wallet_environment {
        let password_file = sources
            .password_file
            .clone()
            .filter(|path| !path.as_os_str().is_empty())
            .or_else(|| {
                env("BT_WALLET_PASSWORD_FILE")
                    .filter(|value| !value.is_empty())
                    .map(PathBuf::from)
            });
        if let Some(path) = password_file {
            let path = crate::wallet::expand_user(&path)?;
            let value = rtao_core::offload(move || {
                let bytes = crate::files::read_secret_file(&path, "password file")?;
                let text = std::str::from_utf8(&bytes).map_err(|error| {
                    io_error(
                        &path,
                        std::io::Error::new(std::io::ErrorKind::InvalidData, error),
                    )
                })?;
                Ok::<_, WalletError>(Zeroizing::new(text.to_owned()))
            })
            .await
            .map_err(|error| WalletError::Blocking(error.to_string()))??;
            return Ok(resolved(
                value.trim().to_owned(),
                PasswordSource::PasswordFile,
            ));
        }
        if sources.keychain {
            return Err(WalletError::FeatureRequired(
                "M4 macOS Keychain password support",
            ));
        }
        if sources.macos_prompt {
            return Err(WalletError::FeatureRequired(
                "M4 macOS password dialog support",
            ));
        }
    }
    match sources.prompt {
        PromptPolicy::Never => Ok(None),
        PromptPolicy::IfTty if !std::io::stdin().is_terminal() => {
            Err(WalletError::PromptUnavailable)
        }
        _ => {
            let prompt = sources
                .prompt_callback
                .clone()
                .ok_or(WalletError::PromptUnavailable)?;
            let message = format!("Enter password to unlock key {}: ", path.display());
            let secret = rtao_core::offload(move || prompt.prompt(&message))
                .await
                .map_err(|error| WalletError::Blocking(error.to_string()))??;
            if secret.expose_secret().is_empty() {
                return Err(WalletError::PasswordRequired);
            }
            Ok(Some(ResolvedPassword {
                secret,
                source: PasswordSource::Prompt,
            }))
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;
    use crate::testsupport as support;
    use std::{
        collections::HashMap,
        sync::atomic::{AtomicUsize, Ordering},
    };
    struct Prompt(Arc<AtomicUsize>);
    impl PasswordPrompt for Prompt {
        fn prompt(&self, _: &str) -> Result<SecretString, WalletError> {
            self.0.fetch_add(1, Ordering::SeqCst);
            Ok(SecretString::new("prompt-only-test".into()))
        }
    }
    #[tokio::test]
    async fn password_files_reject_oversize_nonregular_and_invalid_utf8() {
        let root = support::TempRoot::new();
        let path = root.path().join("password");
        let file = std::fs::File::create(&path).unwrap();
        file.set_len(16 * 1024 * 1024 + 1).unwrap();
        drop(file);
        let sources = PasswordSources {
            password_file: Some(path.clone()),
            ..Default::default()
        };
        async fn resolve(
            sources: &PasswordSources,
        ) -> Result<Option<ResolvedPassword>, WalletError> {
            resolve_with(Path::new("/disposable/coldkey"), sources, true, |_| None).await
        }
        let error = resolve(&sources).await.err().unwrap();
        assert!(
            error
                .to_string()
                .contains("password file exceeds 16 MiB limit")
        );
        assert_eq!(
            std::fs::metadata(&path).unwrap().len(),
            16 * 1024 * 1024 + 1
        );
        std::fs::write(&path, [0xff]).unwrap();
        assert!(matches!(
            resolve(&sources).await,
            Err(WalletError::Io { .. })
        ));
        let directory_sources = PasswordSources {
            password_file: Some(root.path().to_owned()),
            ..Default::default()
        };
        assert!(matches!(
            resolve(&directory_sources).await,
            Err(WalletError::Io { .. })
        ));
        std::fs::write(&path, "  bounded-test-password\n").unwrap();
        assert_eq!(
            resolve(&sources)
                .await
                .unwrap()
                .unwrap()
                .secret
                .expose_secret(),
            "bounded-test-password"
        );
    }
    #[tokio::test]
    async fn precedence_and_empty_values_follow_exact_python_branches() {
        let path = Path::new("/disposable/wallet/coldkey");
        let name = env_var_name(path);
        let encoded =
            rtao_core::keyfile_env::encode_password_for_environment(&name, "native-test").unwrap();
        let mut env = HashMap::from([
            ("BT_WALLET_PASSWORD".to_owned(), "global-test".to_owned()),
            (name.clone(), encoded),
        ]);
        let mut sources = PasswordSources {
            password: Some(SecretString::new("explicit-test".into())),
            ..Default::default()
        };
        let first = resolve_with(path, &sources, true, |key| env.get(key).cloned())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(first.source, PasswordSource::Explicit);
        assert_eq!(first.secret.expose_secret(), "explicit-test");
        sources.password = Some(SecretString::new(String::new()));
        let second = resolve_with(path, &sources, true, |key| env.get(key).cloned())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(second.source, PasswordSource::WalletEnvironment);
        env.insert("BT_WALLET_PASSWORD".into(), String::new());
        let third = resolve_with(path, &sources, true, |key| env.get(key).cloned())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(third.source, PasswordSource::KeyfileEnvironment);
        assert_eq!(third.secret.expose_secret(), "native-test");
        env.insert(name, "invalid-base64!!!".into());
        assert!(
            resolve_with(path, &sources, true, |key| env.get(key).cloned())
                .await
                .is_err()
        );
    }
    #[tokio::test]
    async fn whole_password_file_is_trimmed_and_explicit_file_wins() {
        let temp = support::TempRoot::new();
        let explicit = temp.path().join("explicit-password");
        let fallback = temp.path().join("fallback-password");
        std::fs::write(&explicit, "  first\nsecond \n").unwrap();
        std::fs::write(&fallback, "fallback").unwrap();
        let mut sources = PasswordSources {
            password_file: Some(explicit),
            ..Default::default()
        };
        let env = |key: &str| {
            if key == "BT_WALLET_PASSWORD_FILE" {
                Some(fallback.to_string_lossy().into_owned())
            } else {
                None
            }
        };
        let first = resolve_with(Path::new("/disposable/coldkey"), &sources, true, env)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(first.secret.expose_secret(), "first\nsecond");
        assert_eq!(first.source, PasswordSource::PasswordFile);
        sources.password_file = None;
        assert_eq!(
            resolve_with(Path::new("/disposable/coldkey"), &sources, true, env)
                .await
                .unwrap()
                .unwrap()
                .secret
                .expose_secret(),
            "fallback"
        );
        std::fs::write(&fallback, "").unwrap();
        assert_eq!(
            resolve_with(Path::new("/disposable/coldkey"), &sources, true, env)
                .await
                .unwrap()
                .unwrap()
                .secret
                .expose_secret(),
            ""
        );
    }
    #[tokio::test]
    async fn library_never_prompts_and_explicit_cli_callback_runs_once() {
        let count = Arc::new(AtomicUsize::new(0));
        let mut sources = PasswordSources {
            prompt_callback: Some(Arc::new(Prompt(count.clone()))),
            ..Default::default()
        };
        assert!(
            resolve_with(Path::new("/disposable/coldkey"), &sources, true, |_| None)
                .await
                .unwrap()
                .is_none()
        );
        assert_eq!(count.load(Ordering::SeqCst), 0);
        sources.prompt = PromptPolicy::Always;
        let resolved = resolve_with(Path::new("/disposable/coldkey"), &sources, true, |_| None)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(resolved.source, PasswordSource::Prompt);
        assert_eq!(count.load(Ordering::SeqCst), 1);
        sources.prompt_callback = None;
        assert!(matches!(
            resolve_with(Path::new("/disposable/coldkey"), &sources, true, |_| None).await,
            Err(WalletError::PromptUnavailable)
        ));
    }
    #[tokio::test]
    async fn hotkeys_do_not_consult_wallet_global_password_or_files() {
        let sources = PasswordSources {
            password_file: Some(PathBuf::from("/does-not-exist")),
            ..Default::default()
        };
        assert!(
            resolve_with(Path::new("/disposable/hot"), &sources, false, |key| {
                if key == "BT_WALLET_PASSWORD" {
                    Some("global-test".into())
                } else {
                    None
                }
            })
            .await
            .unwrap()
            .is_none()
        );
    }
    #[test]
    fn secrets_are_redacted_and_path_normalization_matches_python() {
        assert_eq!(
            env_var_name(Path::new("/home/user/.bittensor/wallets/default/coldkey")),
            "BT_PW__HOME_USER__BITTENSOR_WALLETS_DEFAULT_COLDKEY"
        );
        let sources = PasswordSources {
            password: Some(SecretString::new("private-test-password".into())),
            ..Default::default()
        };
        assert!(!format!("{sources:?}").contains("private-test-password"));
        assert!(rtao_core::keyfile_env::encode_password_for_environment("", "test").is_err());
        assert!(
            rtao_core::keyfile_env::decode_password_from_environment("BT_PW_TEST", "!!!").is_err()
        );
    }

    proptest::proptest! {
        #![proptest_config(proptest::test_runner::Config { cases:256,failure_persistence:None,..Default::default() })]
        #[test]
        fn encoded_unicode_passwords_round_trip_without_global_environment(password in ".{0,256}") {
            let name="BT_PW__DISPOSABLE_WALLET_COLDKEY";
            let encoded=rtao_core::keyfile_env::encode_password_for_environment(name,&password).unwrap();
            proptest::prop_assert_eq!(rtao_core::keyfile_env::decode_password_from_environment(name,&encoded).unwrap(),password);
        }
    }
}
