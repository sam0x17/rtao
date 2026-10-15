// Derived from RaoFoundation/subtensor sdk/python/bittensor/reads/hyperparameters.py @ 850f2755bb05a3851f948489f0ab27b0645cebce (Apache-2.0); modified by rtao.
//! Generated scalar hyperparameter entry points.

use crate::ReadError;
use rtao_client::View;
use rtao_primitives::NetUid;

/// Raw chain maximum weight limit (u16), matching the scalar Python read.
pub async fn max_weight_limit<V: View + ?Sized>(
    view: &V,
    netuid: NetUid,
) -> Result<u16, ReadError> {
    crate::generated::interfaces::max_weight_limit(view, Some(netuid)).await
}
/// Minimum number of allowed weights.
pub async fn min_allowed_weights<V: View + ?Sized>(
    view: &V,
    netuid: NetUid,
) -> Result<u16, ReadError> {
    crate::generated::interfaces::min_allowed_weights(view, Some(netuid)).await
}
/// Blocks a newly registered neuron is immune from deregistration.
pub async fn immunity_period<V: View + ?Sized>(view: &V, netuid: NetUid) -> Result<u16, ReadError> {
    crate::generated::interfaces::immunity_period(view, Some(netuid)).await
}
/// Number of epochs in the commit-reveal period.
pub async fn reveal_period<V: View + ?Sized>(view: &V, netuid: NetUid) -> Result<u64, ReadError> {
    crate::generated::interfaces::reveal_period(view, Some(netuid)).await
}
/// Block rate limit for setting weights.
pub async fn weights_rate_limit<V: View + ?Sized>(
    view: &V,
    netuid: NetUid,
) -> Result<u64, ReadError> {
    crate::generated::interfaces::weights_rate_limit(view, Some(netuid)).await
}
