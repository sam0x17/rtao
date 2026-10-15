// Derived from RaoFoundation/subtensor sdk/python/bittensor/reads/epochs.py @ 850f2755bb05a3851f948489f0ab27b0645cebce (Apache-2.0); modified by rtao.
//! Epoch schedules and signed LastUpdate differences.

use crate::ReadError;
use num_bigint::BigInt;
use num_traits::{Signed, ToPrimitive};
use rtao_client::{View, ViewExt};
use rtao_primitives::{NetUid, NetUidStorageIndex, Uid};
use rtao_runtime::{runtime_apis::subnet_info_runtime_api as api, storage::subtensor_module as st};
use serde::{Deserialize, Serialize};

/// An arbitrary signed list index used locally, without chain encoding.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ListIndex(BigInt);

macro_rules! index_from {
    ($($ty:ty),+)=>{$(impl From<$ty> for ListIndex{fn from(value:$ty)->Self{Self(BigInt::from(value))}})+};
}
index_from!(
    i8, i16, i32, i64, i128, isize, u8, u16, u32, u64, u128, usize
);
impl From<Uid> for ListIndex {
    fn from(value: Uid) -> Self {
        Self::from(value.0)
    }
}
impl std::str::FromStr for ListIndex {
    type Err = num_bigint::ParseBigIntError;
    fn from_str(value: &str) -> Result<Self, Self::Err> {
        value.parse().map(Self)
    }
}
impl Serialize for ListIndex {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let number = self
            .0
            .to_string()
            .parse::<serde_json::Number>()
            .map_err(serde::ser::Error::custom)?;
        number.serialize(serializer)
    }
}
impl<'de> Deserialize<'de> for ListIndex {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let number = serde_json::Number::deserialize(deserializer)?;
        number.to_string().parse().map_err(serde::de::Error::custom)
    }
}

/// The subnet's epoch schedule and remaining time at one pinned block.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct EpochStatus {
    /// Subnet being queried.
    pub netuid: NetUid,
    /// Pinned chain height.
    pub block: u64,
    /// Configured epoch tempo.
    pub tempo: u16,
    /// Current subnet epoch index.
    pub epoch_index: u64,
    /// Height of the last epoch run.
    pub last_epoch_block: u64,
    /// Blocks since the last epoch step.
    pub blocks_since_last_step: u64,
    /// Pending owner-triggered epoch, if nonzero.
    pub pending_epoch_at: Option<u64>,
    /// Chain-computed next epoch start, absent when tempo is zero.
    pub next_epoch_start_block: Option<u64>,
    /// Remaining blocks, clamped to zero for overdue epochs.
    pub blocks_remaining: Option<u64>,
    /// Remaining seconds using the runtime's own slot duration.
    pub seconds_remaining: Option<f64>,
}

/// Blocks since the subnet last ran an epoch step.
pub async fn blocks_since_last_step<V: View + ?Sized>(
    view: &V,
    netuid: NetUid,
) -> Result<u64, ReadError> {
    crate::generated::interfaces::blocks_since_last_step(view, Some(netuid)).await
}

/// Blocks since the neuron last updated weights in mechanism zero.
///
/// A future LastUpdate value produces a signed difference, as in Python.
pub async fn blocks_since_last_update<V: View + ?Sized>(
    view: &V,
    netuid: NetUid,
    uid: Uid,
) -> Result<Option<i128>, ReadError> {
    blocks_since_last_update_index(view, netuid, uid).await
}

/// Dynamic Python index semantics, including negative indices into LastUpdate.
pub async fn blocks_since_last_update_index<V: View + ?Sized>(
    view: &V,
    netuid: NetUid,
    uid: impl Into<ListIndex> + Send,
) -> Result<Option<i128>, ReadError> {
    let snapshot = view.pin().await?;
    let last = snapshot
        .storage(&st::LAST_UPDATE, (NetUidStorageIndex(netuid.0),))
        .await?;
    if last.is_empty() {
        return Ok(None);
    }
    let length = BigInt::from(last.len());
    let uid = uid.into().0;
    if uid >= length {
        return Ok(None);
    }
    let index = if uid.is_negative() { length + uid } else { uid };
    let value = index
        .to_usize()
        .and_then(|index| last.get(index))
        .ok_or_else(|| ReadError::InvalidParam {
            name: "uid".into(),
            reason: "list index out of range".into(),
        })?;
    Ok(Some(
        i128::from(snapshot.block().number) - i128::from(*value),
    ))
}

/// Where the subnet is in its epoch cycle, with every access pinned once.
pub async fn epoch_status<V: View + ?Sized>(
    view: &V,
    netuid: NetUid,
) -> Result<EpochStatus, ReadError> {
    let snapshot = view.pin().await?;
    let ((tempo, last_epoch_block, blocks_since_last_step, pending_at, epoch_index), next, seconds) =
        tokio::try_join!(
            snapshot.fetch((
                st::TEMPO.at((netuid,)),
                st::LAST_EPOCH_BLOCK.at((netuid,)),
                st::BLOCKS_SINCE_LAST_STEP.at((netuid,)),
                st::PENDING_EPOCH_AT.at((netuid,)),
                st::SUBNET_EPOCH_INDEX.at((netuid,))
            )),
            snapshot.call(&api::GET_NEXT_EPOCH_START_BLOCK, (netuid,)),
            snapshot.block_time(),
        )?;
    let remaining = next.map(|next| next.saturating_sub(snapshot.block().number));
    let seconds = super::chain::duration_seconds(seconds);
    Ok(EpochStatus {
        netuid,
        block: snapshot.block().number,
        tempo,
        epoch_index,
        last_epoch_block,
        blocks_since_last_step,
        pending_epoch_at: (pending_at != 0).then_some(pending_at),
        next_epoch_start_block: next,
        blocks_remaining: remaining,
        seconds_remaining: remaining.map(|blocks| blocks as f64 * seconds),
    })
}
