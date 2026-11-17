// Derived from RaoFoundation/subtensor sdk/python/bittensor/intents/emission_weights.py @ d967d13588834f4f5aaa8ef4c8fc1a865d581462 (Apache-2.0); modified by rtao.
//! Subnet emission weights: how a subnet splits its alpha emission.
//!
//! Each subnet carries a five-slot vector `[burn, recycle, owner, validator,
//! miner]` of `u64 / u64::MAX` shares. The subnet owner (or root) changes it
//! with `AdminUtils.sudo_set_subnet_emission_weights_v2`, replacing the whole
//! split, including gamma shares. The chain enforces the rules
//! in [`rtao_primitives::emission_weights`] and rate-limits owners to one
//! change per week per subnet (root is exempt; the first change on a subnet is
//! always allowed). Read the current vector and its bounds with the
//! `subnet_emission_weights` read.

use rtao_client::{View, ViewExt};
use rtao_primitives::{AccountId, NetUid, emission_weights as ew, pyfmt::fmt_grouped};
use rtao_reads::reads::subnets::{
    RootPreservationInputs, SubnetEmissionWeightsInfo, emission_weights_rate_limit_key,
    gamma_emission_shares, gamma_emission_targets, root_preservation_inputs,
    subnet_emission_weights_info,
};
use rtao_runtime::storage::subtensor_module as st;
use rtao_tx::{BuildCx, Built, Intent, IntentError, IntentPreflight, PreflightCx};
use serde_json::Value;
use std::collections::BTreeMap;

pub use crate::SetSubnetEmissionWeights;

// py: sdk/python/bittensor/intents/emission_weights.py:52-59
/// `burn 5.00% / owner 18.00% / ...`, omitting a zero burn or recycle.
fn vector_text(weights: &ew::Weights) -> String {
    ew::ADJUSTABLE_SLOTS
        .into_iter()
        .filter(|slot| !matches!(*slot, ew::BURN | ew::RECYCLE) || weights[*slot] != 0)
        .map(|slot| {
            format!(
                "{} {}",
                ew::SLOT_NAMES[slot],
                ew::format_share(weights[slot], 2)
            )
        })
        .collect::<Vec<_>>()
        .join(" / ")
}

fn gamma_text(shares: &BTreeMap<u16, u64>) -> String {
    let text = shares
        .iter()
        .map(|(target, share)| format!("gamma:{target} {}", ew::format_share(*share, 2)))
        .collect::<Vec<_>>()
        .join(" / ");
    if text.is_empty() { "none".into() } else { text }
}

fn pct(value: f64) -> String {
    format!("{:.4}%", value * 100.0)
}

// py: sdk/python/bittensor/intents/emission_weights.py:66-85
fn root_line(root: &ew::RootPreview) -> String {
    let baseline = pct(root.baseline_share);
    let (Some(_), Some(validators_after)) = (root.share_after, root.validators_after) else {
        return format!(
            "root's baseline share here is {baseline} of emission, paid off the top of the validator slot; this validator share is at or below it, so validators would get nothing (the chain rejects)"
        );
    };
    let mut line = format!(
        "root keeps its baseline {baseline} of this subnet's emission (default validator share, global TaoWeight) before and after; alpha validators get {} -> {}",
        pct(root.validators_before),
        pct(validators_after)
    );
    if root.capped_after {
        line.push_str(
            "; the baseline exceeds the validator slot right now, so root takes the whole slot and validators get nothing until the share is raised",
        );
    }
    line
}

fn read_error(error: rtao_reads::ReadError) -> IntentError {
    match error {
        rtao_reads::ReadError::Client(error) => IntentError::Client(error),
        error => IntentError::Chain(rtao_tx::ChainError::with_code(
            error.to_string(),
            None,
            error.code(),
        )),
    }
}

fn invalid(error: ew::EmissionWeightsError) -> IntentError {
    IntentError::Validation(error.to_string())
}

// py: sdk/python/bittensor/intents/emission_weights.py:118-132
/// Normalized `(slot, parts)` updates, in the order given.
///
/// Mirrors Python's constructor checks: slots and values pair up and each
/// slot appears once. Burn and recycle may be
/// nonzero; [`ew::validate`] checks their combined cap.
pub fn updates(intent: &SetSubnetEmissionWeights) -> Result<Vec<(usize, u64)>, IntentError> {
    let gamma = gamma_updates(intent)?;
    if intent.slots.len() != intent.values.len() {
        return Err(IntentError::Validation(format!(
            "slots and values must pair up: got {} slots and {} values",
            intent.slots.len(),
            intent.values.len()
        )));
    }
    if intent.slots.is_empty() && gamma.is_empty() {
        return Err(IntentError::Validation(
            "name at least one slot or gamma share to set".into(),
        ));
    }
    let slots = intent
        .slots
        .iter()
        .map(ew::slot_index_json)
        .collect::<Result<Vec<_>, _>>()
        .map_err(invalid)?;
    let mut seen = [false; 5];
    for slot in &slots {
        if std::mem::replace(&mut seen[*slot], true) {
            return Err(IntentError::Validation(
                "each slot may appear at most once".into(),
            ));
        }
    }
    let values = intent
        .values
        .iter()
        .map(ew::parse_share_json)
        .collect::<Result<Vec<_>, _>>()
        .map_err(invalid)?;
    Ok(slots.into_iter().zip(values).collect())
}

fn gamma_updates(intent: &SetSubnetEmissionWeights) -> Result<BTreeMap<u16, u64>, IntentError> {
    let Value::Object(shares) = &intent.gamma else {
        return Err(IntentError::Validation(
            "gamma must be an object of target shares".into(),
        ));
    };
    let mut parsed = BTreeMap::new();
    for (target, share) in shares {
        if target.is_empty() || !target.bytes().all(|byte| byte.is_ascii_digit()) {
            return Err(IntentError::Validation(format!(
                "gamma target {target:?} is not a netuid"
            )));
        }
        let netuid = target.parse::<u16>().map_err(|_| {
            IntentError::Validation(format!("gamma target {target:?} is not a netuid"))
        })?;
        if parsed
            .insert(netuid, ew::parse_share_json(share).map_err(invalid)?)
            .is_some()
        {
            return Err(IntentError::Validation(
                "each gamma target may appear at most once".into(),
            ));
        }
    }
    Ok(parsed)
}

fn compose_split(
    runtime: &rtao_runtime::RuntimeHandle,
    netuid: NetUid,
    weights: &ew::Weights,
    gamma: &std::collections::BTreeMap<u16, u64>,
) -> Result<rtao_runtime::CallBytes, IntentError> {
    use rtao_core::codec::Value as CoreValue;
    let mut split: Vec<CoreValue> = ["Burn", "Recycle", "Owner", "Validator", "Miner"]
        .into_iter()
        .zip(weights)
        .map(|(slot, share)| {
            CoreValue::Tuple(vec![
                CoreValue::Str(slot.into()),
                CoreValue::Int(i128::from(*share)),
            ])
        })
        .collect();
    split.extend(gamma.iter().map(|(target, share)| {
        CoreValue::Tuple(vec![
            CoreValue::record(vec![(
                "Gamma".into(),
                CoreValue::record(vec![("netuid".into(), CoreValue::Int(i128::from(*target)))]),
            )]),
            CoreValue::Int(i128::from(*share)),
        ])
    }));
    Ok(rtao_runtime::compose_by_name(
        runtime,
        "AdminUtils",
        "sudo_set_subnet_emission_weights_v2",
        &CoreValue::record(vec![
            ("netuid".into(), CoreValue::Int(i128::from(netuid.0))),
            ("split".into(), CoreValue::List(split)),
        ]),
    )?)
}

/// Replace the given slots and shares with Python's normalized integers.
pub(crate) fn canonicalize(intent: &mut SetSubnetEmissionWeights) -> Result<(), IntentError> {
    let updates = updates(intent)?;
    intent.slots = updates.iter().map(|(slot, _)| Value::from(*slot)).collect();
    intent.values = updates
        .iter()
        .map(|(_, value)| Value::from(*value))
        .collect();
    intent.gamma = serde_json::to_value(gamma_updates(intent)?)?;
    Ok(())
}

// py: sdk/python/bittensor/intents/emission_weights.py:144-149
fn merged(
    intent: &SetSubnetEmissionWeights,
    updates: &[(usize, u64)],
    current: Option<&SubnetEmissionWeightsInfo>,
    gamma_share: u64,
) -> Result<ew::Weights, ew::EmissionWeightsError> {
    let base = current.map_or_else(
        || ew::default_weights(ew::DEFAULT_OWNER),
        |info| info.weights,
    );
    if intent.renormalize {
        ew::renormalize_with_gamma(&base, updates, gamma_share)
    } else {
        ew::absorb_rounding(&ew::merge(&base, updates), gamma_share)
    }
}

/// Everything the preview needs, read once at the pinned block.
struct Inputs {
    current: SubnetEmissionWeightsInfo,
    root: RootPreservationInputs,
    last_change_block: u64,
    block: u64,
    owner: Option<AccountId>,
    gamma_before: BTreeMap<u16, u64>,
    gamma_after: BTreeMap<u16, u64>,
    gamma_targets: Option<BTreeMap<u16, bool>>,
    leased: bool,
    /// Sum of the updated gamma shares.
    gamma_share: u64,
}

// py: sdk/python/bittensor/intents/emission_weights.py:164-199
async fn inputs<V: View + ?Sized>(
    view: &V,
    netuid: NetUid,
    gamma_edits: &BTreeMap<u16, u64>,
) -> Result<Option<Inputs>, IntentError> {
    if netuid == NetUid::ROOT {
        return Ok(None);
    }
    let snapshot = view.pin().await?;
    let Some(current) = subnet_emission_weights_info(&snapshot, netuid)
        .await
        .map_err(read_error)?
    else {
        return Ok(None);
    };
    let root = root_preservation_inputs(&snapshot, netuid)
        .await
        .map_err(read_error)?;
    let gamma_before = gamma_emission_shares(&snapshot, netuid)
        .await
        .map_err(read_error)?;
    let gamma_after = ew::merge_gamma(&gamma_before, gamma_edits);
    let gamma_share = gamma_total(&gamma_after)?;
    let gamma_targets = gamma_emission_targets(&snapshot)
        .await
        .map_err(read_error)?;
    let leased = if snapshot
        .block()
        .runtime
        .storage_entry("SubtensorModule", "SubnetUidToLeaseId")
        .is_some()
    {
        snapshot
            .storage(&st::SUBNET_UID_TO_LEASE_ID, (netuid,))
            .await?
            .is_some()
    } else {
        false
    };
    let (last_change_block, owner) = snapshot
        .fetch((
            st::LAST_RATE_LIMITED_BLOCK.at((emission_weights_rate_limit_key(netuid),)),
            st::SUBNET_OWNER.at((netuid,)),
        ))
        .await?;
    Ok(Some(Inputs {
        current,
        root,
        last_change_block,
        block: snapshot.block().number,
        owner: owner.get().copied(),
        gamma_before,
        gamma_after,
        gamma_targets,
        leased,
        gamma_share,
    }))
}

fn gamma_total(shares: &BTreeMap<u16, u64>) -> Result<u64, IntentError> {
    let total: u128 = shares.values().map(|share| u128::from(*share)).sum();
    u64::try_from(total).map_err(|_| IntentError::Validation("gamma shares exceed u64".into()))
}

impl SetSubnetEmissionWeights {
    // py: sdk/python/bittensor/intents/emission_weights.py:219-260
    async fn plan_preview<V: View + ?Sized>(
        &self,
        cx: &PreflightCx<'_, V>,
    ) -> Result<IntentPreflight, IntentError> {
        let summary = self.summary();
        let gamma_edits = gamma_updates(self)?;
        let Some(inputs) = inputs(cx.view, self.netuid, &gamma_edits).await? else {
            return Ok(IntentPreflight {
                effects: vec![summary],
                blocks: vec![format!(
                    "subnet {} does not exist or has no emission vector",
                    self.netuid
                )],
                ..Default::default()
            });
        };
        let updates = updates(self)?;
        let after = match merged(self, &updates, Some(&inputs.current), inputs.gamma_share) {
            Ok(after) => after,
            Err(error) => {
                return Ok(IntentPreflight {
                    effects: vec![summary],
                    blocks: vec![error.to_string()],
                    ..Default::default()
                });
            }
        };
        let baseline_parts = inputs.current.root_baseline_share;
        let mut preview = ew::preview(
            &inputs.current.weights,
            &after,
            &ew::RootInputs {
                baseline_parts,
                global_tao_weight: inputs.root.global_tao_weight,
                root_tao: inputs.root.root_tao,
                alpha_issuance: inputs.root.alpha_issuance,
            },
        );
        let rules = ew::Rules {
            min_validator: Some(ew::min_validator_parts(baseline_parts)),
            min_owner: ew::DEFAULT_OWNER,
            created_via_lease: inputs.current.created_via_lease,
            gamma_share: inputs.gamma_share,
            gamma_targets: inputs.gamma_targets.clone(),
            leased: inputs.leased,
        };
        let mut blocks: Vec<String> = ew::validate(&after, &rules)
            .iter()
            .map(ToString::to_string)
            .collect();
        blocks.extend(
            ew::validate_gamma(self.netuid.0, &inputs.gamma_after, &rules)
                .iter()
                .map(ToString::to_string),
        );
        let shown = compose_split(cx.runtime, self.netuid, &after, &inputs.gamma_after)?;
        if !cx
            .call
            .as_bytes()
            .windows(shown.as_bytes().len())
            .any(|part| part == shown.as_bytes())
        {
            blocks.push(format!(
                "the emission split of subnet {} changed after this call was built, so the call no longer matches this preview; prepare it again",
                self.netuid
            ));
        }
        if let Some(owner) = inputs.owner
            && cx.dispatch_origin != owner
        {
            blocks.push(format!(
                "{} is not the owner of subnet {} (the owner is {owner}); only the owner coldkey, or root through sudo, can change its emission weights",
                cx.dispatch_origin, self.netuid
            ));
        }
        let rate = ew::rate_limit_status(
            inputs.last_change_block,
            inputs.block,
            ew::OWNER_RATE_LIMIT_BLOCKS,
        );
        if !rate.owner_can_change_now {
            let hours = rate.blocks_until_allowed as f64 * ew::BLOCK_SECONDS as f64 / 3600.0;
            blocks.push(format!(
                "the owner changed this vector at block {}; the chain allows the next owner change at block {} (in {} blocks, about {hours:.1} h); root is exempt",
                fmt_grouped(u128::from(rate.last_change_block)),
                fmt_grouped(u128::from(rate.next_allowed_block)),
                fmt_grouped(u128::from(rate.blocks_until_allowed)),
            ));
        }
        preview.rate_limit = Some(rate);
        preview.limits = Some(rules.limits());
        let effects = self.effect_lines(&preview, &inputs.gamma_before, &inputs.gamma_after);
        let mut preview = match serde_json::to_value(&preview)? {
            Value::Object(preview) => preview,
            _ => serde_json::Map::new(),
        };
        preview.insert(
            "gamma".into(),
            serde_json::json!({
                "before": inputs.gamma_before,
                "after": inputs.gamma_after,
            }),
        );
        Ok(IntentPreflight {
            effects,
            blocks,
            preview,
            ..Default::default()
        })
    }

    // py: sdk/python/bittensor/intents/emission_weights.py:262-271
    fn effect_lines(
        &self,
        preview: &ew::Preview,
        gamma_before: &BTreeMap<u16, u64>,
        gamma_after: &BTreeMap<u16, u64>,
    ) -> Vec<String> {
        let (before, after) = (&preview.before.weights, &preview.after.weights);
        let mut lines = vec![
            self.summary(),
            if preview.changed.is_empty() {
                format!("vector: {} (unchanged)", vector_text(before))
            } else {
                format!("vector: {} -> {}", vector_text(before), vector_text(after))
            },
        ];
        if gamma_before != gamma_after {
            lines.push(format!(
                "gamma shares: {} -> {}",
                gamma_text(gamma_before),
                gamma_text(gamma_after)
            ));
            if gamma_before
                .keys()
                .any(|target| !gamma_after.contains_key(target))
            {
                lines.push(
                    "removed gamma shares: their unconverted alpha goes to the miners".into(),
                );
            }
        }
        lines.push(root_line(&preview.root));
        lines
    }
}

impl Intent for SetSubnetEmissionWeights {
    // py: sdk/python/bittensor/intents/emission_weights.py:279-285
    fn summary(&self) -> String {
        let mut parts = match updates(self) {
            Ok(updates) => updates
                .iter()
                .map(|(slot, value)| {
                    format!("{}={}", ew::SLOT_NAMES[*slot], ew::format_share(*value, 2))
                })
                .collect::<Vec<_>>(),
            // Invalid programmatic values are rejected by validate before execution.
            Err(_) => Vec::new(),
        };
        if let Ok(gamma) = gamma_updates(self) {
            parts.extend(
                gamma.iter().map(|(target, share)| {
                    format!("gamma:{target}={}", ew::format_share(*share, 2))
                }),
            );
        }
        let mode = if self.renormalize && self.slots.len() < ew::SLOT_NAMES.len() {
            " (renormalizing the rest)"
        } else {
            ""
        };
        format!(
            "set emission weights on netuid {}: {}{mode}",
            self.netuid,
            parts.join(", ")
        )
    }

    fn touches_netuids(&self) -> smallvec::SmallVec<[NetUid; 2]> {
        let mut touched = smallvec::smallvec![self.netuid];
        if let Ok(gamma) = gamma_updates(self) {
            touched.extend(
                gamma
                    .keys()
                    .copied()
                    .map(NetUid)
                    .filter(|target| *target != self.netuid),
            );
        }
        touched
    }

    // py: sdk/python/bittensor/intents/emission_weights.py:134-162
    async fn build<V: View + ?Sized>(&self, cx: &BuildCx<'_, V>) -> Result<Built, IntentError> {
        let updates = updates(self)?;
        let snapshot = cx.view.pin().await?;
        let current = if self.netuid == NetUid::ROOT {
            None
        } else {
            subnet_emission_weights_info(&snapshot, self.netuid)
                .await
                .map_err(read_error)?
        };
        let before = gamma_emission_shares(&snapshot, self.netuid)
            .await
            .map_err(read_error)?;
        let gamma = ew::merge_gamma(&before, &gamma_updates(self)?);
        let weights =
            merged(self, &updates, current.as_ref(), gamma_total(&gamma)?).map_err(invalid)?;
        Ok(Built::new(compose_split(
            cx.runtime,
            self.netuid,
            &weights,
            &gamma,
        )?))
    }

    async fn effects<V: View + ?Sized>(
        &self,
        cx: &PreflightCx<'_, V>,
    ) -> Result<Vec<String>, IntentError> {
        Ok(self.plan_preview(cx).await?.effects)
    }

    async fn blocks<V: View + ?Sized>(
        &self,
        cx: &PreflightCx<'_, V>,
    ) -> Result<Vec<String>, IntentError> {
        Ok(self.plan_preview(cx).await?.blocks)
    }

    async fn preflight<V: View + ?Sized>(
        &self,
        cx: &PreflightCx<'_, V>,
    ) -> Result<IntentPreflight, IntentError> {
        self.plan_preview(cx).await
    }

    fn validate(&self) -> Result<(), IntentError> {
        updates(self).map(|_| ())
    }

    fn semantic_intent(&self) -> &dyn rtao_tx::DynIntent {
        self
    }
}

#[cfg(test)]
mod adoption_tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn gamma_updates_normalize_targets_and_preserve_zero_removals() {
        let mut intent: SetSubnetEmissionWeights = serde_json::from_value(json!({
            "netuid": 1, "slots": [], "values": [], "gamma": {"03": "5%", "2": 0}
        }))
        .unwrap();
        canonicalize(&mut intent).unwrap();
        assert_eq!(
            intent.gamma,
            json!({"2": 0, "3": ew::parse_share("5%").unwrap()})
        );
        assert!(intent.summary().contains("gamma:2=0.00%, gamma:3=5.00%"));
        let duplicate: SetSubnetEmissionWeights = serde_json::from_value(json!({
            "netuid": 1, "slots": [], "values": [], "gamma": {"03": "5%", "3": "1%"}
        }))
        .unwrap();
        assert!(
            duplicate
                .validate()
                .unwrap_err()
                .to_string()
                .contains("at most once")
        );
        let bad_with_slot: SetSubnetEmissionWeights = serde_json::from_value(json!({
            "netuid": 1, "slots": ["owner"], "values": ["10%"], "gamma": {"-1": "5%"}
        }))
        .unwrap();
        assert!(
            bad_with_slot
                .validate()
                .unwrap_err()
                .to_string()
                .contains("not a netuid")
        );
    }

    #[test]
    fn split_call_preserves_all_slots_and_sorted_gamma_targets_on_491()
    -> Result<(), Box<dyn std::error::Error>> {
        let runtime = rtao_runtime::RuntimeHandle::new(rtao_core::runtime::Runtime::parse(
            include_bytes!("../../../vendor/metadata/testnet/491.v15.scale"),
            491,
            1,
            42,
        )?)?;
        let gamma = std::collections::BTreeMap::from([(9, 17), (3, 11)]);
        let call = compose_split(&runtime, NetUid(7), &[1, 2, 3, 4, 5], &gamma)?;
        // Independent SCALE fixture: pallet/call, u16 netuid, compact count,
        // five unit variants and two Gamma { netuid: u16 } variants.
        let mut expected = vec![
            runtime.pallet_index("AdminUtils").ok_or("pallet")?,
            runtime
                .call_index("AdminUtils", "sudo_set_subnet_emission_weights_v2")
                .ok_or("call")?,
        ];
        expected.extend(7u16.to_le_bytes());
        expected.push(7 << 2);
        for index in 0u8..5 {
            expected.push(index);
            expected.extend(u64::from(index + 1).to_le_bytes());
        }
        for (target, share) in [(3u16, 11u64), (9, 17)] {
            expected.push(5);
            expected.extend(target.to_le_bytes());
            expected.extend(share.to_le_bytes());
        }
        assert_eq!(call.as_bytes(), expected);
        Ok(())
    }
}
