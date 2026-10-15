//! Bittensor transaction intents.

#![deny(missing_docs)]

mod arguments;
mod registry;
/// Python's mapping-or-list weight input carrier.
pub use arguments::WeightsArg;
pub use registry::executor;

/// Interfaces and dispatch generated from the immutable Python catalog.
pub mod generated;
pub use generated::*;

/// Feature-scoped parser injected into [`rtao_tx::Executor`].
#[derive(Clone, Copy, Debug, Default)]
#[doc(alias = "bittensor.intents.registry.build")]
pub struct Registry;

impl rtao_tx::IntentRegistry for Registry {
    fn from_tool(
        &self,
        op: &str,
        args: serde_json::Value,
    ) -> Result<Box<dyn rtao_tx::DynIntent>, rtao_tx::IntentError> {
        Ok(AnyIntent::from_tool(op, args)?.into_dyn())
    }
}

#[cfg(feature = "serving")]
pub mod serving;
#[cfg(feature = "transfer")]
pub mod transfer;

#[cfg(feature = "weights")]
pub mod weights;

#[cfg(feature = "hyperparameters")]
pub mod hyperparameters;

#[cfg(feature = "pow")]
pub mod pow;

#[cfg(feature = "crowdloan")]
pub mod crowdloan;
#[cfg(feature = "governance")]
pub mod governance;
#[cfg(feature = "identity")]
pub mod identity;
#[cfg(feature = "registration")]
pub mod registration;
#[cfg(feature = "registration")]
mod registration_pow;
#[cfg(feature = "root")]
pub mod root;

// The generated records contain only infallibly serializable primitive values.
#[allow(clippy::expect_used)]
#[cfg(any(
    feature = "association",
    feature = "batch",
    feature = "children",
    feature = "coldkey",
    feature = "collateral",
    feature = "crowdloan",
    feature = "evm",
    feature = "governance",
    feature = "hyperparameters",
    feature = "identity",
    feature = "leasing",
    feature = "lock",
    feature = "multisig",
    feature = "proxy",
    feature = "registration",
    feature = "root",
    feature = "serving",
    feature = "staking",
    feature = "transfer",
    feature = "weights"
))]
fn args_with_op(op: &str, intent: &impl serde::Serialize) -> serde_json::Value {
    let fields = serde_json::to_value(intent).expect("generated intent fields serialize");
    let mut result = serde_json::Map::new();
    result.insert("op".into(), serde_json::Value::String(op.into()));
    if let serde_json::Value::Object(fields) = fields {
        result.extend(fields);
    }
    serde_json::Value::Object(result)
}

#[cfg(feature = "transfer")]
fn account_argument(account: rtao_primitives::AccountId) -> rtao_core::DynValue {
    rtao_core::DynValue::new(rtao_core::codec::Value::Bytes(account.0.to_vec()))
}

#[cfg(test)]
mod tests;
