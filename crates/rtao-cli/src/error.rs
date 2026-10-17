// Derived from RaoFoundation/subtensor sdk/python/bittensor/cli/context.py @ 850f2755bb05a3851f948489f0ab27b0645cebce (Apache-2.0); modified by rtao.
//! CLI failures carry the observable process status without secret material.

use std::fmt;

/// Typed context retained until the final CLI diagnostic is rendered.
#[derive(Debug)]
pub enum CliDiagnostic {
    /// Preserve the chain taxonomy, description and documentation.
    Chain(Box<rtao::tx::ChainError>),
    /// An ordinary connection failure awaiting the selected network's context.
    Connection,
    /// Active execution policy refused the operation.
    Policy {
        /// Documentation for the policy violation taxonomy.
        docs_url: String,
    },
}

/// A user-visible CLI failure and its process status.
#[derive(Debug)]
pub struct CliError {
    /// Status: 1 refusal/failure, 2 invalid or missing input, 130 interruption.
    pub code: u8,
    /// Safe diagnostic; never include secret values or captured native output.
    pub message: String,
    /// Optional actionable next step.
    pub help: Option<String>,
    /// Typed diagnostic metadata, independent of the displayed message.
    pub diagnostic: Option<CliDiagnostic>,
}
impl CliError {
    /// A runtime failure or declined mutation.
    pub fn failure(message: impl Into<String>) -> Self {
        Self {
            code: 1,
            message: message.into(),
            help: None,
            diagnostic: None,
        }
    }
    /// Preserve typed chain, signer, gateway and submission recovery help.
    pub(crate) fn transaction(error: rtao::tx::TxError) -> Self {
        use rtao::tx::{IntentError, TxError};
        match error {
            TxError::Chain(error) | TxError::Intent(IntentError::Chain(error)) => Self {
                help: Some(error.remediation().to_owned()),
                diagnostic: Some(CliDiagnostic::Chain(Box::new(error.clone()))),
                ..Self::failure(error.to_string())
            },
            TxError::Policy(error) => Self {
                help: rtao_catalog::py::REMEDIATION
                    .iter()
                    .find_map(|(code, help)| (*code == error.code()).then(|| (*help).to_owned())),
                diagnostic: Some(CliDiagnostic::Policy {
                    docs_url: format!(
                        "{}/errors/{}",
                        rtao_catalog::py::DOCS_URL,
                        error.code().as_str().replace('_', "-"),
                    ),
                }),
                ..Self::failure(error.to_string())
            },
            TxError::Rpc(error) => Self::from(rtao::client::ClientError::Rpc(error)),
            TxError::Intent(IntentError::Client(error)) => Self::from(error),
            TxError::ConnectionNotReady => Self {
                diagnostic: Some(CliDiagnostic::Connection),
                ..Self::failure("connection is not ready")
            },
            error => {
                let help = match &error {
                    TxError::RpcPolicy(policy) => Some(rpc_policy_help(policy)),
                    TxError::Signer(error) | TxError::Intent(IntentError::Signer(error)) => {
                        error.help().map(str::to_owned)
                    }
                    _ => None,
                };
                Self {
                    help,
                    ..Self::failure(error.to_string())
                }
            }
        }
    }
    /// An invalid or missing argument.
    pub fn input(message: impl Into<String>) -> Self {
        Self {
            code: 2,
            ..Self::failure(message)
        }
    }
    /// User cancellation, terminal EOF or interrupt.
    pub fn aborted() -> Self {
        Self {
            code: 130,
            ..Self::failure("aborted.")
        }
    }
    /// Attach safe remediation.
    pub fn with_help(mut self, help: impl Into<String>) -> Self {
        self.help = Some(help.into());
        self
    }
    /// Attach endpoint guidance once, only for a typed connection failure.
    pub(crate) fn for_network(mut self, network: &str) -> Self {
        if self.message.is_empty() {
            return self;
        }
        if matches!(self.diagnostic, Some(CliDiagnostic::Connection)) {
            self.diagnostic = None;
            self.message = format!("could not reach {network}: {}", self.message);
            let retry = "check the endpoint and network connection, then retry";
            self.help = Some(match self.help.take() {
                Some(help) if !help.is_empty() => format!("{help}; {retry}"),
                _ => retry.into(),
            });
        }
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

pub(crate) fn unknown_fate_help(hash: &[u8; 32]) -> String {
    format!(
        "inspect transaction 0x{} on chain before retrying; do not resubmit until its outcome is known",
        hex::encode(hash),
    )
}

fn client_help(error: &rtao::client::ClientError) -> Option<String> {
    match error {
        rtao::client::ClientError::Signing(error) => error.help().map(str::to_owned),
        rtao::client::ClientError::Rpc(rtao_rpc::RpcError::FateUnknown { hash }) => {
            Some(unknown_fate_help(&hash.0))
        }
        error => error.policy().map(rpc_policy_help),
    }
}

fn ordinary_connection(error: &rtao::client::ClientError) -> bool {
    matches!(
        error,
        rtao::client::ClientError::Rpc(
            rtao_rpc::RpcError::Transport(_)
                | rtao_rpc::RpcError::Timeout { .. }
                | rtao_rpc::RpcError::Handshake { .. }
                | rtao_rpc::RpcError::MaxRetriesExceeded { .. }
                | rtao_rpc::RpcError::Closed
        )
    )
}

impl From<rtao::tx::TxError> for CliError {
    fn from(error: rtao::tx::TxError) -> Self {
        Self::transaction(error)
    }
}

impl From<rtao::signer::SignerError> for CliError {
    fn from(error: rtao::signer::SignerError) -> Self {
        let help = error.help().map(str::to_owned);
        Self {
            help,
            ..Self::failure(error.to_string())
        }
    }
}

impl From<rtao::client::ClientError> for CliError {
    fn from(error: rtao::client::ClientError) -> Self {
        let help = client_help(&error);
        Self {
            help,
            diagnostic: ordinary_connection(&error).then_some(CliDiagnostic::Connection),
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
