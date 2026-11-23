// Derived from RaoFoundation/subtensor sdk/python/bittensor/reads/weights.py @ 850f2755bb05a3851f948489f0ab27b0645cebce (Apache-2.0); modified by rtao.
// Derived from RaoFoundation/subtensor sdk/python/bittensor/reads/identity.py @ a68c6e8c65411a842fbf69269dc385aa13041391 (Apache-2.0); modified by rtao.
//! Mechanism weights and delegates.

use crate::ReadError;
use futures::TryStreamExt;
use rtao_client::{View, ViewExt};
use rtao_primitives::{AccountId, MechId, NetUid, NetUidStorageIndex, Uid};
use rtao_runtime::runtime_apis::subnet_registration_runtime_api::GET_MECH_WEIGHT_DELEGATE;
use rtao_runtime::storage::subtensor_module as st;
use std::collections::BTreeMap;

/// Sparse normalized weights, preserving validators with zero-weight rows.
pub type WeightMatrix = BTreeMap<Uid, BTreeMap<Uid, f64>>;

/// Read the account authorized to set a validator's weights on this mechanism.
pub async fn mech_weight_delegate<V: View + ?Sized>(
    view: &V,
    netuid: NetUid,
    mechid: MechId,
    hotkey_ss58: Option<AccountId>,
) -> Result<Option<AccountId>, ReadError> {
    let hotkey = hotkey_ss58.ok_or_else(|| ReadError::InvalidParam {
        name: "hotkey_ss58".into(),
        reason: "a validator hotkey is required".into(),
    })?;
    Ok(view
        .call(&GET_MECH_WEIGHT_DELEGATE, (netuid, mechid, hotkey))
        .await?)
}

/// Normalize each sparse validator weight row by its complete sum.
/// Duplicate targets retain their last value while all values count in the sum.
pub async fn weights<V: View + ?Sized>(
    view: &V,
    netuid: NetUid,
    mechid: MechId,
) -> Result<WeightMatrix, ReadError> {
    let index =
        NetUidStorageIndex::new(netuid, mechid).map_err(|error| ReadError::InvalidParam {
            name: "netuid/mechid".into(),
            reason: error.to_string(),
        })?;
    let snapshot = view.pin().await?;
    let rows: Vec<_> = snapshot
        .storage_iter(&st::WEIGHTS, (index,))
        .try_collect()
        .await?;
    let mut out = BTreeMap::new();
    for (keys, row) in rows {
        let uid = keys
            .last()
            .and_then(rtao_core::DynValue::as_u128)
            .and_then(|uid| u16::try_from(uid).ok())
            .ok_or_else(|| ReadError::SurfaceChanged {
                descriptor: "SubtensorModule.Weights",
                detail: "weight row key is not a UID".into(),
            })?;
        let total: u64 = row.iter().map(|(_, value)| u64::from(*value)).sum();
        let mut normalized = BTreeMap::new();
        if total != 0 {
            for (target, value) in row {
                let value = rtao_primitives::ratio::integer_division(i128::from(value), total)
                    .ok_or_else(|| ReadError::SurfaceChanged {
                        descriptor: "SubtensorModule.Weights",
                        detail: "weight division failed".into(),
                    })?;
                normalized.insert(Uid(target), value);
            }
        }
        out.insert(Uid(uid), normalized);
    }
    Ok(out)
}
