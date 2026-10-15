// Derived from RaoFoundation/subtensor pallets/subtensor/src/rpc_info/metagraph.rs @ 850f2755bb05a3851f948489f0ab27b0645cebce (Apache-2.0); modified by rtao.
//! By-name metagraph wire models from the pinned pallet declarations.

use super::{AxonInfo, ChainIdentityV2, SubnetIdentityV3, ToDynamic};
use rtao_core::codec::Value;
use rtao_primitives::{
    AccountId, AlphaBalance, CompactBytesText, I96F32, NetUidStorageIndex, PerU16, StakeWeight,
    TaoBalance,
};
use scale_decode::DecodeAsType;

/// The runtime's `Metagraph` record, decoded by field name.
#[derive(Clone, Debug, PartialEq, DecodeAsType, serde::Serialize, serde::Deserialize)]
pub struct MetagraphWire {
    /// Runtime field `netuid`.
    pub netuid: NetUidStorageIndex,
    /// Runtime field `name`.
    pub name: CompactBytesText,
    /// Runtime field `symbol`.
    pub symbol: CompactBytesText,
    /// Runtime field `identity`.
    pub identity: Option<SubnetIdentityV3>,
    /// Runtime field `network_registered_at`.
    pub network_registered_at: u64,
    /// Runtime field `owner_hotkey`.
    pub owner_hotkey: AccountId,
    /// Runtime field `owner_coldkey`.
    pub owner_coldkey: AccountId,
    /// Runtime field `block`.
    pub block: u64,
    /// Runtime field `tempo`.
    pub tempo: u16,
    /// Runtime field `last_step`.
    pub last_step: u64,
    /// Runtime field `blocks_since_last_step`.
    pub blocks_since_last_step: u64,
    /// Runtime field `subnet_emission`.
    #[deprecated(note = "The runtime retains this field for wire compatibility")]
    pub subnet_emission: u64,
    /// Runtime field `alpha_in`.
    pub alpha_in: AlphaBalance,
    /// Runtime field `alpha_out`.
    pub alpha_out: AlphaBalance,
    /// Runtime field `tao_in`.
    pub tao_in: TaoBalance,
    /// Runtime field `alpha_out_emission`.
    pub alpha_out_emission: AlphaBalance,
    /// Runtime field `alpha_in_emission`.
    pub alpha_in_emission: AlphaBalance,
    /// Runtime field `tao_in_emission`.
    pub tao_in_emission: TaoBalance,
    /// Runtime field `pending_alpha_emission`.
    pub pending_alpha_emission: AlphaBalance,
    /// Runtime field `pending_root_emission`.
    #[deprecated(note = "The runtime retains this field for wire compatibility")]
    pub pending_root_emission: TaoBalance,
    /// Runtime field `subnet_volume`.
    pub subnet_volume: u128,
    /// Runtime field `moving_price`.
    pub moving_price: I96F32,
    /// Runtime field `rho`.
    pub rho: u16,
    /// Runtime field `kappa`.
    pub kappa: u16,
    /// Runtime field `min_allowed_weights`.
    pub min_allowed_weights: u16,
    /// Runtime field `max_weights_limit`.
    pub max_weights_limit: u16,
    /// Runtime field `weights_version`.
    pub weights_version: u64,
    /// Runtime field `weights_rate_limit`.
    pub weights_rate_limit: u64,
    /// Runtime field `activity_cutoff`.
    pub activity_cutoff: u64,
    /// Runtime field `max_validators`.
    pub max_validators: u16,
    /// Runtime field `num_uids`.
    pub num_uids: u16,
    /// Runtime field `max_uids`.
    pub max_uids: u16,
    /// Runtime field `burn`.
    pub burn: TaoBalance,
    /// Runtime field `difficulty`.
    pub difficulty: u64,
    /// Runtime field `registration_allowed`.
    pub registration_allowed: bool,
    /// Runtime field `pow_registration_allowed`.
    pub pow_registration_allowed: bool,
    /// Runtime field `immunity_period`.
    pub immunity_period: u16,
    /// Runtime field `min_difficulty`.
    pub min_difficulty: u64,
    /// Runtime field `max_difficulty`.
    pub max_difficulty: u64,
    /// Runtime field `min_burn`.
    pub min_burn: TaoBalance,
    /// Runtime field `max_burn`.
    pub max_burn: TaoBalance,
    /// Runtime field `adjustment_alpha`.
    pub adjustment_alpha: u64,
    /// Runtime field `adjustment_interval`.
    pub adjustment_interval: u16,
    /// Runtime field `target_regs_per_interval`.
    pub target_regs_per_interval: u16,
    /// Runtime field `max_regs_per_block`.
    pub max_regs_per_block: u16,
    /// Runtime field `serving_rate_limit`.
    pub serving_rate_limit: u64,
    /// Runtime field `commit_reveal_weights_enabled`.
    pub commit_reveal_weights_enabled: bool,
    /// Runtime field `commit_reveal_period`.
    pub commit_reveal_period: u64,
    /// Runtime field `liquid_alpha_enabled`.
    pub liquid_alpha_enabled: bool,
    /// Runtime field `alpha_high`.
    pub alpha_high: u16,
    /// Runtime field `alpha_low`.
    pub alpha_low: u16,
    /// Runtime field `bonds_moving_avg`.
    pub bonds_moving_avg: u64,
    /// Runtime field `hotkeys`.
    pub hotkeys: Vec<AccountId>,
    /// Runtime field `coldkeys`.
    pub coldkeys: Vec<AccountId>,
    /// Runtime field `identities`.
    pub identities: Vec<Option<ChainIdentityV2>>,
    /// Runtime field `axons`.
    pub axons: Vec<AxonInfo>,
    /// Runtime field `active`.
    pub active: Vec<bool>,
    /// Runtime field `validator_permit`.
    pub validator_permit: Vec<bool>,
    /// Runtime field `pruning_score`.
    #[deprecated(note = "The runtime retains this field for wire compatibility")]
    pub pruning_score: Vec<u16>,
    /// Runtime field `last_update`.
    pub last_update: Vec<u64>,
    /// Runtime field `emission`.
    pub emission: Vec<AlphaBalance>,
    /// Runtime field `dividends`.
    pub dividends: Vec<PerU16>,
    /// Runtime field `incentives`.
    pub incentives: Vec<PerU16>,
    /// Runtime field `consensus`.
    pub consensus: Vec<PerU16>,
    /// Runtime field `trust`.
    #[deprecated(note = "The runtime retains this field for wire compatibility")]
    pub trust: Vec<PerU16>,
    /// Runtime field `rank`.
    #[deprecated(note = "The runtime retains this field for wire compatibility")]
    pub rank: Vec<u16>,
    /// Runtime field `block_at_registration`.
    pub block_at_registration: Vec<u64>,
    /// Runtime field `alpha_stake`.
    pub alpha_stake: Vec<AlphaBalance>,
    /// Runtime field `tao_stake`.
    pub tao_stake: Vec<TaoBalance>,
    /// Runtime field `total_stake`.
    pub total_stake: Vec<StakeWeight>,
    /// Runtime field `tao_dividends_per_hotkey`.
    #[deprecated(note = "The runtime retains this field for wire compatibility")]
    pub tao_dividends_per_hotkey: Vec<(AccountId, TaoBalance)>,
    /// Runtime field `alpha_dividends_per_hotkey`.
    pub alpha_dividends_per_hotkey: Vec<(AccountId, AlphaBalance)>,
    /// Runtime field `collateral_locked`.
    pub collateral_locked: Vec<AlphaBalance>,
    /// Runtime field `collateral_min`.
    pub collateral_min: Vec<AlphaBalance>,
    /// Runtime field `collateral_earned`.
    pub collateral_earned: Vec<AlphaBalance>,
}

#[allow(deprecated)]
impl ToDynamic for MetagraphWire {
    fn to_dynamic(&self) -> Value {
        Value::record(vec![
            ("netuid".into(), self.netuid.to_dynamic()),
            ("name".into(), self.name.to_dynamic()),
            ("symbol".into(), self.symbol.to_dynamic()),
            ("identity".into(), self.identity.to_dynamic()),
            (
                "network_registered_at".into(),
                self.network_registered_at.to_dynamic(),
            ),
            ("owner_hotkey".into(), self.owner_hotkey.to_dynamic()),
            ("owner_coldkey".into(), self.owner_coldkey.to_dynamic()),
            ("block".into(), self.block.to_dynamic()),
            ("tempo".into(), self.tempo.to_dynamic()),
            ("last_step".into(), self.last_step.to_dynamic()),
            (
                "blocks_since_last_step".into(),
                self.blocks_since_last_step.to_dynamic(),
            ),
            ("subnet_emission".into(), self.subnet_emission.to_dynamic()),
            ("alpha_in".into(), self.alpha_in.to_dynamic()),
            ("alpha_out".into(), self.alpha_out.to_dynamic()),
            ("tao_in".into(), self.tao_in.to_dynamic()),
            (
                "alpha_out_emission".into(),
                self.alpha_out_emission.to_dynamic(),
            ),
            (
                "alpha_in_emission".into(),
                self.alpha_in_emission.to_dynamic(),
            ),
            ("tao_in_emission".into(), self.tao_in_emission.to_dynamic()),
            (
                "pending_alpha_emission".into(),
                self.pending_alpha_emission.to_dynamic(),
            ),
            (
                "pending_root_emission".into(),
                self.pending_root_emission.to_dynamic(),
            ),
            ("subnet_volume".into(), self.subnet_volume.to_dynamic()),
            ("moving_price".into(), self.moving_price.to_dynamic()),
            ("rho".into(), self.rho.to_dynamic()),
            ("kappa".into(), self.kappa.to_dynamic()),
            (
                "min_allowed_weights".into(),
                self.min_allowed_weights.to_dynamic(),
            ),
            (
                "max_weights_limit".into(),
                self.max_weights_limit.to_dynamic(),
            ),
            ("weights_version".into(), self.weights_version.to_dynamic()),
            (
                "weights_rate_limit".into(),
                self.weights_rate_limit.to_dynamic(),
            ),
            ("activity_cutoff".into(), self.activity_cutoff.to_dynamic()),
            ("max_validators".into(), self.max_validators.to_dynamic()),
            ("num_uids".into(), self.num_uids.to_dynamic()),
            ("max_uids".into(), self.max_uids.to_dynamic()),
            ("burn".into(), self.burn.to_dynamic()),
            ("difficulty".into(), self.difficulty.to_dynamic()),
            (
                "registration_allowed".into(),
                self.registration_allowed.to_dynamic(),
            ),
            (
                "pow_registration_allowed".into(),
                self.pow_registration_allowed.to_dynamic(),
            ),
            ("immunity_period".into(), self.immunity_period.to_dynamic()),
            ("min_difficulty".into(), self.min_difficulty.to_dynamic()),
            ("max_difficulty".into(), self.max_difficulty.to_dynamic()),
            ("min_burn".into(), self.min_burn.to_dynamic()),
            ("max_burn".into(), self.max_burn.to_dynamic()),
            (
                "adjustment_alpha".into(),
                self.adjustment_alpha.to_dynamic(),
            ),
            (
                "adjustment_interval".into(),
                self.adjustment_interval.to_dynamic(),
            ),
            (
                "target_regs_per_interval".into(),
                self.target_regs_per_interval.to_dynamic(),
            ),
            (
                "max_regs_per_block".into(),
                self.max_regs_per_block.to_dynamic(),
            ),
            (
                "serving_rate_limit".into(),
                self.serving_rate_limit.to_dynamic(),
            ),
            (
                "commit_reveal_weights_enabled".into(),
                self.commit_reveal_weights_enabled.to_dynamic(),
            ),
            (
                "commit_reveal_period".into(),
                self.commit_reveal_period.to_dynamic(),
            ),
            (
                "liquid_alpha_enabled".into(),
                self.liquid_alpha_enabled.to_dynamic(),
            ),
            ("alpha_high".into(), self.alpha_high.to_dynamic()),
            ("alpha_low".into(), self.alpha_low.to_dynamic()),
            (
                "bonds_moving_avg".into(),
                self.bonds_moving_avg.to_dynamic(),
            ),
            ("hotkeys".into(), self.hotkeys.to_dynamic()),
            ("coldkeys".into(), self.coldkeys.to_dynamic()),
            ("identities".into(), self.identities.to_dynamic()),
            ("axons".into(), self.axons.to_dynamic()),
            ("active".into(), self.active.to_dynamic()),
            (
                "validator_permit".into(),
                self.validator_permit.to_dynamic(),
            ),
            ("pruning_score".into(), self.pruning_score.to_dynamic()),
            ("last_update".into(), self.last_update.to_dynamic()),
            ("emission".into(), self.emission.to_dynamic()),
            ("dividends".into(), self.dividends.to_dynamic()),
            ("incentives".into(), self.incentives.to_dynamic()),
            ("consensus".into(), self.consensus.to_dynamic()),
            ("trust".into(), self.trust.to_dynamic()),
            ("rank".into(), self.rank.to_dynamic()),
            (
                "block_at_registration".into(),
                self.block_at_registration.to_dynamic(),
            ),
            ("alpha_stake".into(), self.alpha_stake.to_dynamic()),
            ("tao_stake".into(), self.tao_stake.to_dynamic()),
            ("total_stake".into(), self.total_stake.to_dynamic()),
            (
                "tao_dividends_per_hotkey".into(),
                self.tao_dividends_per_hotkey.to_dynamic(),
            ),
            (
                "alpha_dividends_per_hotkey".into(),
                self.alpha_dividends_per_hotkey.to_dynamic(),
            ),
            (
                "collateral_locked".into(),
                self.collateral_locked.to_dynamic(),
            ),
            ("collateral_min".into(), self.collateral_min.to_dynamic()),
            (
                "collateral_earned".into(),
                self.collateral_earned.to_dynamic(),
            ),
        ])
    }
}

impl MetagraphWire {
    /// Field names required by this by-name projection.
    pub const FIELDS: &'static [&'static str] = &[
        "netuid",
        "name",
        "symbol",
        "identity",
        "network_registered_at",
        "owner_hotkey",
        "owner_coldkey",
        "block",
        "tempo",
        "last_step",
        "blocks_since_last_step",
        "subnet_emission",
        "alpha_in",
        "alpha_out",
        "tao_in",
        "alpha_out_emission",
        "alpha_in_emission",
        "tao_in_emission",
        "pending_alpha_emission",
        "pending_root_emission",
        "subnet_volume",
        "moving_price",
        "rho",
        "kappa",
        "min_allowed_weights",
        "max_weights_limit",
        "weights_version",
        "weights_rate_limit",
        "activity_cutoff",
        "max_validators",
        "num_uids",
        "max_uids",
        "burn",
        "difficulty",
        "registration_allowed",
        "pow_registration_allowed",
        "immunity_period",
        "min_difficulty",
        "max_difficulty",
        "min_burn",
        "max_burn",
        "adjustment_alpha",
        "adjustment_interval",
        "target_regs_per_interval",
        "max_regs_per_block",
        "serving_rate_limit",
        "commit_reveal_weights_enabled",
        "commit_reveal_period",
        "liquid_alpha_enabled",
        "alpha_high",
        "alpha_low",
        "bonds_moving_avg",
        "hotkeys",
        "coldkeys",
        "identities",
        "axons",
        "active",
        "validator_permit",
        "pruning_score",
        "last_update",
        "emission",
        "dividends",
        "incentives",
        "consensus",
        "trust",
        "rank",
        "block_at_registration",
        "alpha_stake",
        "tao_stake",
        "total_stake",
        "tao_dividends_per_hotkey",
        "alpha_dividends_per_hotkey",
        "collateral_locked",
        "collateral_min",
        "collateral_earned",
    ];
}

/// The runtime's `SelectiveMetagraph` record, decoded by field name.
#[derive(Clone, Debug, PartialEq, DecodeAsType, serde::Serialize, serde::Deserialize)]
pub struct SelectiveMetagraphWire {
    /// Runtime field `netuid`.
    pub netuid: NetUidStorageIndex,
    /// Runtime field `name`.
    pub name: Option<CompactBytesText>,
    /// Runtime field `symbol`.
    pub symbol: Option<CompactBytesText>,
    /// Runtime field `identity`.
    pub identity: Option<Option<SubnetIdentityV3>>,
    /// Runtime field `network_registered_at`.
    pub network_registered_at: Option<u64>,
    /// Runtime field `owner_hotkey`.
    pub owner_hotkey: Option<AccountId>,
    /// Runtime field `owner_coldkey`.
    pub owner_coldkey: Option<AccountId>,
    /// Runtime field `block`.
    pub block: Option<u64>,
    /// Runtime field `tempo`.
    pub tempo: Option<u16>,
    /// Runtime field `last_step`.
    pub last_step: Option<u64>,
    /// Runtime field `blocks_since_last_step`.
    pub blocks_since_last_step: Option<u64>,
    /// Runtime field `subnet_emission`.
    #[deprecated(note = "The runtime retains this field for wire compatibility")]
    pub subnet_emission: Option<u64>,
    /// Runtime field `alpha_in`.
    pub alpha_in: Option<AlphaBalance>,
    /// Runtime field `alpha_out`.
    pub alpha_out: Option<AlphaBalance>,
    /// Runtime field `tao_in`.
    pub tao_in: Option<TaoBalance>,
    /// Runtime field `alpha_out_emission`.
    pub alpha_out_emission: Option<AlphaBalance>,
    /// Runtime field `alpha_in_emission`.
    pub alpha_in_emission: Option<AlphaBalance>,
    /// Runtime field `tao_in_emission`.
    pub tao_in_emission: Option<TaoBalance>,
    /// Runtime field `pending_alpha_emission`.
    pub pending_alpha_emission: Option<AlphaBalance>,
    /// Runtime field `pending_root_emission`.
    #[deprecated(note = "The runtime retains this field for wire compatibility")]
    pub pending_root_emission: Option<TaoBalance>,
    /// Runtime field `subnet_volume`.
    pub subnet_volume: Option<u128>,
    /// Runtime field `moving_price`.
    pub moving_price: Option<I96F32>,
    /// Runtime field `rho`.
    pub rho: Option<u16>,
    /// Runtime field `kappa`.
    pub kappa: Option<u16>,
    /// Runtime field `min_allowed_weights`.
    pub min_allowed_weights: Option<u16>,
    /// Runtime field `max_weights_limit`.
    pub max_weights_limit: Option<u16>,
    /// Runtime field `weights_version`.
    pub weights_version: Option<u64>,
    /// Runtime field `weights_rate_limit`.
    pub weights_rate_limit: Option<u64>,
    /// Runtime field `activity_cutoff`.
    pub activity_cutoff: Option<u64>,
    /// Runtime field `max_validators`.
    pub max_validators: Option<u16>,
    /// Runtime field `num_uids`.
    pub num_uids: Option<u16>,
    /// Runtime field `max_uids`.
    pub max_uids: Option<u16>,
    /// Runtime field `burn`.
    pub burn: Option<TaoBalance>,
    /// Runtime field `difficulty`.
    pub difficulty: Option<u64>,
    /// Runtime field `registration_allowed`.
    pub registration_allowed: Option<bool>,
    /// Runtime field `pow_registration_allowed`.
    pub pow_registration_allowed: Option<bool>,
    /// Runtime field `immunity_period`.
    pub immunity_period: Option<u16>,
    /// Runtime field `min_difficulty`.
    pub min_difficulty: Option<u64>,
    /// Runtime field `max_difficulty`.
    pub max_difficulty: Option<u64>,
    /// Runtime field `min_burn`.
    pub min_burn: Option<TaoBalance>,
    /// Runtime field `max_burn`.
    pub max_burn: Option<TaoBalance>,
    /// Runtime field `adjustment_alpha`.
    pub adjustment_alpha: Option<u64>,
    /// Runtime field `adjustment_interval`.
    pub adjustment_interval: Option<u16>,
    /// Runtime field `target_regs_per_interval`.
    pub target_regs_per_interval: Option<u16>,
    /// Runtime field `max_regs_per_block`.
    pub max_regs_per_block: Option<u16>,
    /// Runtime field `serving_rate_limit`.
    pub serving_rate_limit: Option<u64>,
    /// Runtime field `commit_reveal_weights_enabled`.
    pub commit_reveal_weights_enabled: Option<bool>,
    /// Runtime field `commit_reveal_period`.
    pub commit_reveal_period: Option<u64>,
    /// Runtime field `liquid_alpha_enabled`.
    pub liquid_alpha_enabled: Option<bool>,
    /// Runtime field `alpha_high`.
    pub alpha_high: Option<u16>,
    /// Runtime field `alpha_low`.
    pub alpha_low: Option<u16>,
    /// Runtime field `bonds_moving_avg`.
    pub bonds_moving_avg: Option<u64>,
    /// Runtime field `hotkeys`.
    pub hotkeys: Option<Vec<AccountId>>,
    /// Runtime field `coldkeys`.
    pub coldkeys: Option<Vec<AccountId>>,
    /// Runtime field `identities`.
    pub identities: Option<Vec<Option<ChainIdentityV2>>>,
    /// Runtime field `axons`.
    pub axons: Option<Vec<AxonInfo>>,
    /// Runtime field `active`.
    pub active: Option<Vec<bool>>,
    /// Runtime field `validator_permit`.
    pub validator_permit: Option<Vec<bool>>,
    /// Runtime field `pruning_score`.
    #[deprecated(note = "The runtime retains this field for wire compatibility")]
    pub pruning_score: Option<Vec<u16>>,
    /// Runtime field `last_update`.
    pub last_update: Option<Vec<u64>>,
    /// Runtime field `emission`.
    pub emission: Option<Vec<AlphaBalance>>,
    /// Runtime field `dividends`.
    pub dividends: Option<Vec<PerU16>>,
    /// Runtime field `incentives`.
    pub incentives: Option<Vec<PerU16>>,
    /// Runtime field `consensus`.
    pub consensus: Option<Vec<PerU16>>,
    /// Runtime field `trust`.
    #[deprecated(note = "The runtime retains this field for wire compatibility")]
    pub trust: Option<Vec<PerU16>>,
    /// Runtime field `rank`.
    #[deprecated(note = "The runtime retains this field for wire compatibility")]
    pub rank: Option<Vec<u16>>,
    /// Runtime field `block_at_registration`.
    pub block_at_registration: Option<Vec<u64>>,
    /// Runtime field `alpha_stake`.
    pub alpha_stake: Option<Vec<AlphaBalance>>,
    /// Runtime field `tao_stake`.
    pub tao_stake: Option<Vec<TaoBalance>>,
    /// Runtime field `total_stake`.
    pub total_stake: Option<Vec<StakeWeight>>,
    /// Runtime field `tao_dividends_per_hotkey`.
    #[deprecated(note = "The runtime retains this field for wire compatibility")]
    pub tao_dividends_per_hotkey: Option<Vec<(AccountId, TaoBalance)>>,
    /// Runtime field `alpha_dividends_per_hotkey`.
    pub alpha_dividends_per_hotkey: Option<Vec<(AccountId, AlphaBalance)>>,
    /// Runtime field `validators`.
    pub validators: Option<Vec<u16>>,
    /// Runtime field `commitments`.
    pub commitments: Option<Vec<(AccountId, Vec<u8>)>>,
    /// Runtime field `collateral_locked`.
    pub collateral_locked: Option<Vec<AlphaBalance>>,
    /// Runtime field `collateral_min`.
    pub collateral_min: Option<Vec<AlphaBalance>>,
    /// Runtime field `collateral_earned`.
    pub collateral_earned: Option<Vec<AlphaBalance>>,
}

#[allow(deprecated)]
impl ToDynamic for SelectiveMetagraphWire {
    fn to_dynamic(&self) -> Value {
        Value::record(vec![
            ("netuid".into(), self.netuid.to_dynamic()),
            ("name".into(), self.name.to_dynamic()),
            ("symbol".into(), self.symbol.to_dynamic()),
            ("identity".into(), self.identity.to_dynamic()),
            (
                "network_registered_at".into(),
                self.network_registered_at.to_dynamic(),
            ),
            ("owner_hotkey".into(), self.owner_hotkey.to_dynamic()),
            ("owner_coldkey".into(), self.owner_coldkey.to_dynamic()),
            ("block".into(), self.block.to_dynamic()),
            ("tempo".into(), self.tempo.to_dynamic()),
            ("last_step".into(), self.last_step.to_dynamic()),
            (
                "blocks_since_last_step".into(),
                self.blocks_since_last_step.to_dynamic(),
            ),
            ("subnet_emission".into(), self.subnet_emission.to_dynamic()),
            ("alpha_in".into(), self.alpha_in.to_dynamic()),
            ("alpha_out".into(), self.alpha_out.to_dynamic()),
            ("tao_in".into(), self.tao_in.to_dynamic()),
            (
                "alpha_out_emission".into(),
                self.alpha_out_emission.to_dynamic(),
            ),
            (
                "alpha_in_emission".into(),
                self.alpha_in_emission.to_dynamic(),
            ),
            ("tao_in_emission".into(), self.tao_in_emission.to_dynamic()),
            (
                "pending_alpha_emission".into(),
                self.pending_alpha_emission.to_dynamic(),
            ),
            (
                "pending_root_emission".into(),
                self.pending_root_emission.to_dynamic(),
            ),
            ("subnet_volume".into(), self.subnet_volume.to_dynamic()),
            ("moving_price".into(), self.moving_price.to_dynamic()),
            ("rho".into(), self.rho.to_dynamic()),
            ("kappa".into(), self.kappa.to_dynamic()),
            (
                "min_allowed_weights".into(),
                self.min_allowed_weights.to_dynamic(),
            ),
            (
                "max_weights_limit".into(),
                self.max_weights_limit.to_dynamic(),
            ),
            ("weights_version".into(), self.weights_version.to_dynamic()),
            (
                "weights_rate_limit".into(),
                self.weights_rate_limit.to_dynamic(),
            ),
            ("activity_cutoff".into(), self.activity_cutoff.to_dynamic()),
            ("max_validators".into(), self.max_validators.to_dynamic()),
            ("num_uids".into(), self.num_uids.to_dynamic()),
            ("max_uids".into(), self.max_uids.to_dynamic()),
            ("burn".into(), self.burn.to_dynamic()),
            ("difficulty".into(), self.difficulty.to_dynamic()),
            (
                "registration_allowed".into(),
                self.registration_allowed.to_dynamic(),
            ),
            (
                "pow_registration_allowed".into(),
                self.pow_registration_allowed.to_dynamic(),
            ),
            ("immunity_period".into(), self.immunity_period.to_dynamic()),
            ("min_difficulty".into(), self.min_difficulty.to_dynamic()),
            ("max_difficulty".into(), self.max_difficulty.to_dynamic()),
            ("min_burn".into(), self.min_burn.to_dynamic()),
            ("max_burn".into(), self.max_burn.to_dynamic()),
            (
                "adjustment_alpha".into(),
                self.adjustment_alpha.to_dynamic(),
            ),
            (
                "adjustment_interval".into(),
                self.adjustment_interval.to_dynamic(),
            ),
            (
                "target_regs_per_interval".into(),
                self.target_regs_per_interval.to_dynamic(),
            ),
            (
                "max_regs_per_block".into(),
                self.max_regs_per_block.to_dynamic(),
            ),
            (
                "serving_rate_limit".into(),
                self.serving_rate_limit.to_dynamic(),
            ),
            (
                "commit_reveal_weights_enabled".into(),
                self.commit_reveal_weights_enabled.to_dynamic(),
            ),
            (
                "commit_reveal_period".into(),
                self.commit_reveal_period.to_dynamic(),
            ),
            (
                "liquid_alpha_enabled".into(),
                self.liquid_alpha_enabled.to_dynamic(),
            ),
            ("alpha_high".into(), self.alpha_high.to_dynamic()),
            ("alpha_low".into(), self.alpha_low.to_dynamic()),
            (
                "bonds_moving_avg".into(),
                self.bonds_moving_avg.to_dynamic(),
            ),
            ("hotkeys".into(), self.hotkeys.to_dynamic()),
            ("coldkeys".into(), self.coldkeys.to_dynamic()),
            ("identities".into(), self.identities.to_dynamic()),
            ("axons".into(), self.axons.to_dynamic()),
            ("active".into(), self.active.to_dynamic()),
            (
                "validator_permit".into(),
                self.validator_permit.to_dynamic(),
            ),
            ("pruning_score".into(), self.pruning_score.to_dynamic()),
            ("last_update".into(), self.last_update.to_dynamic()),
            ("emission".into(), self.emission.to_dynamic()),
            ("dividends".into(), self.dividends.to_dynamic()),
            ("incentives".into(), self.incentives.to_dynamic()),
            ("consensus".into(), self.consensus.to_dynamic()),
            ("trust".into(), self.trust.to_dynamic()),
            ("rank".into(), self.rank.to_dynamic()),
            (
                "block_at_registration".into(),
                self.block_at_registration.to_dynamic(),
            ),
            ("alpha_stake".into(), self.alpha_stake.to_dynamic()),
            ("tao_stake".into(), self.tao_stake.to_dynamic()),
            ("total_stake".into(), self.total_stake.to_dynamic()),
            (
                "tao_dividends_per_hotkey".into(),
                self.tao_dividends_per_hotkey.to_dynamic(),
            ),
            (
                "alpha_dividends_per_hotkey".into(),
                self.alpha_dividends_per_hotkey.to_dynamic(),
            ),
            ("validators".into(), self.validators.to_dynamic()),
            ("commitments".into(), self.commitments.to_dynamic()),
            (
                "collateral_locked".into(),
                self.collateral_locked.to_dynamic(),
            ),
            ("collateral_min".into(), self.collateral_min.to_dynamic()),
            (
                "collateral_earned".into(),
                self.collateral_earned.to_dynamic(),
            ),
        ])
    }
}

impl SelectiveMetagraphWire {
    /// Field names required by this by-name projection.
    pub const FIELDS: &'static [&'static str] = &[
        "netuid",
        "name",
        "symbol",
        "identity",
        "network_registered_at",
        "owner_hotkey",
        "owner_coldkey",
        "block",
        "tempo",
        "last_step",
        "blocks_since_last_step",
        "subnet_emission",
        "alpha_in",
        "alpha_out",
        "tao_in",
        "alpha_out_emission",
        "alpha_in_emission",
        "tao_in_emission",
        "pending_alpha_emission",
        "pending_root_emission",
        "subnet_volume",
        "moving_price",
        "rho",
        "kappa",
        "min_allowed_weights",
        "max_weights_limit",
        "weights_version",
        "weights_rate_limit",
        "activity_cutoff",
        "max_validators",
        "num_uids",
        "max_uids",
        "burn",
        "difficulty",
        "registration_allowed",
        "pow_registration_allowed",
        "immunity_period",
        "min_difficulty",
        "max_difficulty",
        "min_burn",
        "max_burn",
        "adjustment_alpha",
        "adjustment_interval",
        "target_regs_per_interval",
        "max_regs_per_block",
        "serving_rate_limit",
        "commit_reveal_weights_enabled",
        "commit_reveal_period",
        "liquid_alpha_enabled",
        "alpha_high",
        "alpha_low",
        "bonds_moving_avg",
        "hotkeys",
        "coldkeys",
        "identities",
        "axons",
        "active",
        "validator_permit",
        "pruning_score",
        "last_update",
        "emission",
        "dividends",
        "incentives",
        "consensus",
        "trust",
        "rank",
        "block_at_registration",
        "alpha_stake",
        "tao_stake",
        "total_stake",
        "tao_dividends_per_hotkey",
        "alpha_dividends_per_hotkey",
        "validators",
        "commitments",
        "collateral_locked",
        "collateral_min",
        "collateral_earned",
    ];
}
