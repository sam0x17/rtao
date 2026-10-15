// Derived from RaoFoundation/subtensor sdk/python/bittensor/result.py @ 850f2755bb05a3851f948489f0ab27b0645cebce (Apache-2.0); modified by rtao.
// Derived from RaoFoundation/subtensor sdk/python/bittensor/_substrate.py @ 850f2755bb05a3851f948489f0ab27b0645cebce (Apache-2.0); modified by rtao.
//! Typed views of Python result data and JSON-compatible submission results.

use crate::{ChainError, IntentError};
use rtao_client::{EventRecord, ExtrinsicReport, Inclusion};
use rtao_primitives::{AccountId, Balance, H256, NetUid, Tao};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

/// Typed interpretation of the unchanged operation-specific data map.
pub trait IntentOutput: Send + Sync + 'static + Sized {
    /// Decode only this output's fields, preserving the original data separately.
    fn from_data(data: &Map<String, Value>) -> Result<Self, IntentError>;
}
impl IntentOutput for () {
    fn from_data(_: &Map<String, Value>) -> Result<Self, IntentError> {
        Ok(())
    }
}

/// Result of weight submission or commitment.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct WeightsSubmitted {
    /// Drand reveal round for a timelocked commitment.
    pub reveal_round: Option<u64>,
}
/// Resolved subnet registration details, absent for failed or nonwaiting results.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SubnetRegistered {
    /// Newly registered subnet.
    pub netuid: Option<NetUid>,
    /// Registration mode supplied by Python's completion path.
    #[serde(rename = "registration_mode", alias = "mode")]
    pub mode: Option<String>,
    /// Height at which registration completed.
    pub registered_at_block: Option<u64>,
}
/// A newly created pure proxy.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PureCreated {
    /// Derived proxy account.
    #[serde(rename = "pure_proxy", alias = "pure")]
    pub pure: Option<AccountId>,
    /// Account which created it.
    pub spawner: Option<AccountId>,
    /// Creation height.
    pub height: Option<u64>,
    /// Extrinsic position at creation.
    pub ext_index: Option<u32>,
}
/// Pinned multisig call identity and bytes.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MultisigCall {
    /// Hash agreed by the signatories.
    pub call_hash: Option<H256>,
    /// Exact first-approval call bytes.
    pub call_data: Option<Vec<u8>>,
}
macro_rules! output {($($ty:ty),+)=>{$(impl IntentOutput for $ty {fn from_data(data:&Map<String,Value>)->Result<Self,IntentError> {serde_json::from_value(Value::Object(data.clone())).map_err(IntentError::from)}})+};}
output!(WeightsSubmitted, SubnetRegistered, PureCreated);
impl IntentOutput for MultisigCall {
    fn from_data(data: &Map<String, Value>) -> Result<Self, IntentError> {
        let call_hash = data
            .get("multisig_call_hash")
            .or_else(|| data.get("call_hash"))
            .filter(|value| !value.is_null())
            .map(|value| {
                if let Some(text) = value.as_str() {
                    let bytes = hex::decode(text.strip_prefix("0x").unwrap_or(text))
                        .map_err(|error| IntentError::Validation(error.to_string()))?;
                    let bytes = bytes.try_into().map_err(|_: Vec<u8>| {
                        IntentError::Validation("multisig call hash must contain 32 bytes".into())
                    })?;
                    Ok(H256(bytes))
                } else {
                    serde_json::from_value(value.clone()).map_err(IntentError::from)
                }
            })
            .transpose()?;
        let call_data = data
            .get("multisig_call_data")
            .or_else(|| data.get("call_data"))
            .filter(|value| !value.is_null())
            .map(|value| {
                let text = value.as_str().ok_or_else(|| {
                    IntentError::Validation("multisig call_data is not hexadecimal text".into())
                })?;
                hex::decode(text.strip_prefix("0x").unwrap_or(text))
                    .map_err(|error| IntentError::Validation(error.to_string()))
            })
            .transpose()?;
        Ok(Self {
            call_hash,
            call_data,
        })
    }
}

/// A resolved transaction outcome, including the original Python data map.
#[derive(Clone, Debug)]
pub struct ExtrinsicResult<O = ()> {
    /// Dispatch success or pool acceptance before dispatch.
    pub success: bool,
    /// Human-readable outcome.
    pub message: String,
    /// Inclusion block when known.
    pub block_hash: Option<H256>,
    /// Explorer-style block and position identity.
    pub extrinsic_id: Option<String>,
    /// Public-network explorer URL.
    pub explorer_url: Option<String>,
    /// Actual fee in checked native TAO.
    pub fee: Option<Balance<Tao>>,
    /// Triggered events for successful results; failed facade results omit them.
    pub events: Vec<EventRecord>,
    /// Python-shaped classified chain failure.
    pub error: Option<ChainError>,
    /// Unchanged operation-specific Python data.
    pub data: Map<String, Value>,
    /// Typed view decoded from the same data.
    pub output: O,
    /// Observed pool/inclusion/finality state.
    pub inclusion: Inclusion,
    /// Unknown transmitted fate, distinguished from a definite rejection.
    pub fate_unknown: Option<H256>,
}
impl ExtrinsicResult {
    pub(crate) fn from_report(
        report: ExtrinsicReport,
        network: &str,
        data: Map<String, Value>,
    ) -> Self {
        let error = report
            .error
            .as_ref()
            .map(|error| ChainError::new(report.message.clone(), Some(error.name.clone())));
        let explorer_url = report.extrinsic_id.as_ref().and_then(|id| {
            rtao_catalog::py::EXPLORER_EXTRINSIC_URLS
                .iter()
                .find(|(name, _)| *name == network)
                .map(|(_, template)| template.replace("{id}", id))
        });
        Self {
            success: report.success,
            message: report.message,
            block_hash: report.block_hash,
            extrinsic_id: report.extrinsic_id,
            explorer_url,
            fee: report
                .success
                .then_some(report.fee)
                .flatten()
                .map(|fee| Balance::<Tao>::from_rao(fee.0)),
            events: if report.success {
                report.events
            } else {
                Vec::new()
            },
            error,
            data: if report.success { data } else { Map::new() },
            output: (),
            inclusion: report.inclusion,
            fate_unknown: None,
        }
    }
    pub(crate) fn rejected(error: ChainError, fate_unknown: Option<H256>) -> Self {
        Self {
            success: false,
            message: error.message.clone(),
            block_hash: None,
            extrinsic_id: None,
            explorer_url: None,
            fee: None,
            events: Vec::new(),
            error: Some(error),
            data: Map::new(),
            output: (),
            inclusion: Inclusion::Unobserved,
            fate_unknown,
        }
    }
    /// Decode the typed output from the unchanged data map.
    pub fn with_output<O: IntentOutput>(self) -> Result<ExtrinsicResult<O>, IntentError> {
        let output = O::from_data(&self.data)?;
        Ok(ExtrinsicResult {
            success: self.success,
            message: self.message,
            block_hash: self.block_hash,
            extrinsic_id: self.extrinsic_id,
            explorer_url: self.explorer_url,
            fee: self.fee,
            events: self.events,
            error: self.error,
            data: self.data,
            output,
            inclusion: self.inclusion,
            fate_unknown: self.fate_unknown,
        })
    }
}
impl<O> ExtrinsicResult<O> {
    /// Raise the attached chain error when execution failed.
    pub fn raise_for_failure(self) -> Result<Self, ChainError> {
        if self.success {
            Ok(self)
        } else {
            Err(self.error.clone().unwrap_or_else(|| {
                ChainError::new(
                    if self.message.is_empty() {
                        "Extrinsic failed.".into()
                    } else {
                        self.message.clone()
                    },
                    None,
                )
            }))
        }
    }
    /// Match Python's to_dict field-presence and float-TAO conventions.
    pub fn to_json(&self) -> Value {
        let mut payload = Map::from_iter([
            ("success".into(), json!(self.success)),
            ("message".into(), json!(self.message)),
        ]);
        if let Some(hash) = self.block_hash {
            payload.insert(
                "block_hash".into(),
                json!(format!("0x{}", hex::encode(hash.0))),
            );
        }
        if let Some(id) = self.extrinsic_id.as_ref().filter(|id| !id.is_empty()) {
            payload.insert("extrinsic_id".into(), json!(id));
        }
        if let Some(url) = self.explorer_url.as_ref().filter(|url| !url.is_empty()) {
            payload.insert("explorer_url".into(), json!(url));
        }
        if let Some(fee) = self.fee {
            payload.insert("fee_tao".into(), tao_json(fee));
        }
        if let Some(error) = &self.error {
            payload.insert("error".into(), error.to_json());
        }
        if !self.data.is_empty() {
            payload.insert("data".into(), Value::Object(self.data.clone()));
        }
        Value::Object(payload)
    }
}
pub(crate) fn tao_json(balance: Balance<Tao>) -> Value {
    json!(rtao_primitives::pyfmt::rao_to_f64(balance.rao()))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn typed_outputs_read_python_keys_and_preserve_extra_data() -> Result<(), IntentError> {
        let data = Map::from_iter([
            ("registration_mode".into(), json!("after_deregistration")),
            ("netuid".into(), json!(42)),
            ("registered_at_block".into(), json!(100)),
            ("queued_at_block".into(), json!(90)),
        ]);
        let typed = SubnetRegistered::from_data(&data)?;
        assert_eq!(typed.mode.as_deref(), Some("after_deregistration"));
        assert_eq!(typed.netuid, Some(NetUid(42)));
        assert_eq!(typed.registered_at_block, Some(100));
        let pure = AccountId([1; 32]);
        let typed = PureCreated::from_data(&Map::from_iter([("pure_proxy".into(), json!(pure))]))?;
        assert_eq!(typed.pure, Some(pure));
        assert_eq!(
            WeightsSubmitted::from_data(&Map::new())?,
            WeightsSubmitted::default()
        );
        assert_eq!(
            SubnetRegistered::from_data(&Map::new())?,
            SubnetRegistered::default()
        );
        assert_eq!(PureCreated::from_data(&Map::new())?, PureCreated::default());
        Ok(())
    }
    #[test]
    fn rejected_result_has_only_python_failure_fields_and_retains_native_fate() {
        let failure = ChainError::new("test rejection", None);
        let result = ExtrinsicResult::rejected(failure.clone(), Some(H256([3; 32])));
        assert_eq!(result.inclusion, Inclusion::Unobserved);
        assert_eq!(
            result.to_json(),
            json!({"success":false,"message":"test rejection","error":failure.to_json()})
        );
        assert_eq!(result.raise_for_failure().err(), Some(failure));
    }
    #[test]
    fn multisig_hex_is_checked_without_erasing_original_fields() -> Result<(), IntentError> {
        let data = Map::from_iter([
            ("call_data".into(), json!("0x000102")),
            ("unrelated".into(), json!(9)),
        ]);
        assert_eq!(
            MultisigCall::from_data(&data)?.call_data,
            Some(vec![0, 1, 2])
        );
        assert_eq!(data["unrelated"], json!(9));
        assert!(
            MultisigCall::from_data(&Map::from_iter([("call_data".into(), json!("0x0"))])).is_err()
        );
        Ok(())
    }
    #[test]
    fn multisig_output_reads_python_extras_before_legacy_native_aliases() -> Result<(), IntentError>
    {
        let hash = H256([7; 32]);
        let data = Map::from_iter([
            (
                "multisig_call_hash".into(),
                json!(format!("0x{}", hex::encode(hash.0))),
            ),
            ("multisig_call_data".into(), json!("0x000102")),
            ("call_hash".into(), json!(H256([8; 32]))),
            ("call_data".into(), json!("0xff")),
        ]);
        assert_eq!(
            MultisigCall::from_data(&data)?,
            MultisigCall {
                call_hash: Some(hash),
                call_data: Some(vec![0, 1, 2]),
            }
        );
        assert_eq!(data["multisig_call_data"], "0x000102");
        for value in [json!("0x0"), json!(7)] {
            assert!(
                MultisigCall::from_data(&Map::from_iter([
                    ("multisig_call_data".into(), value),
                    ("call_data".into(), json!("0x00")),
                ]))
                .is_err()
            );
        }
        assert!(
            MultisigCall::from_data(&Map::from_iter([(
                "multisig_call_hash".into(),
                json!("0x01"),
            )]))
            .is_err()
        );
        Ok(())
    }
    #[test]
    fn json_tao_conversion_preserves_correct_rounding_through_native_maximum() {
        for (rao, bits) in [
            (9_007_199_254_740_995, 0x4161_2e0b_e826_d696_u64),
            (u64::MAX, 0x4211_2e0b_e826_d695),
        ] {
            assert_eq!(
                tao_json(Balance::from_rao(rao)).as_f64().map(f64::to_bits),
                Some(bits)
            );
        }
    }
}
