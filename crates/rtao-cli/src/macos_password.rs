// Derived from RaoFoundation/subtensor sdk/python/bittensor/macos_password.py @ 850f2755bb05a3851f948489f0ab27b0645cebce (Apache-2.0); modified by rtao.
//! Python-compatible macOS Keychain and password-dialog argument vectors.

use crate::{
    process::{ProcessCommand, ProcessRunner},
    prompt::python_strip,
};
use secrecy::{ExposeSecret, SecretString};
use std::fmt;
use zeroize::Zeroizing;

/// Keychain service shared with btcli.
pub const KEYCHAIN_SERVICE: &str = "bittensor.coldkey";

/// Fixed errors; subprocess output is never included in diagnostics.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MacosPasswordError {
    /// Native password dialogs require macOS.
    DialogUnavailable,
    /// The system Keychain requires macOS.
    KeychainUnavailable,
    /// The user cancelled the dialog.
    Cancelled,
    /// The native dialog failed.
    DialogFailed,
    /// The user entered no password.
    Empty,
    /// Keychain's two-line stdin protocol rejects newline-bearing passwords.
    Newline,
    /// The Keychain entry could not be saved.
    SaveFailed,
    /// A captured password was not valid UTF-8.
    InvalidEncoding,
    /// The system Keychain executable could not be accessed.
    AccessFailed,
}

impl fmt::Display for MacosPasswordError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::DialogUnavailable => "macOS password dialog is only available on macOS",
            Self::KeychainUnavailable => "macOS Keychain is only available on macOS",
            Self::Cancelled => "password entry cancelled",
            Self::DialogFailed => "macOS password prompt failed",
            Self::Empty => "empty password entered",
            Self::Newline => "password may not contain newline characters",
            Self::SaveFailed => "could not save password to Keychain",
            Self::InvalidEncoding => "password subprocess returned invalid UTF-8",
            Self::AccessFailed => "could not access macOS Keychain",
        })
    }
}
impl std::error::Error for MacosPasswordError {}

/// Native password operations with injectable platform and subprocess seams.
pub struct MacosPasswords<R> {
    runner: R,
    macos: bool,
}

impl<R: ProcessRunner> MacosPasswords<R> {
    /// Use the actual target platform with the provided runner.
    pub fn new(runner: R) -> Self {
        Self::with_platform(runner, cfg!(target_os = "macos"))
    }

    /// Supply an explicit platform for pure argument-vector tests.
    pub fn with_platform(runner: R, macos: bool) -> Self {
        Self { runner, macos }
    }

    /// Recover the runner, including observations collected by a test double.
    pub fn into_runner(self) -> R {
        self.runner
    }

    fn require_macos(&self, dialog: bool) -> Result<(), MacosPasswordError> {
        if self.macos {
            Ok(())
        } else if dialog {
            Err(MacosPasswordError::DialogUnavailable)
        } else {
            Err(MacosPasswordError::KeychainUnavailable)
        }
    }

    /// Show the hidden native macOS password dialog, preserving Python's script.
    pub async fn prompt_password(
        &mut self,
        title: &str,
        message: &str,
    ) -> Result<SecretString, MacosPasswordError> {
        self.require_macos(true)?;
        let script = format!(
            "set dlg to display dialog \"{}\" with title \"{}\" default answer \"\" with hidden answer buttons {{\"Cancel\", \"OK\"}} default button \"OK\"\nreturn text returned of dlg",
            escape_applescript(message),
            escape_applescript(title),
        );
        let result = self
            .runner
            .run(ProcessCommand::new("osascript", ["-e", &script]))
            .await
            .map_err(|_| MacosPasswordError::DialogFailed)?;
        if !result.success {
            // Python checks stderr before stdout, and only these cancellation markers.
            let detail = if result.stderr.is_empty() {
                &result.stdout
            } else {
                &result.stderr
            };
            if detail
                .windows(b"User canceled".len())
                .any(|part| part == b"User canceled")
                || detail.windows(4).any(|part| part == b"-128")
            {
                return Err(MacosPasswordError::Cancelled);
            }
            return Err(MacosPasswordError::DialogFailed);
        }
        let text = process_text(&result.stdout)?;
        let text = text
            .split_once("text returned:")
            .map_or(text.as_str(), |(_, tail)| tail);
        let password = python_strip(text);
        if password.is_empty() {
            return Err(MacosPasswordError::Empty);
        }
        Ok(SecretString::new(password.to_owned()))
    }

    /// Load the exact shared Keychain account; failure or empty output means absent.
    pub async fn keychain_load(
        &mut self,
        wallet_name: &str,
    ) -> Result<Option<SecretString>, MacosPasswordError> {
        self.require_macos(false)?;
        let result = self
            .runner
            .run(ProcessCommand::new(
                "/usr/bin/security",
                [
                    "find-generic-password",
                    "-s",
                    KEYCHAIN_SERVICE,
                    "-a",
                    wallet_name,
                    "-w",
                ],
            ))
            .await
            .map_err(|_| MacosPasswordError::AccessFailed)?;
        if !result.success {
            return Ok(None);
        }
        let text = process_text(&result.stdout)?;
        let password = python_strip(&text);
        Ok((!password.is_empty()).then(|| SecretString::new(password.to_owned())))
    }

    /// Replace an entry using Python's secret-on-stdin two-line protocol.
    pub async fn keychain_save(
        &mut self,
        wallet_name: &str,
        password: &SecretString,
    ) -> Result<(), MacosPasswordError> {
        self.require_macos(false)?;
        let password = password.expose_secret();
        if password.contains(['\n', '\r']) {
            return Err(MacosPasswordError::Newline);
        }
        // The preliminary deletion is deliberately best-effort, as in Python.
        let _ = self
            .runner
            .run(ProcessCommand::new(
                "/usr/bin/security",
                [
                    "delete-generic-password",
                    "-s",
                    KEYCHAIN_SERVICE,
                    "-a",
                    wallet_name,
                ],
            ))
            .await
            .map_err(|_| MacosPasswordError::SaveFailed)?;
        let mut command = ProcessCommand::new(
            "/usr/bin/security",
            [
                "add-generic-password",
                "-s",
                KEYCHAIN_SERVICE,
                "-a",
                wallet_name,
                "-U",
                "-w",
            ],
        );
        let mut input = Zeroizing::new(Vec::with_capacity(
            password.len().saturating_mul(2).saturating_add(2),
        ));
        input.extend_from_slice(password.as_bytes());
        input.push(b'\n');
        input.extend_from_slice(password.as_bytes());
        input.push(b'\n');
        command.input = Some(input);
        let result = self
            .runner
            .run(command)
            .await
            .map_err(|_| MacosPasswordError::SaveFailed)?;
        if result.success {
            Ok(())
        } else {
            Err(MacosPasswordError::SaveFailed)
        }
    }

    /// Delete an entry; preserve Python's boolean result for a failed deletion.
    pub async fn keychain_delete(&mut self, wallet_name: &str) -> Result<bool, MacosPasswordError> {
        self.require_macos(false)?;
        self.runner
            .run(ProcessCommand::new(
                "/usr/bin/security",
                [
                    "delete-generic-password",
                    "-s",
                    KEYCHAIN_SERVICE,
                    "-a",
                    wallet_name,
                ],
            ))
            .await
            .map(|output| output.success)
            .map_err(|_| MacosPasswordError::AccessFailed)
    }

    /// Resolve chain/dialog flags after explicit, environment and file sources.
    ///
    /// The caller must try the four earlier sources first. An absent Keychain
    /// item only falls through when the dialog flag was also requested.
    pub async fn resolve_flags(
        &mut self,
        wallet_name: &str,
        keychain: bool,
        dialog: bool,
    ) -> Result<Option<SecretString>, NativePasswordError> {
        if keychain {
            if let Some(password) = self.keychain_load(wallet_name).await? {
                return Ok(Some(password));
            }
            if !dialog {
                return Err(NativePasswordError::MissingKeychain(wallet_name.to_owned()));
            }
        }
        if dialog {
            return self
                .prompt_password(
                    &format!("Unlock {wallet_name} coldkey"),
                    &format!(
                        "Enter the password for wallet {}.",
                        rtao::primitives::pyfmt::string_repr(wallet_name)
                    ),
                )
                .await
                .map(Some)
                .map_err(Into::into);
        }
        Ok(None)
    }
}

/// Missing-entry remediation and fixed native-password failures.
#[derive(Debug)]
pub enum NativePasswordError {
    /// A requested Keychain account did not contain a password.
    MissingKeychain(String),
    /// A native password operation failed without exposing process output.
    Native(MacosPasswordError),
}

impl fmt::Display for NativePasswordError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MissingKeychain(name) => write!(
                f,
                "no Keychain password for wallet {}; run `rtao wallet keychain save -w {name}`",
                rtao::primitives::pyfmt::string_repr(name)
            ),
            Self::Native(error) => error.fmt(f),
        }
    }
}
impl std::error::Error for NativePasswordError {}
impl From<MacosPasswordError> for NativePasswordError {
    fn from(error: MacosPasswordError) -> Self {
        Self::Native(error)
    }
}

/// Shared async adapter connecting the CLI's runner to wallet password sources.
pub struct NativePasswordAdapter<R> {
    native: tokio::sync::Mutex<MacosPasswords<R>>,
}
impl<R: ProcessRunner> NativePasswordAdapter<R> {
    /// Wrap one native implementation; access is serialized across wallet clones.
    pub fn new(native: MacosPasswords<R>) -> Self {
        Self {
            native: tokio::sync::Mutex::new(native),
        }
    }
    /// Recover the implementation after all password operations finish.
    pub fn into_inner(self) -> MacosPasswords<R> {
        self.native.into_inner()
    }
}
fn wallet_error(error: MacosPasswordError) -> rtao::wallet::WalletError {
    let message = match error {
        MacosPasswordError::DialogUnavailable => "macOS password dialog is only available on macOS",
        MacosPasswordError::KeychainUnavailable => "macOS Keychain is only available on macOS",
        MacosPasswordError::Cancelled => "password entry cancelled",
        MacosPasswordError::DialogFailed => "macOS password prompt failed",
        MacosPasswordError::Empty => "empty password entered",
        MacosPasswordError::Newline => "password may not contain newline characters",
        MacosPasswordError::SaveFailed => "could not save password to Keychain",
        MacosPasswordError::InvalidEncoding => "password subprocess returned invalid UTF-8",
        MacosPasswordError::AccessFailed => "could not access macOS Keychain",
    };
    rtao::wallet::WalletError::NativePassword(message)
}
impl<R: ProcessRunner> rtao::wallet::NativePasswordProvider for NativePasswordAdapter<R> {
    fn keychain_load<'a>(
        &'a self,
        wallet_name: &'a str,
    ) -> rtao::wallet::NativePasswordFuture<'a, Option<SecretString>> {
        Box::pin(async move {
            self.native
                .lock()
                .await
                .keychain_load(wallet_name)
                .await
                .map_err(wallet_error)
        })
    }
    fn macos_prompt<'a>(
        &'a self,
        title: &'a str,
        message: &'a str,
    ) -> rtao::wallet::NativePasswordFuture<'a, SecretString> {
        Box::pin(async move {
            self.native
                .lock()
                .await
                .prompt_password(title, message)
                .await
                .map_err(wallet_error)
        })
    }
}

fn escape_applescript(text: &str) -> String {
    text.replace('\\', "\\\\").replace('"', "\\\"")
}

// subprocess.run(text=True) applies universal newlines before Python's strip
// and dialog-prefix parsing. Keep the normalized copy in erased storage, with
// enough initial capacity that normalization never reallocates secret bytes.
fn process_text(stdout: &[u8]) -> Result<Zeroizing<String>, MacosPasswordError> {
    let text = std::str::from_utf8(stdout).map_err(|_| MacosPasswordError::InvalidEncoding)?;
    let mut normalized = Zeroizing::new(String::with_capacity(text.len()));
    let mut characters = text.chars().peekable();
    while let Some(character) = characters.next() {
        if character == '\r' {
            if characters.peek() == Some(&'\n') {
                let _ = characters.next();
            }
            normalized.push('\n');
        } else {
            normalized.push(character);
        }
    }
    Ok(normalized)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;
    use crate::process::{ProcessError, ProcessFuture, ProcessOutput};
    use std::{collections::VecDeque, ffi::OsStr, path::PathBuf};

    #[derive(Default)]
    struct Runner {
        seen: Vec<ProcessCommand>,
        results: VecDeque<Result<ProcessOutput, ProcessError>>,
    }
    impl ProcessRunner for Runner {
        fn which(&self, _: &OsStr) -> Option<PathBuf> {
            None
        }
        fn run(&mut self, command: ProcessCommand) -> ProcessFuture<'_> {
            self.seen.push(command);
            let result = self.results.pop_front().expect("planned subprocess");
            Box::pin(async move { result })
        }
    }
    fn output(success: bool, stdout: &[u8], stderr: &[u8]) -> Result<ProcessOutput, ProcessError> {
        Ok(ProcessOutput {
            success,
            code: Some(i32::from(!success)),
            stdout: Zeroizing::new(stdout.to_vec()),
            stderr: Zeroizing::new(stderr.to_vec()),
        })
    }
    fn args(command: &ProcessCommand) -> Vec<&str> {
        command
            .args
            .iter()
            .map(|arg| arg.to_str().unwrap())
            .collect()
    }

    #[tokio::test]
    async fn security_vectors_and_secret_stdin_match_the_authenticated_source() {
        let runner = Runner {
            results: VecDeque::from([
                output(true, b"  stored\n", b""),
                output(false, b"", b"missing"),
                output(true, b"", b""),
                output(true, b"", b""),
            ]),
            ..Default::default()
        };
        let mut native = MacosPasswords::with_platform(runner, true);
        assert_eq!(
            native
                .keychain_load("w $(false) `false`")
                .await
                .unwrap()
                .unwrap()
                .expose_secret(),
            "stored"
        );
        let secret = SecretString::new("stdin-secret-sentinel".into());
        native
            .keychain_save("w $(false) `false`", &secret)
            .await
            .unwrap();
        assert!(native.keychain_delete("w $(false) `false`").await.unwrap());
        let runner = native.into_runner();
        assert_eq!(runner.seen.len(), 4);
        for command in &runner.seen {
            assert_eq!(command.program, PathBuf::from("/usr/bin/security"));
            assert!(!format!("{command:?}").contains("stdin-secret-sentinel"));
        }
        assert_eq!(
            args(&runner.seen[0]),
            [
                "find-generic-password",
                "-s",
                "bittensor.coldkey",
                "-a",
                "w $(false) `false`",
                "-w"
            ]
        );
        assert_eq!(
            args(&runner.seen[1]),
            [
                "delete-generic-password",
                "-s",
                "bittensor.coldkey",
                "-a",
                "w $(false) `false`"
            ]
        );
        assert_eq!(
            args(&runner.seen[2]),
            [
                "add-generic-password",
                "-s",
                "bittensor.coldkey",
                "-a",
                "w $(false) `false`",
                "-U",
                "-w"
            ]
        );
        assert_eq!(
            runner.seen[2].input.as_deref().unwrap().as_slice(),
            b"stdin-secret-sentinel\nstdin-secret-sentinel\n"
        );
        assert_eq!(args(&runner.seen[3]), args(&runner.seen[1]));
    }

    #[tokio::test]
    async fn dialog_script_escapes_quotes_and_backslashes_and_parses_prefix() {
        let runner = Runner {
            results: VecDeque::from([output(
                true,
                b"button returned:OK, text returned:  private  \n",
                b"",
            )]),
            ..Default::default()
        };
        let mut native = MacosPasswords::with_platform(runner, true);
        assert_eq!(
            native
                .prompt_password("t\"\\", "m\"\\")
                .await
                .unwrap()
                .expose_secret(),
            "private"
        );
        let runner = native.into_runner();
        assert_eq!(runner.seen[0].program, PathBuf::from("osascript"));
        assert_eq!(
            args(&runner.seen[0]),
            [
                "-e",
                "set dlg to display dialog \"m\\\"\\\\\" with title \"t\\\"\\\\\" default answer \"\" with hidden answer buttons {\"Cancel\", \"OK\"} default button \"OK\"\nreturn text returned of dlg"
            ]
        );
    }

    #[tokio::test]
    async fn successful_password_output_normalizes_internal_cr_and_crlf_before_strip() {
        let runner = Runner {
            results: VecDeque::from([
                output(true, b"  a\rb\r\nc\n  ", b""),
                output(
                    true,
                    "button returned:OK\r\ntext returned:\r\n \u{1c}caf\u{e9}\rline\r\nend\u{1f} \r\n"
                        .as_bytes(),
                    b"",
                ),
                output(true, b"unprefixed\rpassword\r\nvalue\n", b""),
            ]),
            ..Default::default()
        };
        let mut native = MacosPasswords::with_platform(runner, true);
        assert_eq!(
            native
                .keychain_load("w")
                .await
                .unwrap()
                .unwrap()
                .expose_secret(),
            "a\nb\nc"
        );
        assert_eq!(
            native
                .prompt_password("title", "message")
                .await
                .unwrap()
                .expose_secret(),
            "caf\u{e9}\nline\nend"
        );
        assert_eq!(
            native
                .prompt_password("title", "message")
                .await
                .unwrap()
                .expose_secret(),
            "unprefixed\npassword\nvalue"
        );
        assert!(native.into_runner().results.is_empty());
    }

    #[tokio::test]
    async fn failed_process_output_never_appears_in_error_or_source() {
        for (stderr, expected) in [
            (
                b"private-secret-sentinel".as_slice(),
                MacosPasswordError::DialogFailed,
            ),
            (
                b"User canceled private-secret-sentinel".as_slice(),
                MacosPasswordError::Cancelled,
            ),
            (
                b"private-secret-sentinel -128".as_slice(),
                MacosPasswordError::Cancelled,
            ),
        ] {
            let runner = Runner {
                results: VecDeque::from([output(false, b"private-secret-sentinel", stderr)]),
                ..Default::default()
            };
            let error = MacosPasswords::with_platform(runner, true)
                .prompt_password("title", "message")
                .await
                .unwrap_err();
            assert_eq!(error, expected);
            assert!(!format!("{error:?} {error}").contains("private-secret"));
            assert!(std::error::Error::source(&error).is_none());
        }
    }

    #[tokio::test]
    async fn platform_and_newline_fail_before_any_subprocess() {
        let mut unavailable = MacosPasswords::with_platform(Runner::default(), false);
        assert_eq!(
            unavailable.keychain_load("w").await.unwrap_err(),
            MacosPasswordError::KeychainUnavailable
        );
        assert_eq!(
            unavailable.prompt_password("t", "m").await.unwrap_err(),
            MacosPasswordError::DialogUnavailable
        );
        assert!(unavailable.into_runner().seen.is_empty());
        for text in ["x\ny", "x\ry"] {
            let mut native = MacosPasswords::with_platform(Runner::default(), true);
            assert_eq!(
                native
                    .keychain_save("w", &SecretString::new(text.into()))
                    .await
                    .unwrap_err(),
                MacosPasswordError::Newline
            );
            assert!(native.into_runner().seen.is_empty());
        }
    }

    #[tokio::test]
    async fn missing_entry_only_falls_through_with_dialog_flag() {
        let runner = Runner {
            results: VecDeque::from([output(false, b"", b"")]),
            ..Default::default()
        };
        let error = MacosPasswords::with_platform(runner, true)
            .resolve_flags("w", true, false)
            .await
            .unwrap_err();
        assert!(error.to_string().contains("rtao wallet keychain save -w w"));
        let runner = Runner {
            results: VecDeque::from([
                output(true, b" \n", b""),
                output(true, b"dialog-password\n", b""),
            ]),
            ..Default::default()
        };
        let mut native = MacosPasswords::with_platform(runner, true);
        assert_eq!(
            native
                .resolve_flags("w", true, true)
                .await
                .unwrap()
                .unwrap()
                .expose_secret(),
            "dialog-password"
        );
        assert_eq!(native.into_runner().seen.len(), 2);
        let runner = Runner {
            results: VecDeque::from([output(true, b"stored\n", b"")]),
            ..Default::default()
        };
        let mut native = MacosPasswords::with_platform(runner, true);
        assert_eq!(
            native
                .resolve_flags("w", true, true)
                .await
                .unwrap()
                .unwrap()
                .expose_secret(),
            "stored"
        );
        assert_eq!(native.into_runner().seen.len(), 1);
    }
}
