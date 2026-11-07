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
    /// Whether an inspectable composition also contains uninspectable calls.
    fn contains_raw_calls(&self) -> bool {
        false
    }
}

/// Validated safety requirements for a complete intent composition.
///
/// This is independent of call construction: nested wrappers retain their
/// existing dispatch origins and bytes while combining their children's limits.
#[derive(Clone, Debug)]
pub struct IntentSafety {
    /// Outer operation used in policy diagnostics.
    pub op: &'static str,
    /// Aggregate TAO leaving every nested dispatch account.
    pub spend: Spend,
    /// Distinct explicitly touched subnet IDs.
    pub netuids: SmallVec<[NetUid; 2]>,
    /// Whether any operation affects all subnets.
    pub affects_all_subnets: bool,
    /// Whether any operation lacks an inspectable safety contract.
    pub contains_raw_calls: bool,
    /// Whether any operation requires shielded submission.
    pub mev_shield_required: bool,
}

impl IntentSafety {
    /// Combine another child without dropping known limits around raw calls.
    pub fn include(&mut self, child: Self) {
        self.spend = match (self.spend, child.spend) {
            (Spend::Unbounded, _) | (_, Spend::Unbounded) => Spend::Unbounded,
            (Spend::None, spend) | (spend, Spend::None) => spend,
            (Spend::Bounded(left), Spend::Bounded(right)) => left
                .checked_add(right)
                .map(Spend::Bounded)
                .unwrap_or(Spend::Unbounded),
        };
        self.netuids.extend(child.netuids);
        self.netuids.sort_unstable();
        self.netuids.dedup();
        self.affects_all_subnets |= child.affects_all_subnets;
        self.contains_raw_calls |= child.contains_raw_calls;
        self.mev_shield_required |= child.mev_shield_required;
    }
}

impl PolicySubject for IntentSafety {
    fn op(&self) -> &'static str {
        self.op
    }
    fn spend(&self) -> Spend {
        self.spend
    }
    fn touches_netuids(&self) -> SmallVec<[NetUid; 2]> {
        self.netuids.clone()
    }
    fn affects_all_subnets(&self) -> bool {
        self.affects_all_subnets
    }
    fn contains_raw_calls(&self) -> bool {
        self.contains_raw_calls
    }
}
impl<I: Intent> PolicySubject for I {
    fn op(&self) -> &'static str {
        self.semantic_intent().spec().name
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
    fn contains_raw_calls(&self) -> bool {
        // Direct Policy::check callers receive the same nested raw guard as
        // the executor's validated snapshot. Invalid compositions have no
        // inspectable safety contract and require the raw-call opt-in here;
        // execution still rejects their validation error unconditionally.
        self.safety()
            .map_or(true, |safety| safety.contains_raw_calls)
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
        self.check_with_setup_reserve(subject, fee, Balance::from_rao(0))
    }

    /// [`Self::check`] with a permanent account-setup reserve (a protected
    /// pure proxy's registration reserve) counted as spend.
    #[doc(alias = "bittensor.intents.plan.Policy.check")]
    pub fn check_with_setup_reserve(
        &self,
        subject: &dyn PolicySubject,
        fee: Option<Balance<Tao>>,
        reserve: Balance<Tao>,
    ) -> Vec<String> {
        if subject.is_raw() {
            return self.check_raw_call();
        }
        let mut violations = if subject.contains_raw_calls() {
            self.check_raw_call()
        } else {
            Vec::new()
        };
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
            match with_setup_reserve(subject.spend(), reserve) {
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

/// A bounded spend plus a permanent setup reserve; an unbounded spend stays
/// unbounded, and a spend of nothing becomes the reserve (Python's
/// `(spend or Balance(0)) + additional_spend`).
pub(crate) fn with_setup_reserve(spend: Spend, reserve: Balance<Tao>) -> Spend {
    match spend {
        Spend::Unbounded => Spend::Unbounded,
        Spend::None if reserve.rao() == 0 => Spend::None,
        Spend::None => Spend::Bounded(reserve),
        Spend::Bounded(spend) => spend
            .rao()
            .checked_add(reserve.rao())
            .map_or(Spend::Unbounded, |rao| {
                Spend::Bounded(Balance::from_rao(rao))
            }),
    }
}

/// The permanent setup reserve recorded in a composed call's extras.
pub(crate) fn setup_reserve(extras: &serde_json::Map<String, serde_json::Value>) -> Balance<Tao> {
    let rao = |key: &str| {
        extras
            .get(key)
            .and_then(serde_json::Value::as_u64)
            .unwrap_or_default()
    };
    Balance::from_rao(
        rao("hashed_registration_max_deposit_rao")
            .saturating_add(rao("protected_pure_registration_deposit_rao")),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn setup_reserves_count_as_spend_unless_the_spend_is_unbounded() {
        let reserve = Balance::from_rao(5);
        assert_eq!(
            with_setup_reserve(Spend::None, Balance::from_rao(0)),
            Spend::None
        );
        assert_eq!(
            with_setup_reserve(Spend::None, reserve),
            Spend::Bounded(reserve)
        );
        assert_eq!(
            with_setup_reserve(Spend::Bounded(Balance::from_rao(7)), reserve),
            Spend::Bounded(Balance::from_rao(12))
        );
        assert_eq!(
            with_setup_reserve(Spend::Unbounded, reserve),
            Spend::Unbounded
        );
        let extras = serde_json::json!({
            "protected_pure_registration_deposit_rao": 3,
            "hashed_registration_max_deposit_rao": 4,
            "pure_proxy_deposit_rao": 100,
        });
        let extras = extras.as_object().cloned().unwrap_or_default();
        assert_eq!(setup_reserve(&extras).rao(), 7);
        let policy = Policy {
            max_spend: Some(Balance::from_rao(10)),
            ..Default::default()
        };
        let subject = IntentSafety {
            op: "create_pure_proxy",
            spend: Spend::Bounded(Balance::from_rao(8)),
            netuids: SmallVec::new(),
            affects_all_subnets: false,
            contains_raw_calls: false,
            mev_shield_required: false,
        };
        assert!(policy.check(&subject, None).is_empty());
        assert_eq!(
            policy.check_with_setup_reserve(&subject, None, reserve),
            ["spend τ0.000000013 exceeds max_spend_tao τ0.000000010"]
        );
    }

    #[test]
    fn raw_children_keep_known_sibling_caps_and_mandatory_transport_requirements() {
        let mut aggregate = IntentSafety {
            op: "batch",
            spend: Spend::None,
            netuids: SmallVec::new(),
            affects_all_subnets: false,
            contains_raw_calls: true,
            mev_shield_required: false,
        };
        aggregate.include(IntentSafety {
            op: "known_child",
            spend: Spend::Bounded(Balance::from_rao(2)),
            netuids: smallvec::smallvec![NetUid(3), NetUid(3)],
            affects_all_subnets: false,
            contains_raw_calls: false,
            mev_shield_required: true,
        });
        assert_eq!(aggregate.netuids.as_slice(), &[NetUid(3)]);
        assert!(aggregate.mev_shield_required);
        for allow_raw_calls in [false, true] {
            let policy = Policy {
                max_fee: Some(Balance::ZERO),
                max_spend: Some(Balance::from_rao(1)),
                allowed_netuids: Some(BTreeSet::from([NetUid(2)])),
                allow_raw_calls,
            };
            let violations = policy.check(&aggregate, Some(Balance::from_rao(1)));
            assert_eq!(violations.len(), if allow_raw_calls { 3 } else { 4 });
            assert_eq!(
                violations
                    .iter()
                    .any(|message| message.contains("raw call")),
                !allow_raw_calls
            );
            for limit in ["max_fee_tao", "max_spend_tao", "allowed_netuids"] {
                assert!(violations.iter().any(|message| message.contains(limit)));
            }
        }
    }
}
