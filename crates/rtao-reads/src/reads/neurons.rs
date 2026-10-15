// Derived from RaoFoundation/subtensor sdk/python/bittensor/reads/neurons.py @ 850f2755bb05a3851f948489f0ab27b0645cebce (Apache-2.0); modified by rtao.
//! Neuron registration reads.

use crate::ReadError;
use rtao_client::{View, ViewExt};
use rtao_primitives::{AccountId, NetUid, Uid};
use rtao_runtime::storage::subtensor_module as st;

/// UID assigned to a hotkey on a subnet, if registered.
pub async fn uid<V: View + ?Sized>(
    view: &V,
    hotkey: AccountId,
    netuid: NetUid,
) -> Result<Option<Uid>, ReadError> {
    Ok(view.storage(&st::UIDS, (netuid, hotkey)).await?.map(Uid))
}
