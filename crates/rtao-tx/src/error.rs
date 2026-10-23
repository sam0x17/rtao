// Derived from RaoFoundation/subtensor sdk/python/bittensor/result.py @ 850f2755bb05a3851f948489f0ab27b0645cebce (Apache-2.0); modified by rtao.
//! Python's semantic chain errors and the typed executor failure boundary.

use rtao_catalog::ErrorCode;
use rtao_client::{ClientError, PoolRejection};
use rtao_core::CoreError;
use rtao_primitives::UnitError;
use rtao_rpc::{PolicyRefusal, RpcError};
use rtao_signer::SignerError;
use serde_json::{Value, json};
use std::fmt;

/// Classify in Python's name, dispatch, rate-limit and unnamed-text order.
pub fn classify(message: &str, name: Option<&str>) -> ErrorCode {
    rtao_catalog::errors::classify_error(message, name)
}

fn named_error_message(name: &str) -> String {
    rtao_catalog::py::MODULE_ERROR_DOCS
        .iter()
        .rev()
        .find_map(|(candidate, docs)| (*candidate == name && !docs.is_empty()).then_some(*docs))
        .or_else(|| rtao_core::ops::dispatch_error::description(name))
        .unwrap_or(name)
        .to_owned()
}

/// A call failed in preflight, the transaction pool or chain dispatch.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChainError {
    /// Exact human diagnostic.
    pub message: String,
    /// Chain or watch variant, when known.
    pub name: Option<String>,
    /// Semantic error taxonomy.
    pub code: ErrorCode,
    remediation_override: Option<String>,
}
impl ChainError {
    /// Build an error with Python's semantic classifier.
    pub fn new(message: impl Into<String>, name: Option<String>) -> Self {
        let message = message.into();
        let code = classify(&message, name.as_deref());
        Self {
            message,
            name,
            code,
            remediation_override: None,
        }
    }
    /// Retain an explicit semantic code, as Python's optional constructor does.
    pub fn with_code(message: impl Into<String>, name: Option<String>, code: ErrorCode) -> Self {
        Self {
            message: message.into(),
            name,
            code,
            remediation_override: None,
        }
    }
    /// Recommended remedy: exact-name override, substring override, then code.
    pub fn remediation(&self) -> &str {
        if let Some(help) = &self.remediation_override {
            return help;
        }
        if let Some(name) = &self.name
            && let Some((_, help)) = rtao_catalog::py::NAME_HELP_OVERRIDES
                .iter()
                .find(|(candidate, _)| *candidate == name)
        {
            return help;
        }
        let text = self.message.to_lowercase();
        if let Some((_, help)) = rtao_catalog::py::HELP_OVERRIDES
            .iter()
            .find(|(needle, _)| text.contains(needle))
        {
            return help;
        }
        rtao_catalog::py::REMEDIATION
            .iter()
            .find(|(code, _)| *code == self.code)
            .map_or("inspect the message for details", |(_, help)| *help)
    }
    /// Retain an operation-specific remedy, such as funding a multisig signer.
    pub fn with_remediation(mut self, text: impl Into<String>) -> Self {
        self.remediation_override = Some(text.into());
        self
    }
    /// Per-name chain description or dispatch description.
    pub fn description(&self) -> Option<&'static str> {
        let name = self.name.as_deref().filter(|name| !name.is_empty())?;
        rtao_catalog::CHAIN_ERRORS
            .iter()
            .find(|error| error.name == name)
            .map(|error| error.description)
            .or_else(|| rtao_core::ops::dispatch_error::description(name))
    }
    /// Long-form explanation for this semantic code.
    pub fn explanation(&self) -> &'static str {
        rtao_catalog::py::EXPLANATIONS
            .iter()
            .find(|(code, _)| *code == self.code)
            .map_or("", |(_, text)| *text)
    }
    /// Most specific classified docs page, absent for an unknown unnamed error.
    pub fn docs_url(&self) -> Option<String> {
        if let Some(name) = self.name.as_deref().filter(|name| {
            rtao_catalog::errors::NAME_TO_CODE
                .iter()
                .any(|(candidate, _)| *candidate == *name)
        }) {
            return Some(format!(
                "{}/errors/chain/{name}",
                rtao_catalog::py::DOCS_URL
            ));
        }
        (self.code != ErrorCode::Unknown).then(|| {
            format!(
                "{}/errors/{}",
                rtao_catalog::py::DOCS_URL,
                self.code.as_str().replace('_', "-")
            )
        })
    }
    /// Exact Python error-to-dict shape.
    pub fn to_json(&self) -> Value {
        let mut result = json!({"message":self.message,"name":self.name,"code":self.code.as_str(),"remediation":self.remediation()});
        if let Some(description) = self.description().filter(|text| !text.is_empty()) {
            result["description"] = json!(description);
        }
        if let Some(url) = self.docs_url() {
            result["docs_url"] = json!(url);
        }
        result
    }
    /// Map a typed pool/watch rejection into Python's named diagnostic.
    pub fn from_pool(error: &PoolRejection) -> Self {
        // Python's name resolver does not classify a temporary ban. The
        // client recognizes the node's numeric code separately for callers.
        let name = if matches!(error, PoolRejection::TemporarilyBanned) {
            None
        } else {
            error.name().map(str::to_owned)
        };
        let message = name
            .as_deref()
            .map(named_error_message)
            .unwrap_or_else(|| error.to_string());
        Self::new(message, name)
    }
}
impl fmt::Display for ChainError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}
impl std::error::Error for ChainError {}

/// Intent construction, validation, public key defaults or reads failed.
#[derive(Clone, Debug)]
pub enum IntentError {
    /// A ported constructor validation diagnostic.
    Validation(String),
    /// Input fields or tool invocation are invalid.
    InvalidArgument(String),
    /// Explicitly pending catalog operation.
    NotYetPorted(&'static str),
    /// A typed chain/preflight failure.
    Chain(ChainError),
    /// Public chain client failure.
    Client(ClientError),
    /// Public key defaults or signer selection failed.
    Signer(SignerError),
    /// Native money units or range do not match.
    UnitMismatch(UnitError),
    /// Portable core construction failure.
    Core(CoreError),
}
impl IntentError {
    /// Constructor validation error.
    pub fn validation(message: impl Into<String>) -> Self {
        Self::Validation(message.into())
    }
    /// Explicit missing behavior rather than a silently changed call.
    pub fn pending(op: &'static str) -> Self {
        Self::NotYetPorted(op)
    }
    /// Preflight error carrying the chain taxonomy.
    pub fn chain(message: impl Into<String>, name: Option<String>) -> Self {
        Self::Chain(ChainError::new(message, name))
    }
    /// Semantic error code.
    pub fn code(&self) -> ErrorCode {
        match self {
            Self::Validation(_) | Self::InvalidArgument(_) => ErrorCode::InvalidArgument,
            Self::NotYetPorted(_) => ErrorCode::Disabled,
            Self::Chain(error) => error.code,
            Self::Client(error) => error.code(),
            Self::UnitMismatch(_) => ErrorCode::UnitMismatch,
            Self::Signer(_) | Self::Core(_) => ErrorCode::Internal,
        }
    }
}
impl fmt::Display for IntentError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Validation(message) | Self::InvalidArgument(message) => f.write_str(message),
            Self::NotYetPorted(op) => write!(f, "{op}: intent behavior is not yet ported"),
            Self::Chain(error) => error.fmt(f),
            Self::Client(error) => error.fmt(f),
            Self::Signer(error) => error.fmt(f),
            Self::UnitMismatch(error) => error.fmt(f),
            Self::Core(error) => error.fmt(f),
        }
    }
}
impl std::error::Error for IntentError {}
impl From<ClientError> for IntentError {
    fn from(error: ClientError) -> Self {
        Self::Client(error)
    }
}
impl From<SignerError> for IntentError {
    fn from(error: SignerError) -> Self {
        Self::Signer(error)
    }
}
impl From<UnitError> for IntentError {
    fn from(error: UnitError) -> Self {
        Self::UnitMismatch(error)
    }
}
impl From<CoreError> for IntentError {
    fn from(error: CoreError) -> Self {
        Self::Core(error)
    }
}
impl From<rtao_core::codec::Error> for IntentError {
    fn from(error: rtao_core::codec::Error) -> Self {
        Self::Core(error.into())
    }
}
impl From<rtao_runtime::ComposeError> for IntentError {
    fn from(error: rtao_runtime::ComposeError) -> Self {
        Self::Client(error.into())
    }
}
impl From<serde_json::Error> for IntentError {
    fn from(error: serde_json::Error) -> Self {
        Self::InvalidArgument(error.to_string())
    }
}

/// Active policy or preflight blocks refused the reviewed plan.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PolicyError {
    /// Violations in Python's reported order.
    pub violations: Vec<String>,
}
impl PolicyError {
    /// Policy taxonomy.
    pub fn code(&self) -> ErrorCode {
        ErrorCode::PolicyViolation
    }
}
impl fmt::Display for PolicyError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.violations.join("; "))
    }
}
impl std::error::Error for PolicyError {}

/// Failure before or around submission, retaining typed transport metadata.
#[derive(Clone, Debug)]
#[non_exhaustive]
pub enum TxError {
    /// Plan refused by the active policy or hard preflight blocks.
    Policy(PolicyError),
    /// Python-shaped preflight/chain failure.
    Chain(ChainError),
    /// Gateway traffic refusal with retry metadata.
    RpcPolicy(Box<PolicyRefusal>),
    /// Other transport failure.
    Rpc(RpcError),
    /// Connection closed before the operation.
    ConnectionNotReady,
    /// A durable logical operation remains held; no typed success output,
    /// completion hook or replacement may be inferred from its carrier status.
    PendingOperation {
        /// Journal logical identity, retained across application restarts.
        operation: String,
        /// Stable signed pair ordinal.
        pair: u64,
        /// Exact encrypted author request identity.
        carrier_hash: rtao_primitives::H256,
        /// Exact intended signed effect identity.
        inner_hash: rtao_primitives::H256,
        /// Evidence or authorization still required for this operation.
        reason: rtao_client::journal::RecoveryReason,
    },
    /// A submitted operation failed while completing its post-inclusion work.
    PostSubmission {
        /// Original observed receipt, without fabricated completion fields.
        receipt: Box<crate::ExtrinsicResult>,
        /// Primary completion failure; the operation remains an error.
        cause: Box<TxError>,
    },
    /// Intent construction, validation, public key defaults or reads failed.
    Intent(IntentError),
    /// Role selection or private-key unlock failed.
    Signer(SignerError),
    /// Native units/range mismatch.
    UnitMismatch(UnitError),
    /// A runtime descriptor changed shape.
    SurfaceChanged {
        /// Pallet whose call changed.
        pallet: String,
        /// Call name.
        call: String,
        /// Mismatch detail.
        detail: String,
    },
    /// Signed transaction encoding is unavailable or belongs to another chain.
    SigningDisabled {
        /// Failed prerequisite.
        reason: String,
    },
    /// Finality lag leaves a shield's short era unusable.
    ShieldEraExpired {
        /// Shield era period.
        period: u32,
        /// Current finalized lag.
        finality_lag: u64,
    },
}
impl TxError {
    /// Original receipt when post-inclusion work failed.
    pub fn submitted_receipt(&self) -> Option<&crate::ExtrinsicResult> {
        match self {
            Self::PostSubmission { receipt, .. } => Some(receipt),
            _ => None,
        }
    }
    /// Primary error, independent of the retained submission receipt.
    pub fn primary_cause(&self) -> &Self {
        match self {
            Self::PostSubmission { cause, .. } => cause.primary_cause(),
            error => error,
        }
    }
    pub(crate) fn with_submitted_receipt(self, receipt: crate::ExtrinsicResult) -> Self {
        match self {
            error @ Self::PostSubmission { .. } => error,
            cause => Self::PostSubmission {
                receipt: Box::new(receipt),
                cause: Box::new(cause),
            },
        }
    }
    /// Semantic code shared with Python.
    pub fn code(&self) -> ErrorCode {
        match self {
            Self::Policy(_) => ErrorCode::PolicyViolation,
            Self::Chain(error) => error.code,
            Self::RpcPolicy(policy) => classify(&policy.message, None),
            Self::Rpc(error) => classify(&error.to_string(), None),
            Self::ConnectionNotReady | Self::PendingOperation { .. } => ErrorCode::Internal,
            Self::PostSubmission { cause, .. } => cause.code(),
            Self::Intent(error) => error.code(),
            Self::Signer(_) => ErrorCode::Internal,
            Self::UnitMismatch(_) => ErrorCode::UnitMismatch,
            Self::SurfaceChanged { .. } | Self::SigningDisabled { .. } => {
                ErrorCode::PolicyViolation
            }
            Self::ShieldEraExpired { .. } => ErrorCode::Expired,
        }
    }
}
impl fmt::Display for TxError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Policy(error) => error.fmt(f),
            Self::Chain(error) => error.fmt(f),
            Self::RpcPolicy(error) => error.fmt(f),
            Self::Rpc(error) => error.fmt(f),
            Self::ConnectionNotReady => f.write_str("connection is not ready"),
            Self::PendingOperation { pair, .. } => {
                write!(f, "shield pair {pair} remains unresolved")
            }
            Self::PostSubmission { cause, .. } => cause.fmt(f),
            Self::Intent(error) => error.fmt(f),
            Self::Signer(error) => error.fmt(f),
            Self::UnitMismatch(error) => error.fmt(f),
            Self::SurfaceChanged {
                pallet,
                call,
                detail,
            } => write!(f, "surface changed for {pallet}.{call}: {detail}"),
            Self::SigningDisabled { reason } => write!(f, "signing disabled: {reason}"),
            Self::ShieldEraExpired {
                period,
                finality_lag,
            } => write!(
                f,
                "shield period {period} expires with finality lag {finality_lag}"
            ),
        }
    }
}
impl std::error::Error for TxError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::PostSubmission { cause, .. } => Some(cause.as_ref()),
            _ => None,
        }
    }
}
impl From<IntentError> for TxError {
    fn from(error: IntentError) -> Self {
        match error {
            IntentError::Client(error) => error.into(),
            IntentError::Signer(error) => Self::Signer(error),
            IntentError::UnitMismatch(error) => Self::UnitMismatch(error),
            IntentError::Chain(error) => Self::Chain(error),
            error => Self::Intent(error),
        }
    }
}
impl From<ChainError> for TxError {
    fn from(error: ChainError) -> Self {
        Self::Chain(error)
    }
}
impl From<SignerError> for TxError {
    fn from(error: SignerError) -> Self {
        Self::Signer(error)
    }
}
impl From<UnitError> for TxError {
    fn from(error: UnitError) -> Self {
        Self::UnitMismatch(error)
    }
}
impl From<ClientError> for TxError {
    fn from(error: ClientError) -> Self {
        match error {
            ClientError::Rpc(RpcError::Policy(policy)) => Self::RpcPolicy(policy),
            ClientError::Rpc(RpcError::Closed) => Self::ConnectionNotReady,
            ClientError::Rpc(error) => Self::Rpc(error),
            ClientError::Signing(error) => Self::Signer(error),
            ClientError::SigningDisabled { reason } => Self::SigningDisabled { reason },
            ClientError::SurfaceChanged { descriptor, detail } => {
                let (pallet, call) = descriptor.split_once('.').unwrap_or((descriptor, ""));
                Self::SurfaceChanged {
                    pallet: pallet.into(),
                    call: call.into(),
                    detail,
                }
            }
            error => Self::Chain(ChainError::with_code(error.to_string(), None, error.code())),
        }
    }
}
