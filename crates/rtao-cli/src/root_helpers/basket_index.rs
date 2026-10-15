// Derived from RaoFoundation/subtensor sdk/python/bittensor/basket_index.py @ 850f2755bb05a3851f948489f0ab27b0645cebce (Apache-2.0); modified by rtao.
//! Chain-first index-spliced basket display pricing. Monetary inputs remain
//! native checked balances; only display ratios use floating point.
use super::basket_index_data::{BASELINES, INDEX, TR_INDEX};
pub use super::basket_index_data::{EPOCH_BLOCK, GENERATED_BLOCK};
use rtao::reads::reads::owner::{ChainPricing, ValidatorBasketSummary};
use serde::Serialize;

/// Frozen or provisional divisor for a fund.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DisplayPricing {
    /// Raw beta price divisor.
    pub baseline: f64,
    /// First sample block, absent for unknown funds.
    pub first_block: Option<u64>,
    /// Whether the fund is missing from the frozen table.
    pub provisional: bool,
    /// Accrual rate at first sighting.
    pub baseline_rate: f64,
}
/// Inputs needed by the Python fallback, independent of presentation.
#[derive(Clone, Debug)]
pub struct Fund {
    /// Validator address.
    pub hotkey: String,
    /// Realizable NAV, in TAO.
    pub nav_tao: f64,
    /// Spot NAV, in TAO.
    pub spot_nav_tao: f64,
    /// Realizable TAO per beta.
    pub beta_price_tao: f64,
    /// Optional cumulative entitlement rate.
    pub basket_rate: Option<f64>,
    /// Position beta, when pricing a position.
    pub beta: Option<f64>,
    /// Runtime canonical pricing when available.
    pub chain_pricing: Option<ChainPricing>,
}
impl From<&ValidatorBasketSummary> for Fund {
    fn from(row: &ValidatorBasketSummary) -> Self {
        Self {
            hotkey: row.hotkey.to_string(),
            nav_tao: row.nav_tao.amount(),
            spot_nav_tao: row.spot_nav_tao.amount(),
            beta_price_tao: row.beta_price_tao,
            basket_rate: Some(row.basket_rate),
            beta: None,
            chain_pricing: row.chain_pricing.clone(),
        }
    }
}
/// Display fields added to each basket record.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Normalized {
    /// Index-spliced beta price.
    pub display_price_tao: f64,
    /// Rescaled position beta, omitted without a position.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub display_beta: Option<f64>,
    /// Relative bag-index performance.
    pub vs_index: f64,
    /// First sampled block.
    pub index_first_block: Option<u64>,
    /// Unknown-baseline marker.
    pub index_provisional: bool,
    /// Bag index used in this comparison.
    pub basket_index: f64,
    /// Pending entitlement mark.
    pub staker_yield: Option<f64>,
    /// Wealth of one TAO staked at first sighting.
    pub stake_value: Option<f64>,
    /// Relative stake-index performance.
    pub stake_vs_index: Option<f64>,
    /// Stake index used in this comparison.
    pub stake_index: f64,
    /// Canonical time-weighted return, available from the chain only.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub staker_twr: Option<f64>,
}
fn series_level(series: &[(u64, f64)], block: Option<u64>) -> f64 {
    match block {
        None => series.last().map_or(1.0, |sample| sample.1),
        Some(block) => series
            .iter()
            .take_while(|sample| sample.0 <= block)
            .last()
            .map_or(1.0, |sample| sample.1),
    }
}
/// Frozen basket index at or before the requested block.
pub fn index_level(block: Option<u64>) -> f64 {
    series_level(INDEX, block)
}
/// Frozen total-return index at or before the requested block.
pub fn tr_index_level(block: Option<u64>) -> f64 {
    series_level(TR_INDEX, block)
}
/// Frozen divisor or a provisional divisor anchored to the latest index.
pub fn fund_pricing(hotkey: &str, raw_price: f64) -> DisplayPricing {
    if let Some((_, baseline, first, rate)) = BASELINES.iter().find(|row| row.0 == hotkey) {
        return DisplayPricing {
            baseline: *baseline,
            first_block: Some(*first),
            provisional: false,
            baseline_rate: *rate,
        };
    }
    let level = index_level(None);
    let baseline = if level != 0.0 {
        raw_price / level
    } else {
        raw_price
    };
    DisplayPricing {
        baseline: if baseline == 0.0 { 1.0 } else { baseline },
        first_block: None,
        provisional: true,
        baseline_rate: 0.0,
    }
}
/// Spot price removes the size-dependent full-redemption haircut.
pub fn spot_beta_price(row: &Fund) -> f64 {
    if row.nav_tao > 0.0 && row.spot_nav_tao > 0.0 {
        row.beta_price_tao * (row.spot_nav_tao / row.nav_tao)
    } else {
        row.beta_price_tao
    }
}
/// Pending-entitlement mark since the frozen baseline.
pub fn staker_yield(row: &Fund) -> Option<f64> {
    let raw = spot_beta_price(row);
    row.basket_rate
        .map(|rate| (rate - fund_pricing(&row.hotkey, raw).baseline_rate).max(0.0) * raw)
}
/// Pre-v3 approximation of the total-return stake price.
pub fn stake_value(row: &Fund) -> Option<f64> {
    staker_yield(row).map(|yield_| {
        (1.0 + yield_) * tr_index_level(fund_pricing(&row.hotkey, spot_beta_price(row)).first_block)
    })
}
fn apply(row: &Fund, level: f64, tr_level: f64) -> Normalized {
    if let Some(chain) = &row.chain_pricing {
        let divisor = if chain.display_price != 0.0 {
            chain.spot_price / chain.display_price
        } else {
            1.0
        };
        return Normalized {
            display_price_tao: chain.display_price,
            display_beta: row.beta.map(|beta| beta * divisor),
            vs_index: if chain.bag_index != 0.0 {
                chain.display_price / chain.bag_index - 1.0
            } else {
                0.0
            },
            index_first_block: (chain.first_block != 0).then_some(chain.first_block),
            index_provisional: chain.provisional,
            basket_index: chain.bag_index,
            staker_yield: Some(chain.staker_yield),
            stake_value: Some(chain.stake_price),
            stake_vs_index: (chain.stake_index != 0.0)
                .then(|| chain.stake_price / chain.stake_index - 1.0),
            stake_index: chain.stake_index,
            staker_twr: Some(chain.staker_twr),
        };
    }
    let raw = spot_beta_price(row);
    let pricing = fund_pricing(&row.hotkey, raw);
    let display = if pricing.baseline != 0.0 {
        raw / pricing.baseline
    } else {
        raw
    };
    let value = stake_value(row);
    Normalized {
        display_price_tao: display,
        display_beta: row.beta.map(|beta| beta * pricing.baseline),
        vs_index: if level != 0.0 {
            display / level - 1.0
        } else {
            0.0
        },
        index_first_block: pricing.first_block,
        index_provisional: pricing.provisional,
        basket_index: level,
        staker_yield: staker_yield(row),
        stake_value: value,
        stake_vs_index: value
            .filter(|_| tr_level != 0.0)
            .map(|value| value / tr_level - 1.0),
        stake_index: tr_level,
        staker_twr: None,
    }
}
/// Normalize one record against the frozen indexes or canonical chain values.
pub fn normalize_position(row: &Fund) -> Normalized {
    apply(row, index_level(None), tr_index_level(None))
}
/// Normalize a board using spot-NAV weighting; return its bag index and fields.
pub fn normalize_positions(rows: &[Fund]) -> (f64, Vec<Normalized>) {
    let (level, tr_level) =
        if let Some(chain) = rows.iter().find_map(|row| row.chain_pricing.as_ref()) {
            (chain.bag_index, chain.stake_index)
        } else {
            let (mut weight, mut total, mut tr_weight, mut tr_total) = (0.0, 0.0, 0.0, 0.0);
            for row in rows {
                let raw = spot_beta_price(row);
                let pricing = fund_pricing(&row.hotkey, raw);
                let display = if pricing.baseline != 0.0 {
                    raw / pricing.baseline
                } else {
                    raw
                };
                let spot = if row.spot_nav_tao != 0.0 {
                    row.spot_nav_tao
                } else {
                    row.nav_tao
                };
                if spot >= 0.1 && display > 0.0 {
                    weight += spot;
                    total += spot * display;
                    if let Some(value) = stake_value(row) {
                        tr_weight += spot;
                        tr_total += spot * value;
                    }
                }
            }
            (
                if weight != 0.0 {
                    total / weight
                } else {
                    index_level(None)
                },
                if tr_weight != 0.0 {
                    tr_total / tr_weight
                } else {
                    tr_index_level(None)
                },
            )
        };
    (
        level,
        rows.iter().map(|row| apply(row, level, tr_level)).collect(),
    )
}
/// Signed elapsed days at twelve seconds per block, including historical views.
pub fn age_days(first: Option<u64>, current: u64) -> Option<f64> {
    first.and_then(|first| {
        rtao::primitives::ratio::integer_division(i128::from(current) - i128::from(first), 7200)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    fn row(name: &str) -> Fund {
        Fund {
            hotkey: name.into(),
            nav_tao: 2.0,
            spot_nav_tao: 4.0,
            beta_price_tao: 3.0,
            basket_rate: Some(0.25),
            beta: Some(7.0),
            chain_pricing: None,
        }
    }
    #[test]
    fn frozen_series_uses_previous_sample_and_latest_default() {
        let samples = [(10, 1.5), (20, 2.0)];
        assert_eq!(series_level(&samples, Some(9)), 1.0);
        assert_eq!(series_level(&samples, Some(10)), 1.5);
        assert_eq!(series_level(&samples, Some(19)), 1.5);
        assert_eq!(series_level(&samples, Some(20)), 2.0);
        assert_eq!(series_level(&samples, None), 2.0);
        assert_eq!(series_level(&[], None), 1.0);
        assert_eq!(age_days(Some(7201), 1), Some(-1.0));
        assert_eq!(age_days(None, 1), None);
    }
    #[test]
    fn unknown_fund_preserves_position_value_and_uses_raw_spot_yield() -> Result<(), &'static str> {
        let row = row("unknown-test-fund");
        let normalized = normalize_position(&row);
        assert_eq!(spot_beta_price(&row), 6.0);
        assert!(normalized.index_provisional);
        assert_eq!(normalized.index_first_block, None);
        assert!((normalized.display_price_tao - index_level(None)).abs() < 1e-12);
        assert!(
            (normalized.display_beta.ok_or("position beta missing")?
                * normalized.display_price_tao
                - 42.0)
                .abs()
                < 1e-10
        );
        assert_eq!(normalized.staker_yield, Some(1.5));
        assert_eq!(normalized.stake_value, Some(2.5 * tr_index_level(None)));
        let mut absent = row.clone();
        absent.basket_rate = None;
        assert_eq!(normalize_position(&absent).stake_value, None);
        Ok(())
    }
    #[test]
    fn frozen_baseline_subtracts_migration_rate_and_clamps_negative_delta()
    -> Result<(), &'static str> {
        let (name, baseline, first, rate) = BASELINES.first().ok_or("pinned table is empty")?;
        let mut row = row(name);
        row.basket_rate = Some(*rate - 1.0);
        let output = normalize_position(&row);
        assert!(!output.index_provisional);
        assert_eq!(output.index_first_block, Some(*first));
        assert_eq!(output.display_price_tao, 6.0 / baseline);
        assert_eq!(output.staker_yield, Some(0.0));
        assert_eq!(output.stake_value, Some(tr_index_level(Some(*first))));
        Ok(())
    }
    #[test]
    fn runtime_prices_override_fallback_and_mixed_boards_share_runtime_levels() {
        let mut canonical = row("canonical");
        canonical.chain_pricing = Some(ChainPricing {
            hotkey: rtao::primitives::AccountId([1; 32]),
            spot_price: 12.0,
            display_price: 3.0,
            stake_price: 5.0,
            staker_yield: 0.4,
            staker_twr: 1.2,
            bag_index: 2.0,
            stake_index: 4.0,
            first_block: 0,
            provisional: false,
            display_shares: 999.0,
        });
        let fallback = row("fallback");
        let (level, outputs) = normalize_positions(&[canonical, fallback]);
        assert_eq!(level, 2.0);
        assert_eq!(outputs[0].display_beta, Some(28.0));
        assert_eq!(outputs[0].vs_index, 0.5);
        assert_eq!(outputs[0].stake_vs_index, Some(0.25));
        assert_eq!(outputs[0].staker_twr, Some(1.2));
        assert_eq!(outputs[0].index_first_block, None);
        assert_eq!(outputs[1].basket_index, 2.0);
        assert_eq!(outputs[1].stake_index, 4.0);
    }
    #[test]
    fn dust_does_not_pull_live_weighted_index() -> Result<(), &'static str> {
        let (name, baseline, _, _) = BASELINES.first().ok_or("pinned table is empty")?;
        let frozen_level = index_level(None);
        let mut a = row(name);
        let mut b = row("b");
        a.nav_tao = 0.09;
        a.spot_nav_tao = 0.09;
        a.beta_price_tao = baseline * frozen_level * 10.0;
        b.nav_tao = 5.0;
        b.spot_nav_tao = 5.0;
        let (level, outputs) = normalize_positions(&[a.clone(), b.clone()]);
        assert!((outputs[0].display_price_tao - frozen_level * 10.0).abs() < 1e-12);
        assert!((level - frozen_level).abs() < 1e-12);
        assert_eq!(outputs.len(), 2);
        // The exact cutoff participates, so the two distinct prices now
        // determine a weighted average rather than the latest frozen level.
        a.nav_tao = 0.1;
        a.spot_nav_tao = 0.1;
        let (level, _) = normalize_positions(&[a, b]);
        let expected = (0.1 * frozen_level * 10.0 + 5.0 * frozen_level) / 5.1;
        assert!((level - expected).abs() < 1e-12);
        assert_eq!(normalize_positions(&[]).0, frozen_level);
        Ok(())
    }
}
