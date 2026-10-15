//! JSON input carriers shared by generated intent fields.

use rtao_primitives::{Ratio, Uid};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Resolve an exact TAO field without permitting alpha currencies or `all`.
#[cfg(any(feature = "crowdloan", feature = "root"))]
pub(crate) fn tao_amount(
    amount: &rtao_primitives::Amount,
) -> Result<rtao_primitives::Balance<rtao_primitives::Tao>, rtao_tx::IntentError> {
    match amount {
        rtao_primitives::Amount::Exact(money) => {
            Ok(money.resolve(rtao_primitives::NetUid::ROOT)?.tao()?)
        }
        rtao_primitives::Amount::All => Err(rtao_primitives::UnitError::AllNotAllowed.into()),
    }
}

/// Python's weights mapping or parallel-list input, decoded through a JSON value.
#[derive(Debug, Clone, PartialEq)]
pub enum WeightsArg {
    /// UID strings and corresponding relative weights, in JSON object order.
    Map(BTreeMap<String, Ratio>),
    /// Relative weights paired with the separate `uids` field.
    List(Vec<Ratio>),
}

impl WeightsArg {
    /// Resolve and validate the two accepted input shapes using Python's wording.
    pub fn as_pairs(
        &self,
        uids: Option<&[Uid]>,
    ) -> Result<(Vec<Uid>, Vec<f64>), rtao_tx::IntentError> {
        match self {
            Self::Map(_) if uids.is_some() => Err(rtao_tx::IntentError::Validation("Pass weights as a {uid: weight} mapping OR as parallel uids/weights lists, not both.".into())),
            Self::Map(values) => {
                let mut pairs = values.iter().map(|(uid,weight)| {
                    let parsed=rtao_primitives::pyfmt::parse_integer(uid).ok_or_else(||rtao_tx::IntentError::Validation(format!("invalid literal for int() with base 10: {}",rtao_primitives::pyfmt::string_repr(uid))))?;
                    u16::try_from(parsed).map(|uid| (Uid(uid), weight.get())).map_err(|_| rtao_tx::IntentError::Validation("UIDs must be in 0..65535.".into()))
                }).collect::<Result<Vec<_>,_>>()?;
                pairs.sort_by_key(|(uid,_)| *uid);
                if pairs.windows(2).any(|pair| pair[0].0 == pair[1].0) { return Err(rtao_tx::IntentError::Validation("Duplicate UIDs in weights; each UID may appear once.".into())); }
                Ok(pairs.into_iter().unzip())
            }
            Self::List(values) => {
                let uids = uids.ok_or_else(|| rtao_tx::IntentError::Validation("Provide weights as a {uid: weight} mapping, or uids and weights as parallel lists.".into()))?;
                let unique: std::collections::BTreeSet<_> = uids.iter().collect();
                if unique.len() != uids.len() { return Err(rtao_tx::IntentError::Validation("Duplicate UIDs in weights; each UID may appear once.".into())); }
                Ok((uids.to_vec(), values.iter().map(|v| v.get()).collect()))
            }
        }
    }
}

impl Serialize for WeightsArg {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            Self::Map(value) => value.serialize(serializer),
            Self::List(value) => value.serialize(serializer),
        }
    }
}
impl<'de> Deserialize<'de> for WeightsArg {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        use serde::de::Error;
        match serde_json::Value::deserialize(deserializer)? {
            serde_json::Value::Object(mut value) => {
                for weight in value.values_mut() {
                    coerce_weight_boolean(weight);
                }
                serde_json::from_value(serde_json::Value::Object(value))
                    .map(Self::Map)
                    .map_err(D::Error::custom)
            }
            serde_json::Value::Array(mut value) => {
                for weight in &mut value {
                    coerce_weight_boolean(weight);
                }
                serde_json::from_value(serde_json::Value::Array(value))
                    .map(Self::List)
                    .map_err(D::Error::custom)
            }
            _ => Err(D::Error::custom("weights must be a UID mapping or list")),
        }
    }
}

// `_as_pairs` applies float(weight), for which Python bools are 0.0 and 1.0.
// Keep this coercion at the weights boundary; other Ratio fields remain numeric.
fn coerce_weight_boolean(value: &mut serde_json::Value) {
    if let serde_json::Value::Bool(boolean) = value {
        *value = serde_json::Value::from(u8::from(*boolean));
    }
}

#[cfg(feature = "weights")]
pub(crate) fn canonical_weights(
    uids: &mut Option<Vec<Uid>>,
    weights: &mut Option<WeightsArg>,
) -> Result<(), rtao_tx::IntentError> {
    let values = weights.as_ref().ok_or_else(|| {
        rtao_tx::IntentError::Validation(
            "Provide weights as a {uid: weight} mapping, or uids and weights as parallel lists."
                .into(),
        )
    })?;
    let (destinations, values) = values.as_pairs(uids.as_deref())?;
    *uids = Some(destinations);
    *weights = Some(WeightsArg::List(
        values
            .into_iter()
            .map(Ratio::new)
            .collect::<Result<Vec<_>, _>>()?,
    ));
    Ok(())
}

#[cfg(feature = "weights")]
pub(crate) fn weights_json(
    op: &str,
    intent: &impl Serialize,
    uids: Option<&[Uid]>,
    weights: Option<&WeightsArg>,
) -> serde_json::Value {
    let mut result = crate::args_with_op(op, intent);
    if let Some(weights) = weights
        && let Ok((uids, values)) = weights.as_pairs(uids)
    {
        result["uids"] = serde_json::json!(uids);
        result["weights"] = serde_json::json!(values);
    }
    result
}

#[cfg(all(test, any(feature = "crowdloan", feature = "root")))]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::tao_amount;
    use rtao_primitives::{Alpha, Amount, Balance, Money, NetUid, Tao, UnitError};
    use rtao_tx::IntentError;

    #[test]
    fn exact_tao_fields_preserve_native_rao_at_both_boundaries() {
        for rao in [0, 1, u64::MAX] {
            let expected = Balance::<Tao>::from_rao(rao);
            for amount in [Amount::from(Money::from_rao(rao)), Amount::from(expected)] {
                let resolved = tao_amount(&amount).unwrap();
                assert_eq!(resolved, expected);
                assert_eq!(resolved.to_wire().0, rao);
            }
        }
    }

    #[test]
    fn exact_tao_fields_refuse_draining_and_alpha() {
        assert!(matches!(
            tao_amount(&Amount::All),
            Err(IntentError::UnitMismatch(UnitError::AllNotAllowed))
        ));
        let alpha = Balance::<Alpha>::new(NetUid(1), 1).unwrap();
        assert!(matches!(
            tao_amount(&alpha.into()),
            Err(IntentError::UnitMismatch(UnitError::UnitMismatch {
                expected: NetUid::ROOT,
                actual: NetUid(1),
            }))
        ));
    }
}
