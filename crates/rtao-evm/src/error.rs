use alloy_primitives::B256;
use std::{fmt, path::PathBuf};

/// A local validation, keystore or EVM endpoint failure.
#[derive(Debug)]
pub enum EvmError {
    /// Invalid public input or an unsupported artifact shape.
    Validation(String),
    /// Key derivation, authentication or signing failed without secret diagnostics.
    Crypto(String),
    /// A filesystem operation failed at the specified path.
    Io {
        /// The exact file or directory involved.
        path: PathBuf,
        /// The underlying operating-system error.
        source: std::io::Error,
    },
    /// An EVM endpoint returned a JSON-RPC error.
    Rpc {
        /// The endpoint's JSON-RPC error code.
        code: i64,
        /// The endpoint's public error message.
        message: String,
        /// Structured endpoint error data, if present.
        data: Option<serde_json::Value>,
    },
    /// HTTP or TLS failed before a valid response was obtained.
    Transport(String),
    /// A bounded JSON document could not be decoded.
    Json(serde_json::Error),
    /// An explicitly bounded request or observation deadline elapsed.
    Timeout(String),
    /// Polling ended without a receipt for this transaction.
    ReceiptTimeout(B256),
    /// Submission or receipt observation failed after signing.
    ///
    /// The signed transaction may have reached the endpoint. Recover using this
    /// exact hash; this library never retries the submission automatically.
    UnknownFate {
        /// Keccak-256 of the exact signed transaction bytes.
        hash: B256,
        /// Why submission or subsequent observation could not finish.
        cause: Box<EvmError>,
    },
}

impl EvmError {
    /// Construct an error for public input validation.
    pub fn invalid(message: impl Into<String>) -> Self {
        Self::Validation(message.into())
    }

    /// Return the attempted transaction hash when finality is unresolved.
    pub fn unknown_fate_hash(&self) -> Option<B256> {
        match self {
            Self::UnknownFate { hash, .. } | Self::ReceiptTimeout(hash) => Some(*hash),
            _ => None,
        }
    }
}

impl fmt::Display for EvmError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Validation(message)
            | Self::Crypto(message)
            | Self::Transport(message)
            | Self::Timeout(message) => formatter.write_str(message),
            Self::Json(error) => error.fmt(formatter),
            Self::Io { path, source } => write!(formatter, "EVM file {path:?}: {source}"),
            Self::Rpc { code, message, .. } => write!(formatter, "EVM RPC {code}: {message}"),
            Self::ReceiptTimeout(hash) => write!(formatter, "no receipt for {hash}"),
            Self::UnknownFate { hash, cause } => {
                write!(formatter, "transaction {hash} has unresolved fate: {cause}")
            }
        }
    }
}

impl std::error::Error for EvmError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io { source, .. } => Some(source),
            Self::Json(error) => Some(error),
            Self::UnknownFate { cause, .. } => Some(cause),
            _ => None,
        }
    }
}

impl From<serde_json::Error> for EvmError {
    fn from(error: serde_json::Error) -> Self {
        Self::Json(error)
    }
}
