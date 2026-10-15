// Derived from RaoFoundation/subtensor sdk/python/bittensor/wallets.py and keyfiles.py @ 850f2755bb05a3851f948489f0ab27b0645cebce (Apache-2.0); modified by rtao.
//! Password precedence, with all interaction supplied by the calling CLI.

use crate::{Wallet, WalletError, io_error};
use secrecy::{ExposeSecret, SecretString};
use std::{
    fmt,
    future::Future,
    io::IsTerminal,
    path::{Path, PathBuf},
    pin::Pin,
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
    /// The shared macOS Keychain account.
    Keychain,
    /// The explicit native macOS password dialog.
    MacosDialog,
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
            Self::Keychain => "macOS Keychain",
            Self::MacosDialog => "macOS password dialog",
            Self::Prompt => "prompt",
        })
    }
}
/// A hidden-entry callback implemented by a CLI; the library reads no input.
pub trait PasswordPrompt: Send + Sync {
    /// Whether the callback has an eligible terminal; injectable for CLI tests.
    fn is_terminal(&self) -> bool {
        std::io::stdin().is_terminal()
    }
    /// Ask the user for a secret; do not print or log the returned value.
    fn prompt(&self, message: &str) -> Result<SecretString, WalletError>;
    /// Report a fixed validation diagnostic before retrying a terminal prompt.
    ///
    /// Implementations may render it beside their hidden input widget. The
    /// default is silent; this callback never receives a password.
    fn invalid_password(&self, _message: &'static str) -> Result<(), WalletError> {
        Ok(())
    }
}
/// A secret-bearing native password operation without implicit interaction.
pub type NativePasswordFuture<'a, T> =
    Pin<Box<dyn Future<Output = Result<T, WalletError>> + Send + 'a>>;
/// Native password access supplied explicitly by the calling application.
pub trait NativePasswordProvider: Send + Sync {
    /// Read btcli's shared Keychain account for the requested wallet.
    fn keychain_load<'a>(
        &'a self,
        wallet_name: &'a str,
    ) -> NativePasswordFuture<'a, Option<SecretString>>;
    /// Show a hidden native dialog with the requested title and message.
    fn macos_prompt<'a>(
        &'a self,
        title: &'a str,
        message: &'a str,
    ) -> NativePasswordFuture<'a, SecretString>;
}
/// Password inputs in Python precedence order, with library prompting disabled.
#[derive(Clone, Default)]
pub struct PasswordSources {
    /// Explicit input; an empty secret falls through as in Python.
    pub password: Option<SecretString>,
    /// A password file; its entire text is trimmed, matching Python's code.
    pub password_file: Option<PathBuf>,
    /// Request the macOS Keychain source through the explicit native provider.
    pub keychain: bool,
    /// Request the native macOS dialog through the explicit native provider.
    pub macos_prompt: bool,
    /// Permission for an explicitly supplied CLI callback.
    pub prompt: PromptPolicy,
    /// The caller's hidden password-entry implementation.
    pub prompt_callback: Option<Arc<dyn PasswordPrompt>>,
    /// Explicit native-password implementation; unused unless a native flag is set.
    pub native_provider: Option<Arc<dyn NativePasswordProvider>>,
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
            .field(
                "native_provider",
                &self.native_provider.as_ref().map(|_| "configured"),
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
                value
                    .trim_matches(|character: char| {
                        character.is_whitespace() || matches!(character, '\u{1c}'..='\u{1f}')
                    })
                    .to_owned(),
                PasswordSource::PasswordFile,
            ));
        }
        if sources.keychain || sources.macos_prompt {
            let provider = sources
                .native_provider
                .as_ref()
                .ok_or(WalletError::FeatureRequired("native password provider"))?;
            let wallet_name = path
                .parent()
                .and_then(Path::file_name)
                .and_then(|name| name.to_str())
                .ok_or_else(|| {
                    WalletError::InvalidName("coldkey path has no UTF-8 wallet name".into())
                })?;
            if sources.keychain {
                if let Some(secret) = provider.keychain_load(wallet_name).await?
                    && !secret.expose_secret().is_empty()
                {
                    return Ok(Some(ResolvedPassword {
                        secret,
                        source: PasswordSource::Keychain,
                    }));
                }
                if !sources.macos_prompt {
                    return Err(WalletError::KeychainMissing(wallet_name.to_owned()));
                }
            }
            if sources.macos_prompt {
                let title = format!("Unlock {wallet_name} coldkey");
                let message = format!(
                    "Enter the password for wallet {}.",
                    rtao_primitives::pyfmt::string_repr(wallet_name)
                );
                let secret = provider.macos_prompt(&title, &message).await?;
                if secret.expose_secret().is_empty() {
                    return Err(WalletError::PasswordRequired);
                }
                return Ok(Some(ResolvedPassword {
                    secret,
                    source: PasswordSource::MacosDialog,
                }));
            }
        }
    }
    match sources.prompt {
        PromptPolicy::Never => Ok(None),
        PromptPolicy::IfTty
            if !sources
                .prompt_callback
                .as_ref()
                .is_some_and(|prompt| prompt.is_terminal()) =>
        {
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
        std::fs::write(&fallback, "\u{1c}\u{1d}  separator-password \u{1e}\u{1f}").unwrap();
        assert_eq!(
            resolve_with(Path::new("/disposable/coldkey"), &sources, true, env)
                .await
                .unwrap()
                .unwrap()
                .secret
                .expose_secret(),
            "separator-password"
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

    #[derive(Default)]
    struct Native {
        keychain: std::sync::Mutex<std::collections::VecDeque<Option<SecretString>>>,
        calls: std::sync::Mutex<Vec<String>>,
    }
    impl NativePasswordProvider for Native {
        fn keychain_load<'a>(
            &'a self,
            wallet_name: &'a str,
        ) -> NativePasswordFuture<'a, Option<SecretString>> {
            self.calls
                .lock()
                .unwrap()
                .push(format!("keychain:{wallet_name}"));
            let secret = self.keychain.lock().unwrap().pop_front().flatten();
            Box::pin(std::future::ready(Ok(secret)))
        }
        fn macos_prompt<'a>(
            &'a self,
            title: &'a str,
            message: &'a str,
        ) -> NativePasswordFuture<'a, SecretString> {
            self.calls
                .lock()
                .unwrap()
                .push(format!("dialog:{title}:{message}"));
            Box::pin(std::future::ready(Ok(SecretString::new(
                "dialog-only-test".into(),
            ))))
        }
    }

    #[tokio::test]
    async fn native_sources_preserve_provenance_and_missing_keychain_branch() {
        let native = Arc::new(Native::default());
        native.keychain.lock().unwrap().extend([
            Some(SecretString::new("keychain-only-test".into())),
            None,
            Some(SecretString::new(String::new())),
        ]);
        let mut sources = PasswordSources {
            keychain: true,
            native_provider: Some(native.clone()),
            ..Default::default()
        };
        let path = Path::new("/disposable/wallet/coldkey");
        let resolved = resolve_with(path, &sources, true, |_| None)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(resolved.source, PasswordSource::Keychain);
        assert_eq!(resolved.secret.expose_secret(), "keychain-only-test");
        let error = resolve_with(path, &sources, true, |_| None)
            .await
            .err()
            .unwrap();
        assert!(matches!(error, WalletError::KeychainMissing(_)));
        assert_eq!(
            error.to_string(),
            "no Keychain password for wallet 'wallet'; run `rtao wallet keychain save -w wallet`"
        );
        sources.macos_prompt = true;
        let resolved = resolve_with(path, &sources, true, |_| None)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(resolved.source, PasswordSource::MacosDialog);
        assert_eq!(resolved.secret.expose_secret(), "dialog-only-test");
        assert_eq!(
            *native.calls.lock().unwrap(),
            [
                "keychain:wallet",
                "keychain:wallet",
                "keychain:wallet",
                "dialog:Unlock wallet coldkey:Enter the password for wallet 'wallet'.",
            ]
        );
        assert!(!format!("{sources:?}").contains("keychain-only-test"));
        assert!(!format!("{sources:?}").contains("dialog-only-test"));
    }

    #[tokio::test]
    async fn native_provider_is_unused_without_flags_and_never_precedes_earlier_sources() {
        let native = Arc::new(Native::default());
        let path = Path::new("/disposable/wallet/coldkey");
        let mut sources = PasswordSources {
            native_provider: Some(native.clone()),
            ..Default::default()
        };
        assert!(
            resolve_with(path, &sources, true, |_| None)
                .await
                .unwrap()
                .is_none()
        );
        sources.keychain = true;
        sources.macos_prompt = true;
        sources.password = Some(SecretString::new("explicit-test".into()));
        assert_eq!(
            resolve_with(path, &sources, true, |_| None)
                .await
                .unwrap()
                .unwrap()
                .source,
            PasswordSource::Explicit
        );
        sources.password = None;
        assert_eq!(
            resolve_with(path, &sources, true, |name| (name == "BT_WALLET_PASSWORD")
                .then(|| "env-test".into()))
            .await
            .unwrap()
            .unwrap()
            .source,
            PasswordSource::WalletEnvironment
        );
        let name = env_var_name(path);
        let encoded =
            rtao_core::keyfile_env::encode_password_for_environment(&name, "keyfile-test").unwrap();
        assert_eq!(
            resolve_with(path, &sources, true, |key| (key == name)
                .then(|| encoded.clone()))
            .await
            .unwrap()
            .unwrap()
            .source,
            PasswordSource::KeyfileEnvironment
        );
        let temp = support::TempRoot::new();
        let file = temp.path().join("empty-password");
        std::fs::write(&file, " \n").unwrap();
        sources.password_file = Some(file);
        let resolved = resolve_with(path, &sources, true, |_| None)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(resolved.source, PasswordSource::PasswordFile);
        assert!(resolved.secret.expose_secret().is_empty());
        assert!(native.calls.lock().unwrap().is_empty());
        sources.password_file = None;
        assert!(
            resolve_with(path, &sources, false, |_| None)
                .await
                .unwrap()
                .is_none()
        );
        assert!(native.calls.lock().unwrap().is_empty());
    }

    struct TerminalPrompt {
        terminal: bool,
        calls: Arc<AtomicUsize>,
    }
    impl PasswordPrompt for TerminalPrompt {
        fn is_terminal(&self) -> bool {
            self.terminal
        }
        fn prompt(&self, _: &str) -> Result<SecretString, WalletError> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            Ok(SecretString::new("prompt-test".into()))
        }
    }
    #[tokio::test]
    async fn if_tty_uses_the_callback_terminal_authority_and_never_stays_silent() {
        let calls = Arc::new(AtomicUsize::new(0));
        let path = Path::new("/disposable/wallet/coldkey");
        let mut sources = PasswordSources {
            prompt: PromptPolicy::IfTty,
            ..Default::default()
        };
        assert!(matches!(
            resolve_with(path, &sources, true, |_| None).await,
            Err(WalletError::PromptUnavailable)
        ));
        sources.prompt_callback = Some(Arc::new(TerminalPrompt {
            terminal: false,
            calls: calls.clone(),
        }));
        assert!(matches!(
            resolve_with(path, &sources, true, |_| None).await,
            Err(WalletError::PromptUnavailable)
        ));
        assert_eq!(calls.load(Ordering::SeqCst), 0);
        sources.prompt_callback = Some(Arc::new(TerminalPrompt {
            terminal: true,
            calls: calls.clone(),
        }));
        assert_eq!(
            resolve_with(path, &sources, true, |_| None)
                .await
                .unwrap()
                .unwrap()
                .source,
            PasswordSource::Prompt
        );
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        sources.prompt = PromptPolicy::Never;
        assert!(
            resolve_with(path, &sources, true, |_| None)
                .await
                .unwrap()
                .is_none()
        );
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn native_flags_require_an_explicit_provider_instead_of_hidden_host_access() {
        for sources in [
            PasswordSources {
                keychain: true,
                ..Default::default()
            },
            PasswordSources {
                macos_prompt: true,
                ..Default::default()
            },
        ] {
            assert!(matches!(
                resolve_with(Path::new("/disposable/w/coldkey"), &sources, true, |_| None).await,
                Err(WalletError::FeatureRequired("native password provider"))
            ));
        }
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
