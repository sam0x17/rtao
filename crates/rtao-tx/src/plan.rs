// Derived from RaoFoundation/subtensor sdk/python/bittensor/intents/plan.py @ 850f2755bb05a3851f948489f0ab27b0645cebce (Apache-2.0); modified by rtao.
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
        json!({"op":self.op,"summary":self.summary,"signer":self.signer.as_str(),"signer_address":self.signer_address,"args":self.args,"fee_tao":self.fee.map(crate::output::tao_json),"spend_tao":match self.spend {Spend::Bounded(amount)=>Some(crate::output::tao_json(amount)),_=>None},"spend_unbounded":matches!(self.spend,Spend::Unbounded),"effects":self.effects,"warnings":self.warnings,"violations":self.violations,"ok":self.ok(),"docs_url":format!("{}/tx/{}",rtao_catalog::py::DOCS_URL,self.op.replace('_',"-"))})
    }
}
