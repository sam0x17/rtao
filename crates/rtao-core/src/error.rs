// Derived from RaoFoundation/subtensor sdk/bittensor-core/src/error.rs @ 850f2755bb05a3851f948489f0ab27b0645cebce (Apache-2.0); modified by rtao.

use std::fmt;

/// Owned error taxonomy at the rtao boundary to the upstream core.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CoreError {
    /// Malformed keyfile or unsupported encryption envelope.
    Keyfile(String),
    /// Password rejected while decrypting a keyfile.
    WrongPassword(String),
    /// Named runtime item absent from the supplied metadata.
    NotInRuntime(String),
    /// SCALE or metadata encoding/decoding failure.
    Codec(String),
    /// Invalid key, signature encoding, or crypto operation.
    Crypto(String),
    /// Hardware signing device failure or user rejection.
    Device(String),
    /// Upstream blocking JSON-RPC failure, preserved for oracle diagnostics.
    Rpc(String),
    /// Upstream semantic transaction policy failure.
    Policy(String),
}

impl From<bittensor_core::CoreError> for CoreError {
    fn from(error: bittensor_core::CoreError) -> Self {
        match error {
            bittensor_core::CoreError::Keyfile(message) => Self::Keyfile(message),
            bittensor_core::CoreError::WrongPassword(message) => Self::WrongPassword(message),
            bittensor_core::CoreError::NotInRuntime(message) => Self::NotInRuntime(message),
            bittensor_core::CoreError::Codec(message) => Self::Codec(message),
            bittensor_core::CoreError::Crypto(message) => Self::Crypto(message),
            bittensor_core::CoreError::Device(message) => Self::Device(message),
            bittensor_core::CoreError::Rpc(message) => Self::Rpc(message),
            bittensor_core::CoreError::Policy(message) => Self::Policy(message),
        }
    }
}

impl fmt::Display for CoreError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Keyfile(message) => write!(formatter, "keyfile error: {message}"),
            Self::WrongPassword(message) => formatter.write_str(message),
            Self::NotInRuntime(message) => write!(formatter, "{message} not found in this runtime"),
            Self::Codec(message) => write!(formatter, "codec error: {message}"),
            Self::Crypto(message) => write!(formatter, "crypto error: {message}"),
            Self::Device(message) => write!(formatter, "device error: {message}"),
            Self::Rpc(message) => write!(formatter, "rpc error: {message}"),
            Self::Policy(message) => write!(formatter, "policy error: {message}"),
        }
    }
}

impl std::error::Error for CoreError {}
