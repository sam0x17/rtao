// Derived from RaoFoundation/subtensor sdk/python/bittensor/intents/plan.py @ 6a972693ae9580541225bd37696ac2fee26b8ae9 (Apache-2.0); modified by rtao.
//! Reviewable plans retaining exactly the call later submitted.

use rtao_primitives::{AccountId, Balance, Spend, Tao};
use rtao_runtime::CallBytes;
use rtao_signer::SignerRole;
use serde_json::{Map, Value, json};

/// A dry-run preview with immutable composed bytes.
#[derive(Clone, Debug)]
pub struct Plan {
    /// Catalog operation.
    pub op: &'static str,
    /// One-line human summary.
    pub summary: String,
    /// Signing role.
    pub signer: SignerRole,
    /// Public outer signing identity.
    pub signer_address: AccountId,
    /// Best-effort exact fee estimate.
    pub fee: Option<Balance<Tao>>,
    /// Predicted effects.
    pub effects: Vec<String>,
    /// Nonfatal cautions.
    pub warnings: Vec<String>,
    /// Policy and preflight hard stops in reported order.
    pub violations: Vec<String>,
    /// Operation-specific data merged into the outcome.
    pub extras: Map<String, Value>,
    /// Semantic TAO spend.
    pub spend: Spend,
    /// Parsed exact intent arguments, without op.
    pub args: Value,
    /// Structured, JSON-native counterpart of `effects` for intents whose
    /// preview has a fixed shape, such as a before/after vector; empty when
    /// the intent only produces prose.
    pub preview: Map<String, Value>,
    call: CallBytes,
}
impl Plan {
    pub(crate) fn new(
        call: CallBytes,
        op: &'static str,
        summary: String,
        signer: SignerRole,
        signer_address: AccountId,
    ) -> Self {
        Self {
            op,
            summary,
            signer,
            signer_address,
            fee: None,
            effects: Vec::new(),
            warnings: Vec::new(),
            violations: Vec::new(),
            extras: Map::new(),
            spend: Spend::None,
            args: Value::Null,
            preview: Map::new(),
            call,
        }
    }
    /// True when the active policy and preflight permit signing.
    pub fn ok(&self) -> bool {
        self.violations.is_empty()
    }
    /// Borrow the exact runtime-stamped bytes this plan reviews.
    pub fn call(&self) -> &CallBytes {
        &self.call
    }
    /// Python's to_dict shape; signing bytes and extras stay outside it.
    pub fn to_json(&self) -> Value {
        json!({"op":self.op,"summary":self.summary,"signer":self.signer.as_str(),"signer_address":self.signer_address,"args":self.args,"fee_tao":self.fee.map(crate::output::tao_json),"spend_tao":match self.spend {Spend::Bounded(amount)=>Some(crate::output::tao_json(amount)),_=>None},"spend_unbounded":matches!(self.spend,Spend::Unbounded),"effects":self.effects,"warnings":self.warnings,"violations":self.violations,"ok":self.ok(),"preview":self.preview,"docs_url":format!("{}/tx/{}",rtao_catalog::py::DOCS_URL,self.op.replace('_',"-"))})
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ALICE: &str = "5GrwvaEF5zXb26Fz9rcQpDWS57CtERHpNehXCPcNoHGKutQY";

    fn plan() -> Result<Plan, Box<dyn std::error::Error>> {
        let runtime = rtao_runtime::RuntimeHandle::parse(
            include_bytes!("../../../vendor/metadata/finney/478.v15.scale"),
            478,
            1,
            42,
        )?;
        let call = rtao_runtime::calls::system::remark(vec![3, 4, 5]).compose(&runtime)?;
        Ok(Plan::new(
            call,
            "transfer_all",
            "approval sentinel".into(),
            SignerRole::Coldkey,
            ALICE.parse()?,
        ))
    }

    #[test]
    fn approval_json_keeps_complete_python_fields_and_excludes_call_and_extras()
    -> Result<(), Box<dyn std::error::Error>> {
        // Field names and omissions follow pinned plan.py::Plan.to_dict.
        // Exact decimal args and float TAO previews are separate contracts.
        let mut plan = plan()?;
        let call = plan.call().clone();
        plan.fee = Some(Balance::from_rao(125_000_000));
        plan.spend = Spend::Bounded(Balance::from_rao(375_000_000));
        plan.args = json!({"amount_tao":"18446744073.709551615","keep_alive":false});
        plan.effects = vec!["effect sentinel".into()];
        plan.warnings = vec!["warning sentinel".into()];
        plan.violations = vec!["policy sentinel".into()];
        plan.extras
            .insert("private_build_extra".into(), json!([9, 8]));
        assert_eq!(
            plan.to_json(),
            json!({
                "op":"transfer_all",
                "summary":"approval sentinel",
                "signer":"coldkey",
                "signer_address":ALICE,
                "args":{"amount_tao":"18446744073.709551615","keep_alive":false},
                "fee_tao":0.125,
                "spend_tao":0.375,
                "spend_unbounded":false,
                "effects":["effect sentinel"],
                "warnings":["warning sentinel"],
                "violations":["policy sentinel"],
                "ok":false,
                "preview":{},
                "docs_url":"https://www.bittensor.com/docs/tx/transfer-all"
            })
        );
        assert_eq!(plan.call().as_bytes(), call.as_bytes());
        Ok(())
    }

    #[test]
    fn approval_json_distinguishes_absent_zero_and_unbounded_spend()
    -> Result<(), Box<dyn std::error::Error>> {
        let mut plan = plan()?;
        for (spend, spend_tao, spend_unbounded) in [
            (Spend::None, Value::Null, false),
            (Spend::Bounded(Balance::ZERO), json!(0.0), false),
            (Spend::Unbounded, Value::Null, true),
        ] {
            plan.spend = spend;
            assert_eq!(
                plan.to_json(),
                json!({
                    "op":"transfer_all",
                    "summary":"approval sentinel",
                    "signer":"coldkey",
                    "signer_address":ALICE,
                    "args":null,
                    "fee_tao":null,
                    "spend_tao":spend_tao,
                    "spend_unbounded":spend_unbounded,
                    "effects":[],
                    "warnings":[],
                    "violations":[],
                    "ok":true,
                    "preview":{},
                    "docs_url":"https://www.bittensor.com/docs/tx/transfer-all"
                })
            );
        }
        Ok(())
    }
}
