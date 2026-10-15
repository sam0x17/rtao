// Derived from RaoFoundation/subtensor sdk/python/bittensor/cli/secrets.py @ 850f2755bb05a3851f948489f0ab27b0645cebce (Apache-2.0); modified by rtao.
//! Secret flag warnings and ordered, bounded clipboard subprocesses.

use crate::{
    process::{ProcessCommand, ProcessRunner},
    prompt::{PromptError, Terminal},
};
use secrecy::{ExposeSecret, SecretString};
use std::time::Duration;
use zeroize::Zeroizing;

/// Python's clipboard tool preference, including exact arguments.
pub const CLIPBOARD_COMMANDS: &[&[&str]] = &[
    &["pbcopy"],
    &["wl-copy"],
    &["xclip", "-selection", "clipboard"],
    &["xsel", "--clipboard", "--input"],
];

/// Warn about nonempty secret flags without receiving or formatting their values.
pub fn argv_secret_warning(provided: &[(&str, bool)]) -> Option<String> {
    let flags = provided
        .iter()
        .filter(|(_, nonempty)| *nonempty)
        .map(|(flag, _)| format!("`{flag}`"))
        .collect::<Vec<_>>();
    (!flags.is_empty()).then(|| format!("warning: secrets passed as flags ({}) land in shell history and `ps` output — omit the flag to be prompted without echo", flags.join(", ")))
}

/// Copy a secret with Python's executable precedence and ten-second timeout.
///
/// Only stdin carries the secret. Failed tools fall through to the next tool;
/// captured output and errors are deliberately never displayed.
pub async fn copy_to_clipboard(runner: &mut dyn ProcessRunner, secret: &SecretString) -> bool {
    for parts in CLIPBOARD_COMMANDS {
        let Some(program) = runner.which(std::ffi::OsStr::new(parts[0])) else {
            continue;
        };
        let mut command = ProcessCommand::new(program, parts.iter().skip(1).copied());
        command.input = Some(Zeroizing::new(secret.expose_secret().as_bytes().to_vec()));
        command.timeout = Some(Duration::from_secs(10));
        if runner.run(command).await.is_ok_and(|output| output.success) {
            return true;
        }
    }
    false
}

/// Confirm copying without echoing the secret, or report Python's fallback.
///
/// False leaves the caller responsible for its documented printing fallback.
pub async fn copy_secret_to_clipboard(
    runner: &mut dyn ProcessRunner,
    terminal: &mut dyn Terminal,
    secret: &SecretString,
    label: &str,
) -> Result<bool, PromptError> {
    if copy_to_clipboard(runner, secret).await {
        terminal.message(&format!("{label} copied to clipboard (not printed)"))?;
        return Ok(true);
    }
    terminal.message(
        "warning: no clipboard tool found (pbcopy, wl-copy, xclip, or xsel) — printing instead",
    )?;
    Ok(false)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;
    use crate::process::{ProcessError, ProcessFuture, ProcessOutput};
    use std::{
        collections::{HashSet, VecDeque},
        ffi::OsStr,
        path::PathBuf,
    };
    #[derive(Default)]
    struct Runner {
        available: HashSet<String>,
        seen: Vec<ProcessCommand>,
        results: VecDeque<Result<bool, ProcessError>>,
    }
    impl ProcessRunner for Runner {
        fn which(&self, program: &OsStr) -> Option<PathBuf> {
            self.available
                .contains(program.to_str().unwrap())
                .then(|| PathBuf::from("/fixture path").join(program))
        }
        fn run(&mut self, command: ProcessCommand) -> ProcessFuture<'_> {
            self.seen.push(command);
            let result = self
                .results
                .pop_front()
                .unwrap()
                .map(|success| ProcessOutput {
                    success,
                    code: Some(i32::from(!success)),
                    stdout: Zeroizing::new(b"private-output".to_vec()),
                    stderr: Zeroizing::new(b"private-error".to_vec()),
                });
            Box::pin(async move { result })
        }
    }

    #[tokio::test]
    async fn available_tool_precedence_exact_arguments_and_timeout_match_python() {
        let mut runner = Runner {
            available: ["pbcopy", "wl-copy", "xclip", "xsel"]
                .map(String::from)
                .into_iter()
                .collect(),
            results: VecDeque::from([
                Err(ProcessError::Timeout),
                Ok(false),
                Err(ProcessError::Spawn),
                Ok(true),
            ]),
            ..Default::default()
        };
        assert!(
            copy_to_clipboard(
                &mut runner,
                &SecretString::new("private-clipboard-sentinel".into())
            )
            .await
        );
        assert_eq!(runner.seen.len(), 4);
        for (command, expected) in runner.seen.iter().zip(CLIPBOARD_COMMANDS) {
            assert_eq!(
                command.program,
                PathBuf::from("/fixture path").join(expected[0])
            );
            assert_eq!(
                command
                    .args
                    .iter()
                    .map(|arg| arg.to_str().unwrap())
                    .collect::<Vec<_>>(),
                expected[1..]
            );
            assert_eq!(command.timeout, Some(Duration::from_secs(10)));
            assert_eq!(
                command.input.as_deref().unwrap().as_slice(),
                b"private-clipboard-sentinel"
            );
            assert!(!format!("{command:?}").contains("private-clipboard"));
        }
    }

    #[tokio::test]
    async fn first_success_stops_and_missing_executables_are_never_invoked() {
        let mut runner = Runner {
            available: ["wl-copy", "xsel"].map(String::from).into_iter().collect(),
            results: VecDeque::from([Ok(true)]),
            ..Default::default()
        };
        assert!(copy_to_clipboard(&mut runner, &SecretString::new("private".into())).await);
        assert_eq!(runner.seen.len(), 1);
        assert_eq!(runner.seen[0].program.file_name().unwrap(), "wl-copy");
        let mut absent = Runner::default();
        assert!(!copy_to_clipboard(&mut absent, &SecretString::new("private".into())).await);
        assert!(absent.seen.is_empty());
    }

    #[test]
    fn warning_mentions_only_nonempty_flag_names() {
        assert!(argv_secret_warning(&[("--seed", false)]).is_none());
        let warning = argv_secret_warning(&[
            ("--mnemonic", true),
            ("--seed", false),
            ("--private-key", true),
        ])
        .unwrap();
        assert!(warning.contains("`--mnemonic`, `--private-key`"));
        assert!(!warning.contains("`--seed`"));
    }
}
