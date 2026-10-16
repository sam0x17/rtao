// Derived from RaoFoundation/subtensor sdk/python/bittensor/cli/root_helpers.py @ 850f2755bb05a3851f948489f0ab27b0645cebce (Apache-2.0); modified by rtao.
//! Root basket presentation and position helpers.
/// Chain-first basket display pricing and historical fallback.
pub mod basket_index;
/// Generated from the pinned upstream named table by xtask.
pub mod basket_index_data;

use crate::error::CliError;
use rtao::{
    client::{BlockRef, Client},
    primitives::{AccountId, DynBalance, NetUid},
};
use serde::Serialize;
use std::collections::BTreeMap;

/// Shared root position dust cutoff (0.001 TAO).
pub const DUST_POSITION_RAO: u64 = 1_000_000;
/// A coldkey's principal and accrued basket yield on one validator.
#[derive(Clone, Debug, Serialize)]
pub struct RootPosition {
    /// Validator hotkey.
    pub hotkey: AccountId,
    /// Principal on root.
    pub staked: DynBalance,
    /// Realizable unclaimed yield.
    pub accrued: DynBalance,
    /// Local wallet name for an all-wallets view.
    pub wallet: Option<String>,
    /// Owning coldkey for an all-wallets view.
    pub coldkey: Option<AccountId>,
}
impl RootPosition {
    /// Checked native total; never silently wrap or saturate balances.
    pub fn total(&self) -> Result<DynBalance, CliError> {
        self.staked
            .checked_add_rao(self.accrued.rao)
            .map_err(|error| CliError::failure(error.to_string()))
    }
    /// Yield since the last claim relative to principal.
    pub fn return_pct(&self) -> Option<f64> {
        rtao::primitives::ratio::integer_division(i128::from(self.accrued.rao), self.staked.rao)
    }
    /// Python-compatible public position record.
    pub fn as_record(&self) -> Result<serde_json::Value, CliError> {
        let total = self.total()?;
        Ok(
            serde_json::json!({"hotkey":self.hotkey,"wallet":self.wallet,"coldkey":self.coldkey,
            "staked":self.staked,"staked_tao":self.staked.amount(),
            "accrued":self.accrued,"accrued_tao":self.accrued.amount(),
            "return_pct":self.return_pct(),"total":total,"total_tao":total.amount(),"value_tao":total.amount()}),
        )
    }
    /// Whether the checked total falls below the shared dust cutoff.
    pub fn is_dust(&self) -> Result<bool, CliError> {
        Ok(self.total()?.rao < DUST_POSITION_RAO)
    }
}
/// Pin both component reads to one head, retaining accrued-only positions.
pub async fn fetch_root_positions(
    client: &Client,
    owner: AccountId,
) -> Result<Vec<RootPosition>, CliError> {
    let fail = |error: rtao::reads::ReadError| CliError::failure(error.to_string());
    let snapshot = client
        .at(BlockRef::Best)
        .await
        .map_err(|error| CliError::failure(error.to_string()))?;
    let stakes = rtao::reads::reads::m6_staking::stake_for_coldkey(&snapshot, owner)
        .await
        .map_err(fail)?;
    let owed = rtao::reads::reads::m6_root::root_basket_owed_breakdown(&snapshot, owner)
        .await
        .map_err(fail)?;
    // Python dict comprehensions retain the last duplicate, rather than summing.
    let mut rows: BTreeMap<String, RootPosition> = BTreeMap::new();
    for row in stakes
        .into_iter()
        .filter(|row| row.netuid == NetUid::ROOT && row.stake.rao > 0)
    {
        rows.insert(
            row.hotkey.to_string(),
            RootPosition {
                hotkey: row.hotkey,
                staked: row.stake,
                accrued: DynBalance::from_rao(0, NetUid::ROOT),
                wallet: None,
                coldkey: None,
            },
        );
    }
    for row in owed {
        rows.entry(row.hotkey.to_string())
            .or_insert_with(|| RootPosition {
                hotkey: row.hotkey,
                staked: DynBalance::from_rao(0, NetUid::ROOT),
                accrued: DynBalance::from_rao(0, NetUid::ROOT),
                wallet: None,
                coldkey: None,
            })
            .accrued = row.owed_tao;
    }
    Ok(rows.into_values().collect())
}
/// Aggregate wallets in input order, then sort stably by descending total.
pub async fn fetch_all_root_positions(
    client: &Client,
    owners: &[(String, AccountId)],
) -> Result<Vec<RootPosition>, CliError> {
    let mut records = Vec::new();
    for (name, owner) in owners {
        for mut row in fetch_root_positions(client, *owner).await? {
            row.wallet = Some(name.clone());
            row.coldkey = Some(*owner);
            let total = row.total()?.rao;
            records.push((total, row));
        }
    }
    records.sort_by_key(|(total, _)| std::cmp::Reverse(*total));
    Ok(records.into_iter().map(|(_, row)| row).collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn position(staked: u64, accrued: u64) -> RootPosition {
        RootPosition {
            hotkey: AccountId([7; 32]),
            staked: DynBalance::from_rao(staked, NetUid::ROOT),
            accrued: DynBalance::from_rao(accrued, NetUid::ROOT),
            wallet: None,
            coldkey: None,
        }
    }
    #[test]
    fn position_dust_boundary_and_principal_free_yield() -> Result<(), CliError> {
        assert!(position(0, DUST_POSITION_RAO - 1).is_dust()?);
        assert!(!position(0, DUST_POSITION_RAO).is_dust()?);
        assert!(!position(500_000, 500_000).is_dust()?);
        assert_eq!(position(0, 1).return_pct(), None);
        assert_eq!(position(2, 1).return_pct(), Some(0.5));
        let output = position(2, 1).as_record()?;
        assert_eq!(output["total_tao"], serde_json::json!(3e-9));
        assert!(output["coldkey"].is_null());
        Ok(())
    }
    #[test]
    fn native_total_overflow_is_an_error_in_every_presentation_path() {
        let row = position(u64::MAX, 1);
        assert!(row.total().is_err());
        assert!(row.is_dust().is_err());
        assert!(row.as_record().is_err());
    }
    #[test]
    fn public_amounts_and_yield_round_integer_division_once() -> Result<(), CliError> {
        let raw = 9_007_199_254_740_995_u64;
        let row = position(1_000_000_000, raw);
        let expected = f64::from_bits(0x4161_2e0b_e826_d696);
        assert_ne!((raw as f64 / 1e9).to_bits(), expected.to_bits());
        assert_eq!(row.return_pct().map(f64::to_bits), Some(expected.to_bits()));
        assert_eq!(row.as_record()?["accrued_tao"], serde_json::json!(expected));
        Ok(())
    }
}
