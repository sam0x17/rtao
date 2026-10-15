// Derived from RaoFoundation/subtensor sdk/python/bittensor/namespaces.py @ 850f2755bb05a3851f948489f0ab27b0645cebce (Apache-2.0); modified by rtao.
//! Curated aliases over the catalog's typed asynchronous reads.

use std::collections::BTreeMap;

use rtao_client::View;
use rtao_primitives::{AccountId, DynBalance, NetUid};

use crate::{Balances, Neurons, ReadError, Staking, Subnets};

impl<V: View + ?Sized> Balances<'_, V> {
    /// Free TAO balance of one account.
    pub async fn get(&self, address: AccountId) -> Result<DynBalance, ReadError> {
        super::owner::balance(self.view, address).await
    }

    /// Free TAO balances of the requested accounts, fetched together.
    pub async fn get_many(
        &self,
        addresses: Vec<AccountId>,
    ) -> Result<BTreeMap<AccountId, DynBalance>, ReadError> {
        super::owner::balances(self.view, addresses).await
    }
}

impl<V: View + ?Sized> Staking<'_, V> {
    /// Native stake owned by one coldkey and hotkey on the selected subnet.
    pub async fn get(
        &self,
        coldkey_ss58: AccountId,
        hotkey_ss58: AccountId,
        netuid: NetUid,
    ) -> Result<DynBalance, ReadError> {
        super::m6_staking::stake(self.view, coldkey_ss58, hotkey_ss58, netuid).await
    }

    /// All stake positions owned by the coldkey.
    pub async fn positions(
        &self,
        coldkey_ss58: AccountId,
    ) -> Result<Vec<super::owner::StakePosition>, ReadError> {
        super::m6_staking::stake_for_coldkey(self.view, coldkey_ss58).await
    }
}

impl<V: View + ?Sized> Subnets<'_, V> {
    /// Subnet identity, tempo, registration cost and neuron count.
    pub async fn info(&self, netuid: NetUid) -> Result<super::m6_root::SubnetInfo, ReadError> {
        super::m6_root::subnet(self.view, netuid).await
    }

    /// All live subnets, ordered by native subnet identity.
    pub async fn all(&self) -> Result<Vec<super::m6_root::SubnetInfo>, ReadError> {
        super::m6_root::subnets(self.view).await
    }
}

impl<V: View + ?Sized> Neurons<'_, V> {
    /// All full or lite neurons, ordered by UID.
    pub async fn all(
        &self,
        netuid: NetUid,
        lite: bool,
    ) -> Result<Vec<super::m6_accounts::Neuron>, ReadError> {
        super::m6_accounts::neurons(self.view, netuid, lite).await
    }
}
