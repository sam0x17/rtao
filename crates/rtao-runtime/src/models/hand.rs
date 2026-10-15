//! By-name runtime return models from the pinned v478 runtime and SDK structs.
// SPDX-License-Identifier: Apache-2.0

use rtao_core::codec::Value;
use rtao_primitives::{
    AccountId, AlphaBalance, CompactBytesText, I32F32, I96F32, IntoCoreValue, NetUid, PerU16,
    Percent, PriceRao, TaoBalance, U64F64,
};
use scale_decode::DecodeAsType;

use super::{AxonInfo, SubnetIdentityV3, ToDynamic, bytes_dynamic};

#[path = "commitment_data.rs"]
mod commitment_data;
pub use commitment_data::Data;

macro_rules! record_projection {
    ($name:ident { $($field:ident $(=> $project:ident)?),* $(,)? }) => {
        #[allow(deprecated)]
        impl ToDynamic for $name {
            fn to_dynamic(&self) -> Value {
                Value::record(vec![$((stringify!($field).into(), record_projection!(@field &self.$field $(, $project)?))),*])
            }
        }
        impl IntoCoreValue for $name {
            #[allow(deprecated)]
            fn into_core_value(self) -> Value {
                Value::record(vec![$((stringify!($field).into(), record_projection!(@encode self.$field $(, $project)?))),*])
            }
        }
    };
    (@field $value:expr, bytes) => { bytes_dynamic($value) };
    (@field $value:expr) => { $value.to_dynamic() };
    (@encode $value:expr, bytes) => { Value::Bytes($value) };
    (@encode $value:expr) => { $value.into_core_value() };
}

impl ToDynamic for U64F64 {
    fn to_dynamic(&self) -> Value {
        (*self).into_core_value()
    }
}
impl ToDynamic for I32F32 {
    fn to_dynamic(&self) -> Value {
        (*self).into_core_value()
    }
}
impl ToDynamic for Percent {
    fn to_dynamic(&self) -> Value {
        (*self).into_core_value()
    }
}
impl ToDynamic for PriceRao {
    fn to_dynamic(&self) -> Value {
        (*self).into_core_value()
    }
}
impl ToDynamic for [u16; 2] {
    fn to_dynamic(&self) -> Value {
        Value::List(self.iter().map(ToDynamic::to_dynamic).collect())
    }
}

/// Exact field projection of runtime `NeuronInfoLite`.
///
/// Source: pinned `subtensor/src/rpc_info/neuron_info.rs:35`; compact wire fields are decoded through metadata.
#[allow(deprecated)]
#[derive(Clone, Debug, PartialEq, DecodeAsType, serde::Serialize, serde::Deserialize)]
pub struct NeuronInfoLite {
    /// Runtime field `hotkey`.
    pub hotkey: AccountId,
    /// Runtime field `coldkey`.
    pub coldkey: AccountId,
    /// Runtime field `uid`.
    pub uid: u16,
    /// Runtime field `netuid`.
    pub netuid: NetUid,
    /// Runtime field `active`.
    pub active: bool,
    /// Runtime field `axon_info`.
    pub axon_info: AxonInfo,
    /// Runtime field `prometheus_info`.
    pub prometheus_info: PrometheusInfo,
    /// Runtime field `stake`.
    pub stake: Vec<(AccountId, AlphaBalance)>,
    /// Runtime field `rank`.
    #[deprecated(note = "Retained for runtime wire compatibility; no longer computed.")]
    pub rank: u16,
    /// Runtime field `emission`.
    pub emission: AlphaBalance,
    /// Runtime field `incentive`.
    pub incentive: PerU16,
    /// Runtime field `consensus`.
    pub consensus: PerU16,
    /// Runtime field `trust`.
    #[deprecated(note = "Retained for runtime wire compatibility; no longer computed.")]
    pub trust: PerU16,
    /// Runtime field `validator_trust`.
    pub validator_trust: PerU16,
    /// Runtime field `dividends`.
    pub dividends: PerU16,
    /// Runtime field `last_update`.
    pub last_update: u64,
    /// Runtime field `validator_permit`.
    pub validator_permit: bool,
    /// Runtime field `pruning_score`.
    #[deprecated(note = "Retained for runtime wire compatibility; no longer computed.")]
    pub pruning_score: u16,
}
record_projection!(NeuronInfoLite {
    hotkey,
    coldkey,
    uid,
    netuid,
    active,
    axon_info,
    prometheus_info,
    stake,
    rank,
    emission,
    incentive,
    consensus,
    trust,
    validator_trust,
    dividends,
    last_update,
    validator_permit,
    pruning_score
});

/// Exact field projection of runtime `NeuronInfo`.
///
/// Source: pinned `subtensor/src/rpc_info/neuron_info.rs:10`; compact wire fields are decoded through metadata.
#[allow(deprecated)]
#[derive(Clone, Debug, PartialEq, DecodeAsType, serde::Serialize, serde::Deserialize)]
pub struct NeuronInfo {
    /// Runtime field `hotkey`.
    pub hotkey: AccountId,
    /// Runtime field `coldkey`.
    pub coldkey: AccountId,
    /// Runtime field `uid`.
    pub uid: u16,
    /// Runtime field `netuid`.
    pub netuid: NetUid,
    /// Runtime field `active`.
    pub active: bool,
    /// Runtime field `axon_info`.
    pub axon_info: AxonInfo,
    /// Runtime field `prometheus_info`.
    pub prometheus_info: PrometheusInfo,
    /// Runtime field `stake`.
    pub stake: Vec<(AccountId, AlphaBalance)>,
    /// Runtime field `rank`.
    #[deprecated(note = "Retained for runtime wire compatibility; no longer computed.")]
    pub rank: u16,
    /// Runtime field `emission`.
    pub emission: AlphaBalance,
    /// Runtime field `incentive`.
    pub incentive: PerU16,
    /// Runtime field `consensus`.
    pub consensus: PerU16,
    /// Runtime field `trust`.
    #[deprecated(note = "Retained for runtime wire compatibility; no longer computed.")]
    pub trust: PerU16,
    /// Runtime field `validator_trust`.
    pub validator_trust: PerU16,
    /// Runtime field `dividends`.
    pub dividends: PerU16,
    /// Runtime field `last_update`.
    pub last_update: u64,
    /// Runtime field `validator_permit`.
    pub validator_permit: bool,
    /// Runtime field `weights`.
    pub weights: Vec<(u16, u16)>,
    /// Runtime field `bonds`.
    pub bonds: Vec<(u16, u16)>,
    /// Runtime field `pruning_score`.
    #[deprecated(note = "Retained for runtime wire compatibility; no longer computed.")]
    pub pruning_score: u16,
}
record_projection!(NeuronInfo {
    hotkey,
    coldkey,
    uid,
    netuid,
    active,
    axon_info,
    prometheus_info,
    stake,
    rank,
    emission,
    incentive,
    consensus,
    trust,
    validator_trust,
    dividends,
    last_update,
    validator_permit,
    weights,
    bonds,
    pruning_score
});

/// Exact field projection of runtime `DynamicInfo`.
///
/// Source: pinned `subtensor/src/rpc_info/dynamic_info.rs:11`; compact wire fields are decoded through metadata.
#[allow(deprecated)]
#[derive(Clone, Debug, PartialEq, DecodeAsType, serde::Serialize, serde::Deserialize)]
pub struct DynamicInfo {
    /// Runtime field `netuid`.
    pub netuid: NetUid,
    /// Runtime field `owner_hotkey`.
    pub owner_hotkey: AccountId,
    /// Runtime field `owner_coldkey`.
    pub owner_coldkey: AccountId,
    /// Runtime field `subnet_name`.
    pub subnet_name: CompactBytesText,
    /// Runtime field `token_symbol`.
    pub token_symbol: CompactBytesText,
    /// Runtime field `tempo`.
    pub tempo: u16,
    /// Runtime field `last_step`.
    pub last_step: u64,
    /// Runtime field `blocks_since_last_step`.
    pub blocks_since_last_step: u64,
    /// Runtime field `emission`.
    pub emission: u64,
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
    #[deprecated(note = "Retained for runtime wire compatibility; no longer computed.")]
    pub pending_root_emission: TaoBalance,
    /// Runtime field `subnet_volume`.
    pub subnet_volume: u128,
    /// Runtime field `network_registered_at`.
    pub network_registered_at: u64,
    /// Runtime field `subnet_identity`.
    pub subnet_identity: Option<SubnetIdentityV3>,
    /// Runtime field `moving_price`.
    pub moving_price: I96F32,
}
record_projection!(DynamicInfo {
    netuid,
    owner_hotkey,
    owner_coldkey,
    subnet_name,
    token_symbol,
    tempo,
    last_step,
    blocks_since_last_step,
    emission,
    alpha_in,
    alpha_out,
    tao_in,
    alpha_out_emission,
    alpha_in_emission,
    tao_in_emission,
    pending_alpha_emission,
    pending_root_emission,
    subnet_volume,
    network_registered_at,
    subnet_identity,
    moving_price
});

/// Exact field projection of runtime `SubnetInfov2`.
///
/// Source: pinned `subtensor/src/rpc_info/subnet_info.rs:34`; compact wire fields are decoded through metadata.
#[allow(deprecated)]
#[derive(Clone, Debug, PartialEq, DecodeAsType, serde::Serialize, serde::Deserialize)]
pub struct SubnetInfoV2 {
    /// Runtime field `netuid`.
    pub netuid: NetUid,
    /// Runtime field `rho`.
    pub rho: u16,
    /// Runtime field `kappa`.
    pub kappa: u16,
    /// Runtime field `difficulty`.
    pub difficulty: u64,
    /// Runtime field `immunity_period`.
    pub immunity_period: u16,
    /// Runtime field `max_allowed_validators`.
    pub max_allowed_validators: u16,
    /// Runtime field `min_allowed_weights`.
    pub min_allowed_weights: u16,
    /// Runtime field `max_weights_limit`.
    pub max_weights_limit: u16,
    /// Runtime field `scaling_law_power`.
    pub scaling_law_power: u16,
    /// Runtime field `subnetwork_n`.
    pub subnetwork_n: u16,
    /// Runtime field `max_allowed_uids`.
    pub max_allowed_uids: u16,
    /// Runtime field `blocks_since_last_step`.
    pub blocks_since_last_step: u64,
    /// Runtime field `tempo`.
    pub tempo: u16,
    /// Runtime field `network_modality`.
    pub network_modality: u16,
    /// Runtime field `network_connect`.
    pub network_connect: Vec<[u16; 2]>,
    /// Runtime field `emission_value`.
    pub emission_value: u64,
    /// Runtime field `burn`.
    pub burn: TaoBalance,
    /// Runtime field `owner`.
    pub owner: AccountId,
    /// Runtime field `identity`.
    pub identity: Option<SubnetIdentityV3>,
}
record_projection!(SubnetInfoV2 {
    netuid,
    rho,
    kappa,
    difficulty,
    immunity_period,
    max_allowed_validators,
    min_allowed_weights,
    max_weights_limit,
    scaling_law_power,
    subnetwork_n,
    max_allowed_uids,
    blocks_since_last_step,
    tempo,
    network_modality,
    network_connect,
    emission_value,
    burn,
    owner,
    identity
});

/// Exact field projection of runtime `SubnetState`.
///
/// Source: pinned `subtensor/src/rpc_info/show_subnet.rs:12`; compact wire fields are decoded through metadata.
#[allow(deprecated)]
#[derive(Clone, Debug, PartialEq, DecodeAsType, serde::Serialize, serde::Deserialize)]
pub struct SubnetState {
    /// Runtime field `netuid`.
    pub netuid: NetUid,
    /// Runtime field `hotkeys`.
    pub hotkeys: Vec<AccountId>,
    /// Runtime field `coldkeys`.
    pub coldkeys: Vec<AccountId>,
    /// Runtime field `active`.
    pub active: Vec<bool>,
    /// Runtime field `validator_permit`.
    pub validator_permit: Vec<bool>,
    /// Runtime field `pruning_score`.
    #[deprecated(note = "Retained for runtime wire compatibility; no longer computed.")]
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
    #[deprecated(note = "Retained for runtime wire compatibility; no longer computed.")]
    pub trust: Vec<PerU16>,
    /// Runtime field `rank`.
    #[deprecated(note = "Retained for runtime wire compatibility; no longer computed.")]
    pub rank: Vec<u16>,
    /// Runtime field `block_at_registration`.
    pub block_at_registration: Vec<u64>,
    /// Runtime field `alpha_stake`.
    pub alpha_stake: Vec<AlphaBalance>,
    /// Runtime field `tao_stake`.
    pub tao_stake: Vec<TaoBalance>,
    /// Runtime field `total_stake`.
    pub total_stake: Vec<TaoBalance>,
    /// Runtime field `emission_history`.
    pub emission_history: Vec<Vec<AlphaBalance>>,
}
record_projection!(SubnetState {
    netuid,
    hotkeys,
    coldkeys,
    active,
    validator_permit,
    pruning_score,
    last_update,
    emission,
    dividends,
    incentives,
    consensus,
    trust,
    rank,
    block_at_registration,
    alpha_stake,
    tao_stake,
    total_stake,
    emission_history
});

/// Exact field projection of runtime `SimSwapResult`.
///
/// Source: pinned `swap/runtime-api/src/lib.rs:11`; compact wire fields are decoded through metadata.
#[allow(deprecated)]
#[derive(Clone, Debug, PartialEq, DecodeAsType, serde::Serialize, serde::Deserialize)]
pub struct SimSwapResult {
    /// Runtime field `tao_amount`.
    pub tao_amount: TaoBalance,
    /// Runtime field `alpha_amount`.
    pub alpha_amount: AlphaBalance,
    /// Runtime field `tao_fee`.
    pub tao_fee: TaoBalance,
    /// Runtime field `alpha_fee`.
    pub alpha_fee: AlphaBalance,
    /// Runtime field `tao_slippage`.
    pub tao_slippage: TaoBalance,
    /// Runtime field `alpha_slippage`.
    pub alpha_slippage: AlphaBalance,
}
record_projection!(SimSwapResult {
    tao_amount,
    alpha_amount,
    tao_fee,
    alpha_fee,
    tao_slippage,
    alpha_slippage
});

/// Exact field projection of runtime `SubnetPrice`.
///
/// Source: pinned `swap/runtime-api/src/lib.rs:22`; compact wire fields are decoded through metadata.
#[allow(deprecated)]
#[derive(Clone, Debug, PartialEq, DecodeAsType, serde::Serialize, serde::Deserialize)]
pub struct SubnetPrice {
    /// Runtime field `netuid`.
    pub netuid: NetUid,
    /// Runtime field `price`.
    pub price: PriceRao,
}
record_projection!(SubnetPrice { netuid, price });

/// Exact field projection of runtime `StakeInfo`.
///
/// Source: pinned `subtensor/src/rpc_info/stake_info.rs:13`; compact wire fields are decoded through metadata.
#[allow(deprecated)]
#[derive(Clone, Debug, PartialEq, DecodeAsType, serde::Serialize, serde::Deserialize)]
pub struct StakeInfo {
    /// Runtime field `hotkey`.
    pub hotkey: AccountId,
    /// Runtime field `coldkey`.
    pub coldkey: AccountId,
    /// Runtime field `netuid`.
    pub netuid: NetUid,
    /// Runtime field `stake`.
    pub stake: AlphaBalance,
    /// Runtime field `locked`.
    pub locked: u64,
    /// Runtime field `emission`.
    pub emission: AlphaBalance,
    /// Runtime field `tao_emission`.
    pub tao_emission: TaoBalance,
    /// Runtime field `drain`.
    pub drain: u64,
    /// Runtime field `is_registered`.
    pub is_registered: bool,
}
record_projection!(StakeInfo {
    hotkey,
    coldkey,
    netuid,
    stake,
    locked,
    emission,
    tao_emission,
    drain,
    is_registered
});

/// Exact field projection of runtime `StakeAvailability`.
///
/// Source: pinned `subtensor/src/rpc_info/stake_info.rs:27`; compact wire fields are decoded through metadata.
#[allow(deprecated)]
#[derive(Clone, Debug, PartialEq, DecodeAsType, serde::Serialize, serde::Deserialize)]
pub struct StakeAvailability {
    /// Runtime field `total`.
    pub total: AlphaBalance,
    /// Runtime field `locked`.
    pub locked: AlphaBalance,
    /// Runtime field `available`.
    pub available: AlphaBalance,
}
record_projection!(StakeAvailability {
    total,
    locked,
    available
});

/// Exact field projection of runtime `DelegateInfo`.
///
/// Source: pinned `subtensor/src/rpc_info/delegate_info.rs:14`; compact wire fields are decoded through metadata.
#[allow(deprecated)]
#[derive(Clone, Debug, PartialEq, DecodeAsType, serde::Serialize, serde::Deserialize)]
pub struct DelegateInfo {
    /// Runtime field `delegate_ss58`.
    pub delegate_ss58: AccountId,
    /// Runtime field `take`.
    pub take: PerU16,
    /// Runtime field `nominators`.
    pub nominators: Vec<(AccountId, Vec<(NetUid, u64)>)>,
    /// Runtime field `owner_ss58`.
    pub owner_ss58: AccountId,
    /// Runtime field `registrations`.
    pub registrations: Vec<NetUid>,
    /// Runtime field `validator_permits`.
    pub validator_permits: Vec<NetUid>,
    /// Runtime field `return_per_1000`.
    pub return_per_1000: u64,
    /// Runtime field `total_daily_return`.
    pub total_daily_return: u64,
}
record_projection!(DelegateInfo {
    delegate_ss58,
    take,
    nominators,
    owner_ss58,
    registrations,
    validator_permits,
    return_per_1000,
    total_daily_return
});

/// Exact field projection of runtime `PrometheusInfo`.
///
/// Source: pinned `subtensor/src/lib.rs:363`; compact wire fields are decoded through metadata.
#[allow(deprecated)]
#[derive(Clone, Debug, PartialEq, DecodeAsType, serde::Serialize, serde::Deserialize)]
pub struct PrometheusInfo {
    /// Runtime field `block`.
    pub block: u64,
    /// Runtime field `version`.
    pub version: u32,
    /// Runtime field `ip`.
    pub ip: u128,
    /// Runtime field `port`.
    pub port: u16,
    /// Runtime field `ip_type`.
    pub ip_type: u8,
}
record_projection!(PrometheusInfo {
    block,
    version,
    ip,
    port,
    ip_type
});

/// Exact field projection of runtime `NeuronCertificate`.
///
/// Source: pinned `subtensor/src/lib.rs:332`; compact wire fields are decoded through metadata.
#[allow(deprecated)]
#[derive(Clone, Debug, PartialEq, DecodeAsType, serde::Serialize, serde::Deserialize)]
pub struct NeuronCertificate {
    /// Runtime field `public_key`.
    pub public_key: Vec<u8>,
    /// Runtime field `algorithm`.
    pub algorithm: u8,
}
record_projection!(NeuronCertificate { public_key => bytes, algorithm });

/// Exact field projection of runtime `LockState`.
///
/// Source: pinned `subtensor/src/staking/lock.rs:19`; compact wire fields are decoded through metadata.
#[allow(deprecated)]
#[derive(Clone, Debug, PartialEq, DecodeAsType, serde::Serialize, serde::Deserialize)]
pub struct LockState {
    /// Runtime field `locked_mass`.
    pub locked_mass: AlphaBalance,
    /// Runtime field `conviction`.
    pub conviction: U64F64,
    /// Runtime field `last_update`.
    pub last_update: u64,
}
record_projection!(LockState {
    locked_mass,
    conviction,
    last_update
});

/// Exact field projection of runtime `MinerCollateralState`.
///
/// Source: pinned `subtensor/src/lib.rs:550`; compact wire fields are decoded through metadata.
#[allow(deprecated)]
#[derive(Clone, Debug, PartialEq, DecodeAsType, serde::Serialize, serde::Deserialize)]
pub struct MinerCollateralState {
    /// Runtime field `locked`.
    pub locked: AlphaBalance,
    /// Runtime field `drain_ratio`.
    pub drain_ratio: U64F64,
    /// Runtime field `min_locked`.
    pub min_locked: AlphaBalance,
    /// Runtime field `earned`.
    pub earned: AlphaBalance,
}
record_projection!(MinerCollateralState {
    locked,
    drain_ratio,
    min_locked,
    earned
});

/// Exact field projection of runtime `CommitmentInfo`.
///
/// Source: pinned `commitments/src/types.rs:416`; compact wire fields are decoded through metadata.
#[allow(deprecated)]
#[derive(Clone, Debug, PartialEq, DecodeAsType, serde::Serialize, serde::Deserialize)]
pub struct CommitmentInfo {
    /// Runtime field `fields`.
    pub fields: Vec<Data>,
}
record_projection!(CommitmentInfo { fields });

/// Exact field projection of runtime `Registration`.
///
/// Source: pinned `commitments/src/types.rs:474`; compact wire fields are decoded through metadata.
#[allow(deprecated)]
#[derive(Clone, Debug, PartialEq, DecodeAsType, serde::Serialize, serde::Deserialize)]
pub struct CommitmentRegistration {
    /// Runtime field `deposit`.
    pub deposit: TaoBalance,
    /// Runtime field `block`.
    pub block: u32,
    /// Runtime field `info`.
    pub info: CommitmentInfo,
}
record_projection!(CommitmentRegistration {
    deposit,
    block,
    info
});

/// Exact field projection of runtime `SubnetLease`.
///
/// Source: pinned `subtensor/src/subnets/leasing.rs:40`; compact wire fields are decoded through metadata.
#[allow(deprecated)]
#[derive(Clone, Debug, PartialEq, DecodeAsType, serde::Serialize, serde::Deserialize)]
pub struct SubnetLease {
    /// Runtime field `beneficiary`.
    pub beneficiary: AccountId,
    /// Runtime field `coldkey`.
    pub coldkey: AccountId,
    /// Runtime field `hotkey`.
    pub hotkey: AccountId,
    /// Runtime field `emissions_share`.
    pub emissions_share: Percent,
    /// Runtime field `end_block`.
    pub end_block: Option<u32>,
    /// Runtime field `netuid`.
    pub netuid: NetUid,
    /// Runtime field `cost`.
    pub cost: TaoBalance,
}
record_projection!(SubnetLease {
    beneficiary,
    coldkey,
    hotkey,
    emissions_share,
    end_block,
    netuid,
    cost
});

/// Exact field projection of runtime `CrowdloanInfo`.
///
/// Source: pinned `crowdloan/src/lib.rs:58`; compact wire fields are decoded through metadata.
#[allow(deprecated)]
#[derive(Clone, Debug, PartialEq, DecodeAsType, serde::Serialize, serde::Deserialize)]
pub struct CrowdloanInfo {
    /// Runtime field `creator`.
    pub creator: AccountId,
    /// Runtime field `deposit`.
    pub deposit: TaoBalance,
    /// Runtime field `min_contribution`.
    pub min_contribution: TaoBalance,
    /// Runtime field `end`.
    pub end: u32,
    /// Runtime field `cap`.
    pub cap: TaoBalance,
    /// Runtime field `funds_account`.
    pub funds_account: AccountId,
    /// Runtime field `raised`.
    pub raised: TaoBalance,
    /// Runtime field `target_address`.
    pub target_address: Option<AccountId>,
    /// Runtime field `call`.
    pub call: Option<CrowdloanCall>,
    /// Runtime field `finalized`.
    pub finalized: bool,
    /// Runtime field `contributors_count`.
    pub contributors_count: u32,
}
record_projection!(CrowdloanInfo {
    creator,
    deposit,
    min_contribution,
    end,
    cap,
    funds_account,
    raised,
    target_address,
    call,
    finalized,
    contributors_count
});

/// Exact field projection of runtime `ProxyDefinition`.
///
/// Source: pinned `proxy/src/lib.rs:75`; compact wire fields are decoded through metadata.
#[allow(deprecated)]
#[derive(Clone, Debug, PartialEq, DecodeAsType, serde::Serialize, serde::Deserialize)]
pub struct ProxyDefinition {
    /// Runtime field `delegate`.
    pub delegate: AccountId,
    /// Runtime field `proxy_type`.
    pub proxy_type: ProxyType,
    /// Runtime field `delay`.
    pub delay: u32,
}
record_projection!(ProxyDefinition {
    delegate,
    proxy_type,
    delay
});

/// Exact field projection of runtime `Timepoint`.
///
/// Source: pinned `polkadot-sdk/frame/multisig/src/lib.rs:99`; compact wire fields are decoded through metadata.
#[allow(deprecated)]
#[derive(Clone, Debug, PartialEq, DecodeAsType, serde::Serialize, serde::Deserialize)]
pub struct MultisigTimepoint {
    /// Runtime field `height`.
    pub height: u32,
    /// Runtime field `index`.
    pub index: u32,
}
record_projection!(MultisigTimepoint { height, index });

/// Exact field projection of runtime `Multisig`.
///
/// Source: pinned `polkadot-sdk/frame/multisig/src/lib.rs:109`; compact wire fields are decoded through metadata.
#[allow(deprecated)]
#[derive(Clone, Debug, PartialEq, DecodeAsType, serde::Serialize, serde::Deserialize)]
pub struct MultisigInfo {
    /// Runtime field `when`.
    pub when: MultisigTimepoint,
    /// Runtime field `deposit`.
    pub deposit: TaoBalance,
    /// Runtime field `depositor`.
    pub depositor: AccountId,
    /// Runtime field `approvals`.
    pub approvals: Vec<AccountId>,
}
record_projection!(MultisigInfo {
    when,
    deposit,
    depositor,
    approvals
});

/// Exact field projection of runtime `BasketHolding`.
///
/// Source: pinned `subtensor/src/rpc_info/basket_info.rs:17`; compact wire fields are decoded through metadata.
#[allow(deprecated)]
#[derive(Clone, Debug, PartialEq, DecodeAsType, serde::Serialize, serde::Deserialize)]
pub struct BasketHolding {
    /// Runtime field `netuid`.
    pub netuid: NetUid,
    /// Runtime field `alpha`.
    pub alpha: AlphaBalance,
    /// Runtime field `spot_tao`.
    pub spot_tao: TaoBalance,
    /// Runtime field `realizable_tao`.
    pub realizable_tao: TaoBalance,
}
record_projection!(BasketHolding {
    netuid,
    alpha,
    spot_tao,
    realizable_tao
});

/// Exact field projection of runtime `BasketSummary`.
///
/// Source: pinned `subtensor/src/rpc_info/basket_info.rs:30`; compact wire fields are decoded through metadata.
#[allow(deprecated)]
#[derive(Clone, Debug, PartialEq, DecodeAsType, serde::Serialize, serde::Deserialize)]
pub struct BasketSummary {
    /// Runtime field `hotkey`.
    pub hotkey: AccountId,
    /// Runtime field `nav_tao`.
    pub nav_tao: TaoBalance,
    /// Runtime field `spot_nav_tao`.
    pub spot_nav_tao: TaoBalance,
    /// Runtime field `shares`.
    pub shares: u64,
    /// Runtime field `deposited_tao`.
    pub deposited_tao: TaoBalance,
    /// Runtime field `redeemed_tao`.
    pub redeemed_tao: TaoBalance,
    /// Runtime field `holdings`.
    pub holdings: Vec<BasketHolding>,
}
record_projection!(BasketSummary {
    hotkey,
    nav_tao,
    spot_nav_tao,
    shares,
    deposited_tao,
    redeemed_tao,
    holdings
});

/// Exact field projection of runtime `BasketPosition`.
///
/// Source: pinned `subtensor/src/rpc_info/basket_info.rs:55`; compact wire fields are decoded through metadata.
#[allow(deprecated)]
#[derive(Clone, Debug, PartialEq, DecodeAsType, serde::Serialize, serde::Deserialize)]
pub struct BasketPosition {
    /// Runtime field `hotkey`.
    pub hotkey: AccountId,
    /// Runtime field `beta`.
    pub beta: u64,
    /// Runtime field `beta_total`.
    pub beta_total: u64,
    /// Runtime field `nav_tao`.
    pub nav_tao: TaoBalance,
    /// Runtime field `value_tao`.
    pub value_tao: TaoBalance,
    /// Runtime field `spot_value_tao`.
    pub spot_value_tao: TaoBalance,
}
record_projection!(BasketPosition {
    hotkey,
    beta,
    beta_total,
    nav_tao,
    value_tao,
    spot_value_tao
});

/// Exact field projection of runtime `BetaPricing`.
///
/// Source: pinned `subtensor/src/rpc_info/basket_info.rs:78`; compact wire fields are decoded through metadata.
#[allow(deprecated)]
#[derive(Clone, Debug, PartialEq, DecodeAsType, serde::Serialize, serde::Deserialize)]
pub struct BetaPricing {
    /// Runtime field `hotkey`.
    pub hotkey: AccountId,
    /// Runtime field `spot_price`.
    pub spot_price: U64F64,
    /// Runtime field `display_price`.
    pub display_price: U64F64,
    /// Runtime field `stake_price`.
    pub stake_price: U64F64,
    /// Runtime field `staker_yield`.
    pub staker_yield: U64F64,
    /// Runtime field `staker_twr`.
    pub staker_twr: U64F64,
    /// Runtime field `bag_index`.
    pub bag_index: U64F64,
    /// Runtime field `stake_index`.
    pub stake_index: U64F64,
    /// Runtime field `first_block`.
    pub first_block: u64,
    /// Runtime field `provisional`.
    pub provisional: bool,
    /// Runtime field `spot_nav_tao`.
    pub spot_nav_tao: TaoBalance,
    /// Runtime field `shares`.
    pub shares: u64,
    /// Runtime field `display_shares`.
    pub display_shares: U64F64,
}
record_projection!(BetaPricing {
    hotkey,
    spot_price,
    display_price,
    stake_price,
    staker_yield,
    staker_twr,
    bag_index,
    stake_index,
    first_block,
    provisional,
    spot_nav_tao,
    shares,
    display_shares
});

/// Exact field projection of runtime `HyperparamEntry`.
///
/// Source: pinned `subtensor/src/rpc_info/subnet_info.rs:152`; compact wire fields are decoded through metadata.
#[allow(deprecated)]
#[derive(Clone, Debug, PartialEq, DecodeAsType, serde::Serialize, serde::Deserialize)]
pub struct HyperparamEntry {
    /// Runtime field `name`.
    pub name: Vec<u8>,
    /// Runtime field `value`.
    pub value: HyperparamValue,
}
record_projection!(HyperparamEntry { name => bytes, value });

/// Tagged hyperparameter value from the runtime's v3 name/value interface.
///
/// Entry names remain raw bytes, including names unknown to this SDK. A new
/// wire value variant requires an updated model and surface projection.
#[derive(Clone, Debug, PartialEq, DecodeAsType, serde::Serialize, serde::Deserialize)]
pub enum HyperparamValue {
    /// Runtime Bool value with exact wire precision.
    Bool(bool),
    /// Runtime U16 value with exact wire precision.
    U16(u16),
    /// Runtime U32 value with exact wire precision.
    U32(u32),
    /// Runtime U64 value with exact wire precision.
    U64(u64),
    /// Runtime U128 value with exact wire precision.
    U128(u128),
    /// Runtime TaoBalance value with exact wire precision.
    TaoBalance(TaoBalance),
    /// Runtime I32F32 value with exact wire precision.
    I32F32(I32F32),
    /// Runtime U64F64 value with exact wire precision.
    U64F64(U64F64),
}
impl ToDynamic for HyperparamValue {
    fn to_dynamic(&self) -> Value {
        let (name, payload) = match self {
            Self::Bool(value) => ("Bool", value.to_dynamic()),
            Self::U16(value) => ("U16", value.to_dynamic()),
            Self::U32(value) => ("U32", value.to_dynamic()),
            Self::U64(value) => ("U64", value.to_dynamic()),
            Self::U128(value) => ("U128", value.to_dynamic()),
            Self::TaoBalance(value) => ("TaoBalance", value.to_dynamic()),
            Self::I32F32(value) => ("I32F32", value.to_dynamic()),
            Self::U64F64(value) => ("U64F64", value.to_dynamic()),
        };
        Value::record(vec![(name.into(), payload)])
    }
}
impl IntoCoreValue for HyperparamValue {
    fn into_core_value(self) -> Value {
        self.to_dynamic()
    }
}

/// Runtime proxy permission category, resolved by variant name.
///
/// Source: pinned `common/src/proxy.rs`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, DecodeAsType, serde::Serialize, serde::Deserialize)]
pub enum ProxyType {
    /// The runtime's `Any` permission category.
    Any,
    /// The runtime's `Owner` permission category.
    Owner,
    /// The runtime's `NonCritical` permission category.
    NonCritical,
    /// The runtime's `NonTransfer` permission category.
    NonTransfer,
    /// The runtime's `Senate` permission category.
    Senate,
    /// The runtime's `NonFungible` permission category.
    NonFungible,
    /// The runtime's `Triumvirate` permission category.
    Triumvirate,
    /// The runtime's `Governance` permission category.
    Governance,
    /// The runtime's `Staking` permission category.
    Staking,
    /// The runtime's `Registration` permission category.
    Registration,
    /// The runtime's `Transfer` permission category.
    Transfer,
    /// The runtime's `SmallTransfer` permission category.
    SmallTransfer,
    /// The runtime's `RootWeights` permission category.
    RootWeights,
    /// The runtime's `ChildKeys` permission category.
    ChildKeys,
    /// The runtime's `SudoUncheckedSetCode` permission category.
    SudoUncheckedSetCode,
    /// The runtime's `SwapHotkey` permission category.
    SwapHotkey,
    /// The runtime's `SubnetLeaseBeneficiary` permission category.
    SubnetLeaseBeneficiary,
    /// The runtime's `RootClaim` permission category.
    RootClaim,
    /// The runtime's `BasketTrading` permission category.
    BasketTrading,
}
impl ToDynamic for ProxyType {
    fn to_dynamic(&self) -> Value {
        Value::Str(
            match self {
                Self::Any => "Any",
                Self::Owner => "Owner",
                Self::NonCritical => "NonCritical",
                Self::NonTransfer => "NonTransfer",
                Self::Senate => "Senate",
                Self::NonFungible => "NonFungible",
                Self::Triumvirate => "Triumvirate",
                Self::Governance => "Governance",
                Self::Staking => "Staking",
                Self::Registration => "Registration",
                Self::Transfer => "Transfer",
                Self::SmallTransfer => "SmallTransfer",
                Self::RootWeights => "RootWeights",
                Self::ChildKeys => "ChildKeys",
                Self::SudoUncheckedSetCode => "SudoUncheckedSetCode",
                Self::SwapHotkey => "SwapHotkey",
                Self::SubnetLeaseBeneficiary => "SubnetLeaseBeneficiary",
                Self::RootClaim => "RootClaim",
                Self::BasketTrading => "BasketTrading",
            }
            .into(),
        )
    }
}
impl IntoCoreValue for ProxyType {
    fn into_core_value(self) -> Value {
        self.to_dynamic()
    }
}

/// The runtime's bounded call representation used by crowdloan finalization.
///
/// A preimage hash remains distinct from an inline encoded call. Source: the
/// pinned Polkadot SDK's `frame/support/src/traits/preimages.rs`.
#[derive(Clone, Debug, PartialEq, Eq, DecodeAsType, serde::Serialize, serde::Deserialize)]
pub enum CrowdloanCall {
    /// A legacy preimage hash whose encoded length is unavailable.
    Legacy {
        /// The 32-byte call preimage hash.
        hash: [u8; 32],
    },
    /// An inline SCALE-encoded runtime call, bounded to 128 bytes on chain.
    Inline(Vec<u8>),
    /// A call preimage hash and upper bound on its encoded length.
    Lookup {
        /// The 32-byte call preimage hash.
        hash: [u8; 32],
        /// The maximum encoded call length recorded by the runtime.
        len: u32,
    },
}
impl ToDynamic for CrowdloanCall {
    fn to_dynamic(&self) -> Value {
        let (name, payload) = match self {
            Self::Legacy { hash } => (
                "Legacy",
                Value::record(vec![("hash".into(), Value::hex(hash))]),
            ),
            Self::Inline(bytes) => ("Inline", bytes_dynamic(bytes)),
            Self::Lookup { hash, len } => (
                "Lookup",
                Value::record(vec![
                    ("hash".into(), Value::hex(hash)),
                    ("len".into(), len.to_dynamic()),
                ]),
            ),
        };
        Value::record(vec![(name.into(), payload)])
    }
}
impl IntoCoreValue for CrowdloanCall {
    fn into_core_value(self) -> Value {
        let (name, payload) = match self {
            Self::Legacy { hash } => (
                "Legacy",
                Value::record(vec![("hash".into(), Value::Bytes(hash.to_vec()))]),
            ),
            Self::Inline(bytes) => ("Inline", Value::Bytes(bytes)),
            Self::Lookup { hash, len } => (
                "Lookup",
                Value::record(vec![
                    ("hash".into(), Value::Bytes(hash.to_vec())),
                    ("len".into(), len.into_core_value()),
                ]),
            ),
        };
        Value::record(vec![(name.into(), payload)])
    }
}
