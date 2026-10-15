//! CLI failures carry the observable process status without secret material.

use std::fmt;

/// A user-visible CLI failure and its process status.
#[derive(Debug)]
pub struct CliError {
    /// Status: 1 refusal/failure, 2 invalid or missing input, 130 interruption.
    pub code: u8,
    /// Safe diagnostic; never include secret values or captured native output.
    pub message: String,
    /// Optional actionable next step.
    pub help: Option<String>,
}
impl CliError {
    /// A runtime failure or declined mutation.
    pub fn failure(message: impl Into<String>) -> Self {
        Self {
            code: 1,
            message: message.into(),
            help: None,
        }
    }
    /// An invalid or missing argument.
    pub fn input(message: impl Into<String>) -> Self {
        Self {
            code: 2,
            message: message.into(),
            help: None,
        }
    }
    /// User cancellation, terminal EOF or interrupt.
    pub fn aborted() -> Self {
        Self {
            code: 130,
            message: "aborted.".into(),
            help: None,
        }
    }
    /// Attach safe remediation.
    pub fn with_help(mut self, help: impl Into<String>) -> Self {
        self.help = Some(help.into());
        self
    }
}
impl fmt::Display for CliError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}
impl std::error::Error for CliError {}
