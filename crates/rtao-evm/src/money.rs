// Derived from RaoFoundation/subtensor sdk/python/bittensor/evm/rpc.py @ 850f2755bb05a3851f948489f0ab27b0645cebce (Apache-2.0); modified by rtao.
//! Exact EVM wei to native rao conversion.

use crate::{EvmError, Result};
use alloy_primitives::U256;
use rtao_primitives::{DynBalance, NetUid};

/// EVM base units in one TAO, matching Ethereum's eighteen decimal places.
pub const WEI_PER_TAO: u64 = 1_000_000_000_000_000_000;
/// EVM base units in one native rao.
pub const WEI_PER_RAO: u64 = 1_000_000_000;

/// Convert wei to native TAO, truncating sub-rao dust and checking the u64 domain.
pub fn wei_to_balance(wei: U256) -> Result<DynBalance> {
    let rao = wei / U256::from(WEI_PER_RAO);
    let rao = u64::try_from(rao)
        .map_err(|_| EvmError::invalid("EVM value exceeds the native u64 rao balance range"))?;
    Ok(DynBalance::from_rao(rao, NetUid::ROOT))
}

/// Convert native TAO to wei, refusing subnet alpha balances.
pub fn balance_to_wei(balance: &DynBalance) -> Result<U256> {
    if balance.netuid != NetUid::ROOT {
        return Err(EvmError::invalid(
            "EVM values are TAO; got an alpha balance",
        ));
    }
    Ok(U256::from(balance.rao) * U256::from(WEI_PER_RAO))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn conversions_truncate_only_sub_rao_dust_and_check_native_limits() -> Result<()> {
        let balance = DynBalance::from_rao(1_500_000_000, NetUid::ROOT);
        assert_eq!(
            balance_to_wei(&balance)?,
            U256::from(1_500_000_000_000_000_000u64)
        );
        assert_eq!(wei_to_balance(balance_to_wei(&balance)?)?, balance);
        assert_eq!(wei_to_balance(U256::from(WEI_PER_RAO - 1))?.rao, 0);
        let maximum = balance_to_wei(&DynBalance::from_rao(u64::MAX, NetUid::ROOT))?;
        assert_eq!(
            wei_to_balance(maximum + U256::from(WEI_PER_RAO - 1))?.rao,
            u64::MAX
        );
        assert!(wei_to_balance(maximum + U256::from(WEI_PER_RAO)).is_err());
        assert!(wei_to_balance(U256::MAX).is_err());
        let alpha = DynBalance::from_rao(
            1,
            NetUid::new(1).map_err(|_| EvmError::invalid("fixture netuid"))?,
        );
        assert!(balance_to_wei(&alpha).is_err());
        Ok(())
    }
}
