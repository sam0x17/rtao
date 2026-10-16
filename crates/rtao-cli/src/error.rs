// Derived from RaoFoundation/subtensor sdk/python/bittensor/cli/context.py @ 850f2755bb05a3851f948489f0ab27b0645cebce (Apache-2.0); modified by rtao.
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
    /// Preserve typed chain remediation, gateway help and submission recovery.
    pub(crate) fn transaction(error: rtao::tx::TxError) -> Self {
        let help = match &error {
            rtao::tx::TxError::Chain(error)
            | rtao::tx::TxError::Intent(rtao::tx::IntentError::Chain(error)) => {
                Some(error.remediation().to_owned())
            }
            rtao::tx::TxError::RpcPolicy(policy)
            | rtao::tx::TxError::Rpc(rtao_rpc::RpcError::Policy(policy)) => {
                Some(rpc_policy_help(policy))
            }
            rtao::tx::TxError::Rpc(rtao_rpc::RpcError::FateUnknown { hash }) => {
                Some(unknown_fate_help(hash))
            }
            rtao::tx::TxError::Intent(rtao::tx::IntentError::Client(error)) => client_help(error),
            _ => None,
        };
        Self {
            help,
            ..Self::failure(error.to_string())
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

fn rpc_policy_help(policy: &rtao_rpc::PolicyRefusal) -> String {
    match policy.retry_after.as_deref().filter(|raw| !raw.is_empty()) {
        Some(raw) if raw.bytes().all(|byte| byte.is_ascii_digit()) => {
            format!("wait {raw} seconds, then retry")
        }
        Some(raw) => format!("retry after {raw}"),
        None => "reduce the request or connection rate, then retry".into(),
    }
}

fn unknown_fate_help(hash: &rtao_rpc::H256) -> String {
    format!(
        "inspect transaction 0x{} on chain before retrying; do not resubmit until its outcome is known",
        hex::encode(hash.0),
    )
}

fn client_help(error: &rtao::client::ClientError) -> Option<String> {
    match error {
        rtao::client::ClientError::Rpc(rtao_rpc::RpcError::FateUnknown { hash }) => {
            Some(unknown_fate_help(hash))
        }
        error => error.policy().map(rpc_policy_help),
    }
}

impl From<rtao::client::ClientError> for CliError {
    fn from(error: rtao::client::ClientError) -> Self {
        let help = client_help(&error);
        Self {
            help,
            ..Self::failure(error.to_string())
        }
    }
}

impl From<rtao::reads::ReadError> for CliError {
    fn from(error: rtao::reads::ReadError) -> Self {
        match error {
            rtao::reads::ReadError::Client(error) => Self::from(error),
            error => Self::failure(error.to_string()),
        }
    }
}

impl From<rtao_intents::pow::PowError> for CliError {
    fn from(error: rtao_intents::pow::PowError) -> Self {
        match error {
            rtao_intents::pow::PowError::Client(error) => Self::from(error),
            rtao_intents::pow::PowError::Head(rtao::client::HeadError::Rpc(error)) => {
                Self::from(rtao::client::ClientError::from(error))
            }
            error => Self::failure(error.to_string()),
        }
    }
}
