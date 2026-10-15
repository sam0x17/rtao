// Derived from RaoFoundation/subtensor sdk/python/bittensor/cli/context.py @ 850f2755bb05a3851f948489f0ab27b0645cebce (Apache-2.0); modified by rtao.
// Derived from RaoFoundation/subtensor sdk/python/bittensor/intents/registration.py @ 850f2755bb05a3851f948489f0ab27b0645cebce (Apache-2.0); modified by rtao.
//! Live registration cost disclosure at the application boundary.

use rtao::{
    client::View,
    primitives::{DynBalance, NetUid},
    tx::DynIntent,
};

use crate::error::CliError;

pub(super) struct RegistrationReview {
    pub suffix: String,
    pub cost: String,
    pub notes: [String; 2],
}

pub(super) async fn live<V: View + ?Sized>(
    view: &V,
    intent: &dyn DynIntent,
) -> Result<Option<RegistrationReview>, CliError> {
    let semantic = intent.semantic_intent_dyn();
    let recycled = match semantic.spec().name {
        "root_register" => true,
        "burned_register" => false,
        _ => return Ok(None),
    };
    let netuid = if recycled {
        NetUid::ROOT
    } else {
        let netuid = semantic
            .to_json_dyn()
            .get("netuid")
            .and_then(serde_json::Value::as_u64)
            .and_then(|value| u16::try_from(value).ok())
            .ok_or_else(|| CliError::input("registration review requires a native netuid"))?;
        NetUid(netuid)
    };
    let (burn, lock) = rtao_intents::registration::neuron_registration_split(view, netuid)
        .await
        .map_err(|error| CliError::failure(error.to_string()))?;
    let burn = DynBalance::from(burn);
    let lock = DynBalance::from(lock);
    let lock_part = if lock.rao == 0 {
        "none".into()
    } else {
        lock.to_string()
    };
    let mut cost = format!("{burn} {}", if recycled { "recycled" } else { "burned" });
    if lock.rao != 0 {
        cost.push_str(&format!(" + {lock} locked as collateral"));
    }
    let lock_line = if lock.rao != 0 {
        format!("lock {lock} as miner collateral")
    } else if recycled {
        "lock none".into()
    } else {
        "lock none (full cost is burned)".into()
    };
    Ok(Some(RegistrationReview {
        suffix: format!("burn {burn} · lock {lock_part}"),
        cost,
        notes: [
            format!(
                "burn {burn} ({})",
                if recycled {
                    "recycled into issuance"
                } else {
                    "destroyed"
                }
            ),
            lock_line,
        ],
    }))
}
