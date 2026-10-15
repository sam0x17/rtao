// Derived from RaoFoundation/subtensor sdk/python/bittensor/reads/{delegation,leasing}.py @ 850f2755bb05a3851f948489f0ab27b0645cebce (Apache-2.0); modified by rtao.
// Derived from RaoFoundation/subtensor sdk/python/bittensor/reads/delegation.py @ 850f2755bb05a3851f948489f0ab27b0645cebce (Apache-2.0); modified by rtao.
// Derived from RaoFoundation/subtensor sdk/python/bittensor/reads/leasing.py @ 850f2755bb05a3851f948489f0ab27b0645cebce (Apache-2.0); modified by rtao.
//! Delegate relations, subnet leases and crowdloans at the selected chain state.

use futures::TryStreamExt;
use rtao_client::{View, ViewExt};
use rtao_core::DynValue;
use rtao_primitives::{AccountId, DynBalance, NetUid};
use rtao_runtime::{models, runtime_apis as api, storage as st};
use serde::Serialize;

use super::owner::{DelegateInfo, delegate_info};
use crate::ReadError;

/// Every delegate, preserving the runtime API's order.
// py: sdk/python/bittensor/reads/delegation.py:68-71
pub async fn delegates<V: View + ?Sized>(view: &V) -> Result<Vec<DelegateInfo>, ReadError> {
    Ok(view
        .call(&api::delegate_info_runtime_api::GET_DELEGATES, ())
        .await?
        .into_iter()
        .map(delegate_info)
        .collect())
}

/// One delegate, absent when the runtime API returns `None`.
// py: sdk/python/bittensor/reads/delegation.py:80-83
pub async fn delegate<V: View + ?Sized>(
    view: &V,
    hotkey_ss58: AccountId,
) -> Result<Option<DelegateInfo>, ReadError> {
    Ok(view
        .call(
            &api::delegate_info_runtime_api::GET_DELEGATE,
            (hotkey_ss58,),
        )
        .await?
        .map(delegate_info))
}

/// Whether the delegate API returns a present record for this hotkey.
// py: sdk/python/bittensor/reads/delegation.py:92-95
pub async fn is_delegate<V: View + ?Sized>(
    view: &V,
    hotkey_ss58: AccountId,
) -> Result<bool, ReadError> {
    Ok(view
        .call(
            &api::delegate_info_runtime_api::GET_DELEGATE,
            (hotkey_ss58,),
        )
        .await?
        .is_some())
}

/// Stored child edges, in storage order, with exact u64-normalized proportions.
// py: sdk/python/bittensor/reads/delegation.py:156-166
pub async fn children<V: View + ?Sized>(
    view: &V,
    hotkey_ss58: AccountId,
    netuid: NetUid,
) -> Result<Vec<(u64, AccountId)>, ReadError> {
    Ok(view
        .storage(&st::subtensor_module::CHILD_KEYS, (hotkey_ss58, netuid))
        .await?)
}

/// Stored parent edges, in storage order, with exact u64-normalized proportions.
// py: sdk/python/bittensor/reads/delegation.py:178-188
pub async fn parents<V: View + ?Sized>(
    view: &V,
    hotkey_ss58: AccountId,
    netuid: NetUid,
) -> Result<Vec<(u64, AccountId)>, ReadError> {
    Ok(view
        .storage(&st::subtensor_module::PARENT_KEYS, (hotkey_ss58, netuid))
        .await?)
}

/// Child proposal still in cooldown, including an empty default proposal.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct PendingChildren {
    /// Proposed child edges in stored order.
    pub children: Vec<(u64, AccountId)>,
    /// Block at which the proposal becomes eligible to apply.
    pub cooldown_block: u64,
}

/// Read a child proposal using the reversed `(netuid, hotkey)` storage key.
// py: sdk/python/bittensor/reads/delegation.py:200-215
pub async fn pending_children<V: View + ?Sized>(
    view: &V,
    hotkey_ss58: AccountId,
    netuid: NetUid,
) -> Result<PendingChildren, ReadError> {
    let (children, cooldown_block) = view
        .storage(
            &st::subtensor_module::PENDING_CHILD_KEYS,
            (netuid, hotkey_ss58),
        )
        .await?;
    Ok(PendingChildren {
        children,
        cooldown_block,
    })
}

/// Whether the unit-valued suspension marker is present, even though `()` is empty.
// py: sdk/python/bittensor/reads/delegation.py:224-235
pub async fn childkey_threshold_suspended<V: View + ?Sized>(
    view: &V,
    hotkey_ss58: AccountId,
) -> Result<bool, ReadError> {
    Ok(view
        .storage(
            &st::subtensor_module::CHILDKEY_THRESHOLD_SUSPENDED,
            (hotkey_ss58,),
        )
        .await?
        .is_some())
}

/// Python's lease presentation, with cost in native TAO rao.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Lease {
    /// Runtime lease identifier.
    pub lease_id: u32,
    /// Account receiving the lease's emissions.
    pub beneficiary: AccountId,
    /// Lease owner's coldkey.
    pub coldkey: AccountId,
    /// Lease owner's hotkey.
    pub hotkey: AccountId,
    /// Whole percentage of emissions, from zero through one hundred.
    pub emissions_share: u8,
    /// Expiry block, absent for a perpetual lease.
    pub end_block: Option<u32>,
    /// Leased subnet.
    pub netuid: NetUid,
    /// Native lease cost, denominated in TAO.
    pub cost: DynBalance,
}

fn lease_info(lease_id: u32, value: models::SubnetLease) -> Lease {
    Lease {
        lease_id,
        beneficiary: value.beneficiary,
        coldkey: value.coldkey,
        hotkey: value.hotkey,
        emissions_share: value.emissions_share.0,
        end_block: value.end_block,
        netuid: value.netuid,
        cost: DynBalance::from_rao(value.cost.0, NetUid::ROOT),
    }
}

/// One subnet lease, or `None` for an absent lease ID.
// py: sdk/python/bittensor/reads/leasing.py:12-23,32-39
pub async fn lease<V: View + ?Sized>(view: &V, lease_id: u32) -> Result<Option<Lease>, ReadError> {
    Ok(view
        .storage(&st::subtensor_module::SUBNET_LEASES, (lease_id,))
        .await?
        .map(|value| lease_info(lease_id, value)))
}

fn id_key(keys: &[DynValue], descriptor: &'static str) -> Result<u32, ReadError> {
    if let [key] = keys
        && let Some(id) = key.as_u128().and_then(|id| u32::try_from(id).ok())
    {
        return Ok(id);
    }
    Err(ReadError::SurfaceChanged {
        descriptor,
        detail: "expected exactly one native u32 ID key".into(),
    })
}

/// Every lease, preserving the chain's map iteration order.
// py: sdk/python/bittensor/reads/leasing.py:43-46
pub async fn leases<V: View + ?Sized>(view: &V) -> Result<Vec<Lease>, ReadError> {
    let snapshot = view.pin().await?;
    let rows: Vec<_> = snapshot
        .storage_iter(&st::subtensor_module::SUBNET_LEASES, ())
        .try_collect()
        .await?;
    rows.into_iter()
        .map(|(keys, value)| {
            Ok(lease_info(
                id_key(&keys, "SubtensorModule.SubnetLeases")?,
                value,
            ))
        })
        .collect()
}

/// Python's detailed crowdloan presentation; all exact balances are TAO.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Crowdloan {
    /// Crowdloan creator.
    pub creator: AccountId,
    /// Reserved creator deposit.
    pub deposit: DynBalance,
    /// Minimum accepted contribution.
    pub min_contribution: DynBalance,
    /// Funding cap.
    pub cap: DynBalance,
    /// Contributions raised so far.
    pub raised: DynBalance,
    /// Block at which contributions close.
    pub end: u32,
    /// Whether the crowdloan has finalized.
    pub finalized: bool,
    /// Optional target receiving a successful crowdloan's funds.
    pub target_address: Option<AccountId>,
    /// Account holding contributions; present in every native crowdloan record.
    pub funds_account: Option<AccountId>,
}

fn crowdloan_info(value: models::CrowdloanInfo) -> Crowdloan {
    Crowdloan {
        creator: value.creator,
        deposit: DynBalance::from_rao(value.deposit.0, NetUid::ROOT),
        min_contribution: DynBalance::from_rao(value.min_contribution.0, NetUid::ROOT),
        cap: DynBalance::from_rao(value.cap.0, NetUid::ROOT),
        raised: DynBalance::from_rao(value.raised.0, NetUid::ROOT),
        end: value.end,
        finalized: value.finalized,
        target_address: value.target_address,
        funds_account: Some(value.funds_account),
    }
}

/// One crowdloan, or `None` for an absent crowdloan ID.
// py: sdk/python/bittensor/reads/leasing.py:55-74
pub async fn crowdloan<V: View + ?Sized>(
    view: &V,
    crowdloan_id: u32,
) -> Result<Option<Crowdloan>, ReadError> {
    Ok(view
        .storage(&st::crowdloan::CROWDLOANS, (crowdloan_id,))
        .await?
        .map(crowdloan_info))
}

/// Summary of a crowdloan, retaining Python's floating TAO amount presentation.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct CrowdloanSummary {
    /// Native crowdloan ID used for the numeric output sort.
    pub id: u32,
    /// Crowdloan creator.
    pub creator: AccountId,
    /// Raised amount as a numeric TAO value.
    pub raised_tao: f64,
    /// Funding cap as a numeric TAO value.
    pub cap_tao: f64,
    /// Whether this crowdloan has finalized.
    pub finalized: bool,
}

/// Every crowdloan, sorted by numeric crowdloan ID rather than hashed key order.
// py: sdk/python/bittensor/reads/leasing.py:78-94
pub async fn crowdloans<V: View + ?Sized>(view: &V) -> Result<Vec<CrowdloanSummary>, ReadError> {
    let snapshot = view.pin().await?;
    let rows: Vec<_> = snapshot
        .storage_iter(&st::crowdloan::CROWDLOANS, ())
        .try_collect()
        .await?;
    let mut summaries = rows
        .into_iter()
        .map(|(keys, value)| {
            Ok(CrowdloanSummary {
                id: id_key(&keys, "Crowdloan.Crowdloans")?,
                creator: value.creator,
                raised_tao: DynBalance::from_rao(value.raised.0, NetUid::ROOT).amount(),
                cap_tao: DynBalance::from_rao(value.cap.0, NetUid::ROOT).amount(),
                finalized: value.finalized,
            })
        })
        .collect::<Result<Vec<_>, ReadError>>()?;
    summaries.sort_by_key(|row| row.id);
    Ok(summaries)
}

/// One contributor and their contribution as Python's numeric TAO amount.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct CrowdloanContributor {
    /// Contributing coldkey.
    pub contributor: AccountId,
    /// Contribution in numeric TAO.
    pub amount_tao: f64,
}

/// Contributors to one crowdloan, preserving map order and fixing the ID prefix.
// py: sdk/python/bittensor/reads/leasing.py:103-112
pub async fn crowdloan_contributors<V: View + ?Sized>(
    view: &V,
    crowdloan_id: u32,
) -> Result<Vec<CrowdloanContributor>, ReadError> {
    let snapshot = view.pin().await?;
    let rows: Vec<_> = snapshot
        .storage_iter(&st::crowdloan::CONTRIBUTIONS, (crowdloan_id,))
        .try_collect()
        .await?;
    rows.into_iter()
        .map(|(keys, amount)| {
            let contributor = match keys.as_slice() {
                [key] => key.as_str().and_then(|key| key.parse().ok()),
                _ => None,
            }
            .ok_or_else(|| ReadError::SurfaceChanged {
                descriptor: "Crowdloan.Contributions",
                detail: "expected exactly one free AccountId key".into(),
            })?;
            Ok(CrowdloanContributor {
                contributor,
                amount_tao: DynBalance::from_rao(amount.0, NetUid::ROOT).amount(),
            })
        })
        .collect()
}
