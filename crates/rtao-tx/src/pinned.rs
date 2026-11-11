// Derived from RaoFoundation/subtensor sdk/python/bittensor/_pinned.py @ b6a81d144078eac18bd25c3af7cb31579bb670f3 (Apache-2.0); modified by rtao.
//! Approved dynamic inputs reused while a later multisig round rebuilds a call.
//!
//! A later approval round that must rebuild its call (for example to recover a
//! pure proxy's reserve metadata) would otherwise re-derive prices and `ALL`
//! amounts from current chain state, and the rebuilt bytes would no longer
//! match the opening approval. Within [`scope`](crate::pinned::scope), builds
//! that participate read those inputs from the pinned call instead. This is
//! used only for a rebuild that is then compared byte for byte with the
//! pinned call; it is never permission to accept an imported call or to skip
//! a guard.

use crate::IntentError;
use rtao_runtime::{CallBytes, RuntimeHandle};
use serde_json::{Map, Value};
use std::{
    collections::HashMap,
    future::Future,
    sync::{Arc, Mutex},
};

tokio::task_local! {
    static PINNED: Arc<PinnedInputs>;
}

/// Calls decoded from pinned bytes, in pre-order, with per-call consumption counters.
#[doc(alias = "bittensor._pinned._Inputs")]
#[derive(Debug)]
pub struct PinnedInputs {
    /// `(module, function, named arguments)` in Python's visit order.
    calls: Vec<(String, String, Map<String, Value>)>,
    /// Calls consumed so far for each `(module, function)`.
    indices: Mutex<HashMap<(String, String), usize>>,
}

impl PinnedInputs {
    /// Decode exact pinned call bytes against the signing runtime.
    pub fn decode(call: &CallBytes, runtime: &RuntimeHandle) -> Result<Self, IntentError> {
        let decoded = runtime
            .core()
            .decode_spec(&rtao_core::runtime::TypeSpec::Call, call.as_bytes(), true)
            .map_err(rtao_core::CoreError::from)?;
        Ok(Self::from_decoded(
            &rtao_core::codec::value::to_corpus_json(&decoded),
        ))
    }

    /// Collect every call in a decoded tree, recording each before its children.
    pub fn from_decoded(decoded: &Value) -> Self {
        fn visit(value: &Value, calls: &mut Vec<(String, String, Map<String, Value>)>) {
            match value {
                Value::Object(object) => {
                    if let (Some(module), Some(function)) = (
                        object.get("call_module").and_then(Value::as_str),
                        object.get("call_function").and_then(Value::as_str),
                    ) {
                        let args = object
                            .get("call_args")
                            .and_then(Value::as_array)
                            .map(|args| {
                                args.iter()
                                    .filter_map(|arg| {
                                        Some((
                                            arg.get("name")?.as_str()?.to_owned(),
                                            arg.get("value").cloned().unwrap_or(Value::Null),
                                        ))
                                    })
                                    .collect()
                            })
                            .unwrap_or_default();
                        calls.push((module.to_owned(), function.to_owned(), args));
                    }
                    for child in object.values() {
                        visit(child, calls);
                    }
                }
                Value::Array(items) => {
                    for child in items {
                        visit(child, calls);
                    }
                }
                _ => {}
            }
        }
        let mut calls = Vec::new();
        visit(decoded, &mut calls);
        Self {
            calls,
            indices: Mutex::new(HashMap::new()),
        }
    }

    fn index(&self, module: &str, function: &str) -> usize {
        self.indices
            .lock()
            .map(|indices| {
                indices
                    .get(&(module.to_owned(), function.to_owned()))
                    .copied()
                    .unwrap_or(0)
            })
            .unwrap_or(0)
    }

    /// Position and arguments of the next unconsumed `(module, function)` call.
    fn next(&self, module: &str, function: &str) -> Option<(usize, &Map<String, Value>)> {
        let index = self.index(module, function);
        self.calls
            .iter()
            .enumerate()
            .filter(|(_, (m, f, _))| m == module && f == function)
            .nth(index)
            .map(|(position, (_, _, args))| (position, args))
    }

    /// Arguments of the next approved `(module, function)` call.
    pub fn current(&self, module: &str, function: &str) -> Result<Map<String, Value>, IntentError> {
        self.next(module, function)
            .map(|(_, args)| args.clone())
            .ok_or_else(|| {
                IntentError::validation(
                    "imported multisig call does not match the staking operation",
                )
            })
    }

    /// Consume one approved `(module, function)` call.
    pub fn advance(&self, module: &str, function: &str) {
        if let Ok(mut indices) = self.indices.lock() {
            *indices
                .entry((module.to_owned(), function.to_owned()))
                .or_insert(0) += 1;
        }
    }

    /// The approved branch among `functions`: the one whose next call comes first.
    pub fn choice(
        &self,
        module: &str,
        functions: &[&'static str],
    ) -> Result<&'static str, IntentError> {
        functions
            .iter()
            .filter_map(|function| {
                self.next(module, function)
                    .map(|(position, _)| (position, *function))
            })
            .min()
            .map(|(_, function)| function)
            .ok_or_else(|| {
                IntentError::validation(
                    "imported multisig call does not match the approved operation",
                )
            })
    }
}

/// Run `future` with `inputs` as the approved dynamic inputs of every build it awaits.
#[doc(alias = "bittensor._pinned.pinned_inputs")]
pub async fn scope<F: Future>(inputs: PinnedInputs, future: F) -> F::Output {
    PINNED.scope(Arc::new(inputs), future).await
}

fn active() -> Option<Arc<PinnedInputs>> {
    PINNED.try_with(Arc::clone).ok()
}

/// Whether a pinned rebuild is in progress.
pub fn is_active() -> bool {
    active().is_some()
}

/// Approved arguments for the next `(module, function)` call, when rebuilding.
///
/// Outside a rebuild this is `Ok(None)`. Inside one a missing call refuses.
pub fn current(module: &str, function: &str) -> Result<Option<Map<String, Value>>, IntentError> {
    active()
        .map(|state| state.current(module, function))
        .transpose()
}

/// Consume one `(module, function)` call after composing it in a rebuild.
pub fn advance(module: &str, function: &str) {
    if let Some(state) = active() {
        state.advance(module, function);
    }
}

fn integer(value: &Value) -> Option<u128> {
    match value {
        Value::Number(number) => number
            .as_u64()
            .map(u128::from)
            .or_else(|| number.to_string().parse().ok()),
        Value::Bool(value) => Some(u128::from(*value)),
        _ => None,
    }
}

/// The originally resolved amount for an `all` request, if rebuilding.
#[doc(alias = "bittensor._pinned.approved_amount")]
pub fn approved_amount(
    module: &str,
    function: &str,
    parameter: &str,
) -> Result<Option<u64>, IntentError> {
    let Some(approved) = current(module, function)? else {
        return Ok(None);
    };
    let missing =
        || IntentError::validation("imported multisig call is missing its approved staking amount");
    let value = approved.get(parameter).ok_or_else(missing)?;
    integer(value)
        .and_then(|value| u64::try_from(value).ok())
        .map(Some)
        .ok_or_else(missing)
}

/// [`approved_amount`] for a `SubtensorModule` call.
#[doc(alias = "bittensor._pinned.approved_staking_amount")]
pub fn approved_staking_amount(
    function: &str,
    parameter: &str,
) -> Result<Option<u64>, IntentError> {
    approved_amount("SubtensorModule", function, parameter)
}

/// The approved state-dependent branch among `functions`, if rebuilding.
#[doc(alias = "bittensor._pinned.approved_function")]
pub fn approved_function(
    module: &str,
    functions: &[&'static str],
) -> Result<Option<&'static str>, IntentError> {
    active()
        .map(|state| state.choice(module, functions))
        .transpose()
}

/// An input normally derived from mutable state: the approved value when
/// rebuilding, else `resolve`.
pub async fn derived_u64<F, Fut>(
    approved: Option<&Map<String, Value>>,
    name: &str,
    resolve: F,
) -> Result<u64, IntentError>
where
    F: FnOnce() -> Fut,
    Fut: Future<Output = Result<u64, IntentError>>,
{
    let Some(approved) = approved else {
        return resolve().await;
    };
    approved
        .get(name)
        .and_then(integer)
        .and_then(|value| u64::try_from(value).ok())
        .ok_or_else(|| {
            IntentError::validation("imported multisig call is missing an approved staking input")
        })
}

/// Compose one participating call, freezing only its state-derived inputs.
///
/// `compose` receives the approved arguments while rebuilding (else `None`)
/// and must take explicit arguments from the intent, never from them; only
/// inputs derived from chain state come from the approval (through
/// [`derived_u64`]). The executor then compares the whole rebuilt call,
/// including every guard and origin wrapper, with the pinned bytes.
#[doc(alias = "bittensor._pinned.compose_pinned_call")]
pub async fn compose_pinned<F, Fut>(
    module: &str,
    function: &str,
    compose: F,
) -> Result<CallBytes, IntentError>
where
    F: FnOnce(Option<Map<String, Value>>) -> Fut,
    Fut: Future<Output = Result<CallBytes, IntentError>>,
{
    let approved = current(module, function)?;
    let call = compose(approved).await?;
    advance(module, function);
    Ok(call)
}

/// [`compose_pinned`] for a `SubtensorModule` call.
#[doc(alias = "bittensor._pinned.compose_staking_call")]
pub async fn compose_staking_call<F, Fut>(
    function: &str,
    compose: F,
) -> Result<CallBytes, IntentError>
where
    F: FnOnce(Option<Map<String, Value>>) -> Fut,
    Fut: Future<Output = Result<CallBytes, IntentError>>,
{
    compose_pinned("SubtensorModule", function, compose).await
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;
    use serde_json::json;

    fn call(module: &str, function: &str, args: Value) -> Value {
        let args = args
            .as_object()
            .map(|args| {
                args.iter()
                    .map(|(name, value)| json!({"name": name, "value": value}))
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        json!({"call_index": "0x0000", "call_module": module, "call_function": function, "call_args": args})
    }

    fn batch() -> Value {
        call(
            "Utility",
            "batch_all",
            json!({"calls": [
                call("SubtensorModule", "add_stake_limit", json!({"amount_staked": 7, "limit_price": 11})),
                call("SubtensorModule", "decrease_take", json!({"take": 1})),
                call("SubtensorModule", "add_stake_limit", json!({"amount_staked": 9, "limit_price": 13})),
                call("SubtensorModule", "increase_take", json!({"take": 2})),
            ]}),
        )
    }

    #[tokio::test]
    async fn outside_a_rebuild_nothing_is_approved_and_derived_inputs_resolve() {
        assert!(!is_active());
        assert_eq!(
            approved_staking_amount("add_stake", "amount_staked").unwrap(),
            None
        );
        assert_eq!(
            approved_function("SubtensorModule", &["increase_take"]).unwrap(),
            None
        );
        let value = derived_u64(None, "limit_price", || async { Ok(5) })
            .await
            .unwrap();
        assert_eq!(value, 5);
    }

    #[tokio::test]
    async fn repeated_calls_consume_their_own_approved_inputs_in_order() {
        let inputs = PinnedInputs::from_decoded(&batch());
        scope(inputs, async {
            assert!(is_active());
            for (amount, price) in [(7, 11), (9, 13)] {
                assert_eq!(
                    approved_staking_amount("add_stake_limit", "amount_staked").unwrap(),
                    Some(amount)
                );
                let approved = current("SubtensorModule", "add_stake_limit").unwrap();
                let limit = derived_u64(approved.as_ref(), "limit_price", || async {
                    Err(IntentError::validation(
                        "a rebuild never derives a fresh price",
                    ))
                })
                .await
                .unwrap();
                assert_eq!(limit, price);
                advance("SubtensorModule", "add_stake_limit");
            }
            let error = current("SubtensorModule", "add_stake_limit").unwrap_err();
            assert!(
                error
                    .to_string()
                    .contains("does not match the staking operation")
            );
            let error = approved_staking_amount("remove_stake", "amount_unstaked").unwrap_err();
            assert!(
                error
                    .to_string()
                    .contains("does not match the staking operation")
            );
        })
        .await;
    }

    #[tokio::test]
    async fn approved_branches_follow_call_order_and_missing_inputs_refuse() {
        let inputs = PinnedInputs::from_decoded(&batch());
        scope(inputs, async {
            let functions = ["increase_take", "decrease_take"];
            assert_eq!(
                approved_function("SubtensorModule", &functions).unwrap(),
                Some("decrease_take")
            );
            advance("SubtensorModule", "decrease_take");
            assert_eq!(
                approved_function("SubtensorModule", &functions).unwrap(),
                Some("increase_take")
            );
            advance("SubtensorModule", "increase_take");
            let error = approved_function("SubtensorModule", &functions).unwrap_err();
            assert!(
                error
                    .to_string()
                    .contains("does not match the approved operation")
            );
            let approved = current("SubtensorModule", "add_stake_limit").unwrap();
            let error = derived_u64(approved.as_ref(), "missing", || async { Ok(0) })
                .await
                .unwrap_err();
            assert!(
                error
                    .to_string()
                    .contains("missing an approved staking input")
            );
            let error = approved_staking_amount("add_stake_limit", "missing").unwrap_err();
            assert!(
                error
                    .to_string()
                    .contains("missing its approved staking amount")
            );
        })
        .await;
        assert!(!is_active());
    }
}
