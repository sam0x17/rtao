// Derived from RaoFoundation/subtensor sdk/python/bittensor/macos_password.py @ 850f2755bb05a3851f948489f0ab27b0645cebce (Apache-2.0); modified by rtao.
// Additional source at the same commit/license: sdk/python/bittensor/cli/secrets.py.
//! Injectable, shell-free subprocess execution with erased secret buffers.

use std::{
    ffi::{OsStr, OsString},
    fmt,
    future::Future,
    path::{Path, PathBuf},
    pin::Pin,
    process::Stdio,
    time::Duration,
};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWriteExt};
use zeroize::Zeroizing;

/// Maximum bytes retained from each subprocess output pipe.
pub const MAX_PROCESS_OUTPUT: usize = 1024 * 1024;

/// A command whose arguments contain no secret material.
pub struct ProcessCommand {
    /// Executable path or a name resolved through the runner's PATH.
    pub program: PathBuf,
    /// Argument vector, without shell interpolation.
    pub args: Vec<OsString>,
    /// Optional secret input, erased on completion or cancellation.
    pub input: Option<Zeroizing<Vec<u8>>>,
    /// Whole-operation timeout, including writing and reading the pipes.
    pub timeout: Option<Duration>,
}

impl ProcessCommand {
    /// Build an argument-vector command with no input or deadline.
    pub fn new(
        program: impl Into<PathBuf>,
        args: impl IntoIterator<Item = impl Into<OsString>>,
    ) -> Self {
        Self {
            program: program.into(),
            args: args.into_iter().map(Into::into).collect(),
            input: None,
            timeout: None,
        }
    }
}

impl fmt::Debug for ProcessCommand {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ProcessCommand")
            .field("program", &self.program)
            .field("args", &self.args)
            .field("input", &self.input.as_ref().map(|_| "[REDACTED]"))
            .field("timeout", &self.timeout)
            .finish()
    }
}

/// Captured subprocess output; both pipes may contain secrets.
pub struct ProcessOutput {
    /// Whether the executable exited successfully.
    pub success: bool,
    /// Platform exit code; absent when terminated by a signal.
    pub code: Option<i32>,
    /// Standard output, erased when discarded.
    pub stdout: Zeroizing<Vec<u8>>,
    /// Standard error, erased when discarded.
    pub stderr: Zeroizing<Vec<u8>>,
}

impl fmt::Debug for ProcessOutput {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ProcessOutput")
            .field("success", &self.success)
            .field("code", &self.code)
            .field("stdout", &"[REDACTED]")
            .field("stderr", &"[REDACTED]")
            .finish()
    }
}

/// Fixed subprocess diagnostics that never include captured output or input.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProcessError {
    /// The executable could not be started.
    Spawn,
    /// A pipe or process-wait operation failed.
    Io,
    /// The whole operation exceeded its deadline.
    Timeout,
    /// A captured pipe exceeded the bounded retention limit.
    OutputLimit,
}

impl fmt::Display for ProcessError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Spawn => "could not start subprocess",
            Self::Io => "subprocess I/O failed",
            Self::Timeout => "subprocess timed out",
            Self::OutputLimit => "subprocess output exceeded limit",
        })
    }
}
impl std::error::Error for ProcessError {}

/// Future returned by a process runner.
pub type ProcessFuture<'a> =
    Pin<Box<dyn Future<Output = Result<ProcessOutput, ProcessError>> + Send + 'a>>;

/// Injectable subprocess and executable-discovery boundary.
pub trait ProcessRunner: Send {
    /// Find an executable, following the runner's PATH order.
    fn which(&self, program: &OsStr) -> Option<PathBuf>;
    /// Run the exact executable and argument vector, capturing secret output.
    fn run(&mut self, command: ProcessCommand) -> ProcessFuture<'_>;
}

impl<R: ProcessRunner + ?Sized> ProcessRunner for Box<R> {
    fn which(&self, program: &OsStr) -> Option<PathBuf> {
        (**self).which(program)
    }
    fn run(&mut self, command: ProcessCommand) -> ProcessFuture<'_> {
        (**self).run(command)
    }
}

/// Live runner; never invokes a shell or prints command output.
#[derive(Clone, Debug)]
pub struct SystemProcessRunner {
    search_path: Vec<PathBuf>,
}

impl Default for SystemProcessRunner {
    fn default() -> Self {
        Self {
            search_path: std::env::var_os("PATH")
                .map(|path| std::env::split_paths(&path).collect())
                .unwrap_or_default(),
        }
    }
}

impl SystemProcessRunner {
    /// Use explicit search directories, useful for isolated executable fixtures.
    pub fn with_search_path(search_path: Vec<PathBuf>) -> Self {
        Self { search_path }
    }
}

fn executable(path: &Path) -> bool {
    let Ok(current_dir) = std::env::current_dir() else {
        return false;
    };
    let absolute = if path.is_absolute() {
        path.to_owned()
    } else {
        current_dir.join(path)
    };
    // Check this candidate directly: PATH tilde expansion would differ from
    // shutil.which, and raw mode bits do not establish this user's X_OK access.
    which::which_in(absolute, Some(OsString::new()), current_dir).is_ok()
}

async fn capture(mut pipe: impl AsyncRead + Unpin) -> Result<Zeroizing<Vec<u8>>, ProcessError> {
    // Allocate once so Vec growth cannot leave discarded secret copies behind.
    let mut captured = Zeroizing::new(Vec::with_capacity(MAX_PROCESS_OUTPUT));
    let mut chunk = Zeroizing::new([0_u8; 4096]);
    loop {
        let size = pipe
            .read(&mut chunk[..])
            .await
            .map_err(|_| ProcessError::Io)?;
        if size == 0 {
            return Ok(captured);
        }
        if captured.len().saturating_add(size) > MAX_PROCESS_OUTPUT {
            return Err(ProcessError::OutputLimit);
        }
        captured.extend_from_slice(&chunk[..size]);
    }
}

impl ProcessRunner for SystemProcessRunner {
    fn which(&self, program: &OsStr) -> Option<PathBuf> {
        let program_path = Path::new(program);
        if program_path.components().count() > 1 || program_path.is_absolute() {
            return executable(program_path).then(|| program_path.to_owned());
        }
        self.search_path
            .iter()
            .map(|directory| directory.join(program))
            .find(|path| executable(path))
    }

    fn run(&mut self, command: ProcessCommand) -> ProcessFuture<'_> {
        Box::pin(async move {
            let mut child = tokio::process::Command::new(&command.program)
                .args(&command.args)
                .stdin(if command.input.is_some() {
                    Stdio::piped()
                } else {
                    Stdio::null()
                })
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .kill_on_drop(true)
                .spawn()
                .map_err(|_| ProcessError::Spawn)?;
            let Some(stdout) = child.stdout.take() else {
                let _ = child.kill().await;
                return Err(ProcessError::Io);
            };
            let Some(stderr) = child.stderr.take() else {
                let _ = child.kill().await;
                return Err(ProcessError::Io);
            };
            let mut stdin = child.stdin.take();
            let result = {
                let operation = async {
                    let write = async {
                        if let Some(input) = command.input {
                            let mut pipe = stdin.take().ok_or(ProcessError::Io)?;
                            pipe.write_all(&input).await.map_err(|_| ProcessError::Io)?;
                            pipe.shutdown().await.map_err(|_| ProcessError::Io)?;
                        }
                        Ok::<_, ProcessError>(())
                    };
                    let wait = async { child.wait().await.map_err(|_| ProcessError::Io) };
                    let (_, stdout, stderr, status) =
                        tokio::try_join!(write, capture(stdout), capture(stderr), wait)?;
                    Ok(ProcessOutput {
                        success: status.success(),
                        code: status.code(),
                        stdout,
                        stderr,
                    })
                };
                match command.timeout {
                    Some(timeout) => tokio::time::timeout(timeout, operation)
                        .await
                        .map_err(|_| ProcessError::Timeout)
                        .and_then(std::convert::identity),
                    None => operation.await,
                }
            };
            if result.is_err() {
                // Cancellation drops every pipe future and zeroizes its buffers.
                // Reap the direct child before exposing the fixed diagnostic.
                let _ = child.kill().await;
                let _ = child.wait().await;
            }
            result
        })
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;
    use std::io::Write;

    fn child_command(name: &str) -> ProcessCommand {
        ProcessCommand::new(
            std::env::current_exe().unwrap(),
            ["--exact", name, "--nocapture", "--test-threads=1"],
        )
    }

    fn exact_child(name: &str) -> bool {
        let args = std::env::args().collect::<Vec<_>>();
        args.windows(2)
            .any(|pair| pair[0] == "--exact" && pair[1] == name)
    }

    #[test]
    fn pipe_child() {
        if !exact_child("process::tests::pipe_child") {
            return;
        }
        let mut input = Zeroizing::new(String::new());
        std::io::Read::read_to_string(&mut std::io::stdin(), &mut input).unwrap();
        std::io::stdout().write_all(input.as_bytes()).unwrap();
        std::io::stderr()
            .write_all(b"private-error-sentinel")
            .unwrap();
    }

    #[test]
    fn sleeping_child() {
        if exact_child("process::tests::sleeping_child") {
            std::thread::sleep(Duration::from_secs(30));
        }
    }

    #[test]
    fn large_output_child() {
        if exact_child("process::tests::large_output_child") {
            std::io::stdout()
                .write_all(&vec![b'x'; MAX_PROCESS_OUTPUT + 8192])
                .unwrap();
        }
    }

    #[tokio::test]
    async fn live_pipes_capture_secret_input_without_debug_disclosure() {
        let mut command = child_command("process::tests::pipe_child");
        command.input = Some(Zeroizing::new(b"stdin-private-sentinel".to_vec()));
        command.timeout = Some(Duration::from_secs(10));
        assert!(!format!("{command:?}").contains("stdin-private-sentinel"));
        let output = SystemProcessRunner::default().run(command).await.unwrap();
        assert!(output.success);
        assert!(
            output
                .stdout
                .windows(22)
                .any(|bytes| bytes == b"stdin-private-sentinel")
        );
        assert_eq!(&output.stderr[..], b"private-error-sentinel");
        let debug = format!("{output:?}");
        assert!(!debug.contains("private"));
    }

    #[tokio::test]
    async fn live_deadline_kills_and_reaps_the_child() {
        let mut command = child_command("process::tests::sleeping_child");
        command.timeout = Some(Duration::from_millis(100));
        let start = std::time::Instant::now();
        assert_eq!(
            SystemProcessRunner::default()
                .run(command)
                .await
                .unwrap_err(),
            ProcessError::Timeout
        );
        assert!(start.elapsed() < Duration::from_secs(5));
    }

    #[tokio::test]
    async fn live_output_limit_is_enforced_without_retaining_output_in_errors() {
        let mut command = child_command("process::tests::large_output_child");
        command.timeout = Some(Duration::from_secs(10));
        assert_eq!(
            SystemProcessRunner::default()
                .run(command)
                .await
                .unwrap_err(),
            ProcessError::OutputLimit
        );
    }
}
