// Derived from RaoFoundation/subtensor sdk/python/bittensor/intents/plan.py @ 850f2755bb05a3851f948489f0ab27b0645cebce (Apache-2.0); modified by rtao.
//! Integer-amount safety limits over semantic intent subjects.

use crate::Intent;
use rtao_primitives::{Balance, NetUid, Spend, Tao};
use smallvec::SmallVec;
use std::collections::BTreeSet;

/// The semantic safety contract consumed by Policy::check.
pub trait PolicySubject: Send + Sync {
    /// Operation name used in Python's violation diagnostics.
    fn op(&self) -> &'static str;
    /// TAO moved or destroyed by this operation.
    fn spend(&self) -> Spend;
    /// Subnets this operation explicitly touches.
    fn touches_netuids(&self) -> SmallVec<[NetUid; 2]>;
    /// Whether the operation spans every subnet.
    fn affects_all_subnets(&self) -> bool;
    /// Whether there is no inspectable intent safety contract.
    fn is_raw(&self) -> bool {
        false
    }
}
impl<I: Intent> PolicySubject for I {
    fn op(&self) -> &'static str {
        I::SPEC.name
    }
    fn spend(&self) -> Spend {
        Intent::spend(self)
    }
    fn touches_netuids(&self) -> SmallVec<[NetUid; 2]> {
        Intent::touches_netuids(self)
    }
    fn affects_all_subnets(&self) -> bool {
        Intent::affects_all_subnets(self)
    }
}

/// Optional limits enforced on the exact reviewed semantic intent.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Policy {
    /// Unavailable estimates also violate a configured fee cap.
    pub max_fee: Option<Balance<Tao>>,
    /// Any unbounded spend violates a configured spend cap.
    pub max_spend: Option<Balance<Tao>>,
    /// Global-subnet operations violate every configured allowlist.
    pub allowed_netuids: Option<BTreeSet<NetUid>>,
    /// Explicit opt-in for calls without an intent safety contract.
    pub allow_raw_calls: bool,
}
impl Policy {
    /// Check exact native rao and preserve Python's violation order and wording.
    pub fn check(&self, subject: &dyn PolicySubject, fee: Option<Balance<Tao>>) -> Vec<String> {
        if subject.is_raw() {
            return self.check_raw_call();
        }
        let mut violations = Vec::new();
        if let Some(max) = self.max_fee {
            let max_label = rtao_primitives::DynBalance::from(max);
            match fee {
                None => violations.push(format!(
                    "max_fee_tao {max_label} is configured but the fee could not be estimated"
                )),
                Some(fee) if fee.rao() > max.rao() => violations.push(format!(
                    "fee {} exceeds max_fee_tao {max_label}",
                    rtao_primitives::DynBalance::from(fee)
                )),
                Some(_) => {}
            }
        }
        if let Some(max) = self.max_spend {
            let max_label = rtao_primitives::DynBalance::from(max);
            match subject.spend() {
            Spend::Unbounded=>violations.push(format!("{} spends an amount that cannot be bounded before execution; max_spend_tao {max_label} blocks it",subject.op())),
            Spend::Bounded(spend) if spend.rao()>max.rao()=>violations.push(format!("spend {} exceeds max_spend_tao {max_label}",rtao_primitives::DynBalance::from(spend))),
            Spend::None|Spend::Bounded(_)=>{}
        }
        }
        if let Some(allowed) = &self.allowed_netuids {
            let text = format!(
                "[{}]",
                allowed
                    .iter()
                    .map(|uid| uid.0.to_string())
                    .collect::<Vec<_>>()
                    .join(", ")
            );
            if subject.affects_all_subnets() {
                violations.push(format!(
                    "{} acts on all subnets, not just allowed_netuids {text}",
                    subject.op()
                ));
            }
            for uid in subject.touches_netuids() {
                if !allowed.contains(&uid) {
                    violations.push(format!("netuid {} not in allowed_netuids {text}", uid.0));
                }
            }
        }
        violations
    }
    /// Python's separate raw-call opt-in gate; fee/spend limits cannot inspect it.
    pub fn check_raw_call(&self) -> Vec<String> {
        if self.allow_raw_calls {
            vec![]
        } else {
            vec!["raw call submission is disabled by policy (set allow_raw_calls=True)".into()]
        }
    }
}
