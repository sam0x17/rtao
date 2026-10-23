// Derived from RaoFoundation/subtensor sdk/python/bittensor/_substrate.py @ 850f2755bb05a3851f948489f0ab27b0645cebce (Apache-2.0); modified by rtao.
// Derived from RaoFoundation/subtensor sdk/python/bittensor/_transport/errors.py @ 850f2755bb05a3851f948489f0ab27b0645cebce (Apache-2.0); modified by rtao.
//! Errors at the public asynchronous chain boundary.

use std::{borrow::Cow, fmt};

use rtao_core::CoreError;
use rtao_primitives::H256;
use rtao_rpc::{PolicyRefusal, RpcError, SubmitError};

/// Failure of a client read, connection, or transaction helper.
#[derive(Clone, Debug)]
#[non_exhaustive]
pub enum ClientError {
    /// Durable storage, authorization or unresolved-operation refusal.
    Journal(crate::journal::JournalError),
    /// A typed transport error, with gateway metadata retained.
    Rpc(RpcError),
    /// Metadata or SCALE decoding failed.
    Decode {
        /// Value being decoded.
        what: Cow<'static, str>,
        /// Decoder failure.
        source: CoreError,
    },
    /// The runtime has no supported encoding for this legacy API.
    RuntimeApiUnavailable {
        /// API trait name.
        api: String,
        /// API method name.
        method: String,
    },
    /// The API version does not meet the descriptor's requirement.
    ApiVersion {
        /// API trait name.
        api: String,
        /// Required version.
        need: u32,
        /// Advertised version, or zero when absent.
        have: u32,
    },
    /// A compiled descriptor's shape differs from the live metadata.
    SurfaceChanged {
        /// Stable descriptor identity.
        descriptor: &'static str,
        /// Shape difference.
        detail: String,
    },
    /// Reads remain available, but this client cannot sign safely.
    SigningDisabled {
        /// Failed signing precondition.
        reason: String,
    },
    /// The node did not return a requested block.
    BlockNotFound(H256),
    /// A JSON-RPC response or client configuration was malformed.
    Protocol(String),
    /// The core rejected an operation.
    Core(CoreError),
    /// A signing backend refused or failed to sign the prepared transaction.
    Signing(rtao_signer::SignerError),
    /// Spec-stamped call composition or runtime validation failed.
    Compose(rtao_runtime::ComposeError),
}

impl ClientError {
    /// Semantic code from the pinned Python error taxonomy.
    pub fn code(&self) -> rtao_catalog::ErrorCode {
        use rtao_catalog::ErrorCode;
        match self {
            Self::Journal(_) => ErrorCode::PolicyViolation,
            Self::Rpc(RpcError::Policy(policy)) => {
                rtao_catalog::errors::classify_error(&policy.message, None)
            }
            Self::Rpc(error) => rtao_catalog::errors::classify_error(&error.to_string(), None),
            Self::BlockNotFound(_) => ErrorCode::NotFound,
            Self::SigningDisabled { .. } => ErrorCode::PolicyViolation,
            Self::Compose(_) => ErrorCode::PolicyViolation,
            Self::Protocol(_) => ErrorCode::InvalidArgument,
            Self::ApiVersion { .. } | Self::RuntimeApiUnavailable { .. } => ErrorCode::Disabled,
            Self::Decode { .. }
            | Self::Core(_)
            | Self::Signing(_)
            | Self::SurfaceChanged { .. } => ErrorCode::Internal,
        }
    }
    /// Gateway metadata that callers use to honor retry delays.
    pub fn policy(&self) -> Option<&PolicyRefusal> {
        match self {
            Self::Rpc(RpcError::Policy(policy)) => Some(policy),
            _ => None,
        }
    }

    /// Preserve the transport cause and metadata at the public boundary.
    ///
    /// Display normalizes the public text without rewriting the typed cause.
    pub fn public_rpc(error: RpcError) -> Self {
        Self::Rpc(error)
    }
}

impl fmt::Display for ClientError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Journal(error) => error.fmt(f),
            Self::Rpc(RpcError::Policy(policy)) => fmt::Display::fmt(policy, f),
            Self::Rpc(error) => {
                let detail = error.to_string();
                // Match Python str.strip, including its four C0 separators.
                // The primitive helper is crate-private; this is the same
                // predicate used by this crate's Python config readers.
                let detail = detail.trim_matches(|character: char| {
                    character.is_whitespace() || matches!(character, '\u{1c}'..='\u{1f}')
                });
                let detail = if detail.is_empty() {
                    if matches!(error, RpcError::Timeout { .. }) {
                        "the connection timed out without a response"
                    } else {
                        "the RPC connection failed without an error detail"
                    }
                } else {
                    detail
                };
                f.write_str(detail)
            }
            Self::Decode { what, source } => write!(f, "cannot decode {what}: {source}"),
            Self::RuntimeApiUnavailable { api, method } => {
                write!(f, "runtime API unavailable: {api}.{method}")
            }
            Self::ApiVersion { api, need, have } => {
                write!(f, "runtime API {api} requires version {need}, found {have}")
            }
            Self::SurfaceChanged { descriptor, detail } => {
                write!(f, "surface changed for {descriptor}: {detail}")
            }
            Self::SigningDisabled { reason } => write!(f, "signing disabled: {reason}"),
            Self::BlockNotFound(hash) => {
                write!(f, "Block not found for \"0x{}\"", hex::encode(hash.0))
            }
            Self::Protocol(detail) => f.write_str(detail),
            Self::Core(error) => error.fmt(f),
            Self::Signing(error) => error.fmt(f),
            Self::Compose(error) => error.fmt(f),
        }
    }
}

impl std::error::Error for ClientError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Rpc(error) => Some(error),
            Self::Decode { source, .. } | Self::Core(source) => Some(source),
            Self::Signing(error) => Some(error),
            Self::Compose(error) => Some(error),
            _ => None,
        }
    }
}

impl From<RpcError> for ClientError {
    fn from(error: RpcError) -> Self {
        Self::public_rpc(error)
    }
}

impl From<CoreError> for ClientError {
    fn from(error: CoreError) -> Self {
        Self::Core(error)
    }
}

impl From<rtao_runtime::ComposeError> for ClientError {
    fn from(error: rtao_runtime::ComposeError) -> Self {
        Self::Compose(error)
    }
}

impl From<subxt_rpcs::Error> for ClientError {
    fn from(error: subxt_rpcs::Error) -> Self {
        if let subxt_rpcs::Error::Client(source) = &error {
            if let Some(error) = source.downcast_ref::<RpcError>() {
                return Self::public_rpc(error.clone());
            }
            if let Some(error) = source.downcast_ref::<SubmitError>() {
                return Self::from(match error {
                    SubmitError::Rpc(error) => error.clone(),
                    SubmitError::FateUnknown { hash } => RpcError::FateUnknown { hash: *hash },
                    _ => RpcError::Transport(error.to_string()),
                });
            }
            if let Some(error) = source.downcast_ref::<Self>() {
                return error.clone();
            }
        }
        if let subxt_rpcs::Error::User(error) = &error {
            let data = error
                .data
                .as_ref()
                .and_then(|raw| serde_json::from_str(raw.get()).ok());
            let payload =
                serde_json::json!({"code": error.code,"message":error.message,"data":data});
            if let Some(policy) = rtao_rpc::policy::classify_rpc_error(&payload) {
                return Self::Rpc(RpcError::Policy(Box::new(policy)));
            }
            return Self::Rpc(RpcError::JsonRpc {
                code: error.code.into(),
                message: error.message.clone(),
                data,
                payload: Box::new(payload),
            });
        }
        Self::Rpc(RpcError::Transport(error.to_string()))
    }
}
