// Derived from RaoFoundation/subtensor sdk/python/bittensor/intents/emission_weights.py @ 6a972693ae9580541225bd37696ac2fee26b8ae9 (Apache-2.0); modified by rtao.
//! Subnet emission weights: how a subnet splits its alpha emission.
//!
//! Each subnet carries a five-slot vector `[burn, recycle, owner, validator,
//! miner]` of `u64 / u64::MAX` shares. The subnet owner (or root) changes it
//! with `AdminUtils.sudo_set_subnet_emission_weights`, a sparse update: name
//! the slots being changed and their new shares; untouched
//! owner/validator/miner slots keep their value. The chain enforces the rules
//! in [`rtao_primitives::emission_weights`] and rate-limits owners to one
//! change per week per subnet (root is exempt; the first change on a subnet is
//! always allowed). Read the current vector and its bounds with the
//! `subnet_emission_weights` read.

use rtao_client::{View, ViewExt};
use rtao_primitives::{AccountId, NetUid, emission_weights as ew, pyfmt::fmt_grouped};
use rtao_reads::reads::subnets::{
    RootPreservationInputs, SubnetEmissionWeightsInfo, emission_weights_rate_limit_key,
    root_preservation_inputs, subnet_emission_weights_info,
};
use rtao_runtime::{calls::admin_utils, storage::subtensor_module as st};
use rtao_tx::{BuildCx, Built, Intent, IntentError, IntentPreflight, PreflightCx};
use serde_json::Value;

pub use crate::SetSubnetEmissionWeights;

fn slot_label(slot: usize) -> String {
    format!("{} (slot {slot})", ew::SLOT_NAMES[slot])
}

// py: sdk/python/bittensor/intents/emission_weights.py:52-62
fn vector_text(weights: &ew::Weights) -> String {
    ew::ADJUSTABLE_SLOTS
        .into_iter()
        .chain(
            [ew::BURN, ew::RECYCLE]
                .into_iter()
                .filter(|slot| weights[*slot] != 0),
        )
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

fn pct(value: f64) -> String {
    format!("{:.4}%", value * 100.0)
}

// py: sdk/python/bittensor/intents/emission_weights.py:69-88
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

// py: sdk/python/bittensor/intents/emission_weights.py:120-140
/// Normalized `(slot, parts)` updates, in the order given.
///
/// Mirrors Python's constructor checks: slots and values pair up, at least
/// one slot is named, each slot appears once, and burn and recycle stay zero.
pub fn updates(intent: &SetSubnetEmissionWeights) -> Result<Vec<(usize, u64)>, IntentError> {
    if intent.slots.len() != intent.values.len() {
        return Err(IntentError::Validation(format!(
            "slots and values must pair up: got {} slots and {} values",
            intent.slots.len(),
            intent.values.len()
        )));
    }
    if intent.slots.is_empty() {
        return Err(IntentError::Validation(
            "name at least one slot to set".into(),
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
    for (slot, value) in slots.iter().zip(&values) {
        if matches!(*slot, ew::BURN | ew::RECYCLE) && *value != 0 {
            return Err(IntentError::Validation(format!(
                "{} must be 0 in this runtime; it is reserved for a later upgrade",
                slot_label(*slot)
            )));
        }
    }
    Ok(slots.into_iter().zip(values).collect())
}

/// Replace the given slots and shares with Python's normalized integers.
pub(crate) fn canonicalize(intent: &mut SetSubnetEmissionWeights) -> Result<(), IntentError> {
    let updates = updates(intent)?;
    intent.slots = updates.iter().map(|(slot, _)| Value::from(*slot)).collect();
    intent.values = updates
        .iter()
        .map(|(_, value)| Value::from(*value))
        .collect();
    Ok(())
}

fn merged(
    intent: &SetSubnetEmissionWeights,
    updates: &[(usize, u64)],
    current: Option<&SubnetEmissionWeightsInfo>,
) -> Result<ew::Weights, ew::EmissionWeightsError> {
    let base = current.map_or_else(
        || ew::default_weights(ew::DEFAULT_OWNER),
        |info| info.weights,
    );
    if intent.renormalize {
        ew::renormalize(&base, updates)
    } else {
        Ok(ew::merge(&base, updates))
    }
}

/// Everything the preview needs, read once at the pinned block.
struct Inputs {
    current: SubnetEmissionWeightsInfo,
    root: RootPreservationInputs,
    last_change_block: u64,
    block: u64,
    owner: Option<AccountId>,
}

// py: sdk/python/bittensor/intents/emission_weights.py:164-196
async fn inputs<V: View + ?Sized>(view: &V, netuid: NetUid) -> Result<Option<Inputs>, IntentError> {
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
    }))
}

impl SetSubnetEmissionWeights {
    // py: sdk/python/bittensor/intents/emission_weights.py:198-256
    async fn plan_preview<V: View + ?Sized>(
        &self,
        cx: &PreflightCx<'_, V>,
    ) -> Result<IntentPreflight, IntentError> {
        let summary = self.summary();
        let Some(inputs) = inputs(cx.view, self.netuid).await? else {
            return Ok(IntentPreflight {
                effects: vec![summary],
                blocks: vec![format!(
                    "subnet {} does not exist (the root subnet has no emission vector)",
                    self.netuid
                )],
                ..Default::default()
            });
        };
        let updates = updates(self)?;
        let after = match merged(self, &updates, Some(&inputs.current)) {
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
        };
        let mut blocks: Vec<String> = ew::validate(&after, &rules)
            .iter()
            .map(ToString::to_string)
            .collect();
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
        let effects = self.effect_lines(&preview);
        let preview = match serde_json::to_value(&preview)? {
            Value::Object(preview) => preview,
            _ => serde_json::Map::new(),
        };
        Ok(IntentPreflight {
            effects,
            blocks,
            preview,
            ..Default::default()
        })
    }

    // py: sdk/python/bittensor/intents/emission_weights.py:258-267
    fn effect_lines(&self, preview: &ew::Preview) -> Vec<String> {
        let (before, after) = (&preview.before.weights, &preview.after.weights);
        vec![
            self.summary(),
            if preview.changed.is_empty() {
                format!("vector: {} (unchanged)", vector_text(before))
            } else {
                format!("vector: {} -> {}", vector_text(before), vector_text(after))
            },
            root_line(&preview.root),
        ]
    }
}

impl Intent for SetSubnetEmissionWeights {
    // py: sdk/python/bittensor/intents/emission_weights.py:275-281
    fn summary(&self) -> String {
        let parts = match updates(self) {
            Ok(updates) => updates
                .iter()
                .map(|(slot, value)| {
                    format!("{}={}", ew::SLOT_NAMES[*slot], ew::format_share(*value, 2))
                })
                .collect::<Vec<_>>()
                .join(", "),
            // Invalid programmatic values are rejected by validate before execution.
            Err(_) => String::new(),
        };
        let mode = if self.renormalize {
            " (renormalizing the rest)"
        } else {
            ""
        };
        format!(
            "set emission weights on netuid {}: {parts}{mode}",
            self.netuid
        )
    }

    // py: sdk/python/bittensor/intents/emission_weights.py:142-162
    async fn build<V: View + ?Sized>(&self, cx: &BuildCx<'_, V>) -> Result<Built, IntentError> {
        let updates = updates(self)?;
        let (uids, values): (Vec<u16>, Vec<u64>) = if self.renormalize {
            let current = subnet_emission_weights_info(cx.view, self.netuid)
                .await
                .map_err(read_error)?;
            let merged = merged(self, &updates, current.as_ref()).map_err(invalid)?;
            ((0..5).collect(), merged.to_vec())
        } else {
            let mut sorted = updates;
            sorted.sort_unstable_by_key(|(slot, _)| *slot);
            sorted
                .into_iter()
                .map(|(slot, value)| (slot as u16, value))
                .unzip()
        };
        Ok(Built::new(
            admin_utils::sudo_set_subnet_emission_weights(self.netuid, uids, values)
                .compose(cx.runtime)?,
        ))
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
