// Derived from RaoFoundation/subtensor sdk/python/bittensor/reads/{locks,collateral}.py @ 850f2755bb05a3851f948489f0ab27b0645cebce (Apache-2.0); modified by rtao.
// Derived from RaoFoundation/subtensor sdk/python/bittensor/reads/collateral.py @ 850f2755bb05a3851f948489f0ab27b0645cebce (Apache-2.0); modified by rtao.
//! Locked-alpha accounting, conviction projections and registration collateral.

use std::collections::BTreeMap;

use futures::TryStreamExt;
use rtao_client::{ReadPlan, View, ViewExt};
use rtao_core::{DynValue, codec::Value};
use rtao_primitives::{AccountId, DynBalance, NetUid, Symbol};
use rtao_runtime::{models, runtime_apis as api, storage as st};
use serde::Serialize;

use crate::ReadError;

const DEFAULT_LOCK_RATE: u64 = 934_866;
const ONE_YEAR_BLOCKS: u128 = 7200 * 365 + 1800;

fn surface(descriptor: &'static str, detail: impl Into<String>) -> ReadError {
    ReadError::SurfaceChanged {
        descriptor,
        detail: detail.into(),
    }
}

fn balance(rao: u64, netuid: NetUid, symbols: &BTreeMap<NetUid, String>) -> DynBalance {
    let mut amount = DynBalance::from_rao(rao, netuid);
    amount.symbol = symbols.get(&netuid).map(|s| Symbol::new(s.clone()));
    amount
}

fn account(key: &DynValue, descriptor: &'static str) -> Result<AccountId, ReadError> {
    key.as_str()
        .ok_or_else(|| surface(descriptor, "map key is not an account"))?
        .parse::<AccountId>()
        .map_err(|e| surface(descriptor, e.to_string()))
}

/// One runtime-decayed lock, retaining its storage-key target and decay mode.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct ColdkeyLock {
    /// Hotkey targeted by this lock.
    pub hotkey: AccountId,
    /// Subnet currency.
    pub netuid: NetUid,
    /// Runtime-rolled locked mass.
    pub locked_alpha: DynBalance,
    /// Present false `DecayingLock` means perpetual; absence does not.
    pub is_perpetual: bool,
}

/// A target-side lock record with the contributing coldkey.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct HotkeyLock {
    /// Coldkey whose lock targets this hotkey.
    pub coldkey: AccountId,
    /// Fields shared with the coldkey-side record.
    #[serde(flatten)]
    pub lock: ColdkeyLock,
}

async fn lock_record<V: View + ?Sized>(
    view: &V,
    coldkey: AccountId,
    netuid: NetUid,
    hotkey: AccountId,
    symbols: &BTreeMap<NetUid, String>,
) -> Result<Option<ColdkeyLock>, ReadError> {
    // py: sdk/python/bittensor/reads/locks.py:33-47
    let (state, decaying) = futures::try_join!(
        view.call(
            &api::stake_info_runtime_api::GET_COLDKEY_LOCK,
            (coldkey, netuid)
        ),
        view.storage(&st::subtensor_module::DECAYING_LOCK, (coldkey, netuid)),
    )?;
    Ok(state.map(|state| ColdkeyLock {
        hotkey,
        netuid,
        locked_alpha: balance(state.locked_mass.0, netuid, symbols),
        is_perpetual: decaying == Some(false),
    }))
}

fn lock_keys(keys: &[DynValue]) -> Result<(NetUid, AccountId), ReadError> {
    let [netuid, hotkey] = keys else {
        return Err(surface(
            "SubtensorModule.Lock",
            "expected netuid and hotkey remainder",
        ));
    };
    let netuid = netuid
        .as_u128()
        .and_then(|n| u16::try_from(n).ok())
        .ok_or_else(|| surface("SubtensorModule.Lock", "netuid key is not u16"))?;
    Ok((NetUid(netuid), account(hotkey, "SubtensorModule.Lock")?))
}

/// One coldkey's lock on a subnet, absent when its storage-key target is missing.
// py: sdk/python/bittensor/reads/locks.py:67-71
pub async fn coldkey_lock<V: View + ?Sized>(
    view: &V,
    coldkey_ss58: AccountId,
    netuid: NetUid,
) -> Result<Option<ColdkeyLock>, ReadError> {
    let snapshot = view.pin().await?;
    let rows: Vec<_> = snapshot
        .storage_iter(&st::subtensor_module::LOCK, (coldkey_ss58,))
        .try_collect()
        .await?;
    for (keys, _) in rows {
        let (nid, hotkey) = lock_keys(&keys)?;
        if nid == netuid {
            return lock_record(
                &snapshot,
                coldkey_ss58,
                netuid,
                hotkey,
                &snapshot.client().token_symbols().await,
            )
            .await;
        }
    }
    Ok(None)
}

/// All locks held by one coldkey, sorted by subnet and SS58 target.
// py: sdk/python/bittensor/reads/locks.py:78-87
pub async fn locks_for_coldkey<V: View + ?Sized>(
    view: &V,
    coldkey_ss58: AccountId,
) -> Result<Vec<ColdkeyLock>, ReadError> {
    let snapshot = view.pin().await?;
    let rows: Vec<_> = snapshot
        .storage_iter(&st::subtensor_module::LOCK, (coldkey_ss58,))
        .try_collect()
        .await?;
    let mut pairs = rows
        .iter()
        .map(|(keys, _)| lock_keys(keys))
        .collect::<Result<Vec<_>, _>>()?;
    pairs.sort_by_key(|(nid, hotkey)| (*nid, hotkey.to_string()));
    let symbols = snapshot.client().token_symbols().await;
    let mut result = Vec::with_capacity(pairs.len());
    for (nid, hotkey) in pairs {
        if let Some(record) = lock_record(&snapshot, coldkey_ss58, nid, hotkey, &symbols).await? {
            result.push(record);
        }
    }
    Ok(result)
}

/// All coldkeys targeting a hotkey, sorted by their SS58 address.
// py: sdk/python/bittensor/reads/locks.py:107-116
pub async fn locks_for_hotkey<V: View + ?Sized>(
    view: &V,
    hotkey_ss58: AccountId,
    netuid: NetUid,
) -> Result<Vec<HotkeyLock>, ReadError> {
    let snapshot = view.pin().await?;
    let rows: Vec<_> = snapshot
        .storage_iter(
            &st::subtensor_module::LOCKING_COLDKEYS,
            (netuid, hotkey_ss58),
        )
        .try_collect()
        .await?;
    let mut coldkeys = Vec::with_capacity(rows.len());
    for (keys, _) in rows {
        let [key] = keys.as_slice() else {
            return Err(surface(
                "SubtensorModule.LockingColdkeys",
                "expected coldkey remainder",
            ));
        };
        coldkeys.push(account(key, "SubtensorModule.LockingColdkeys")?);
    }
    coldkeys.sort_by_key(ToString::to_string);
    let symbols = snapshot.client().token_symbols().await;
    let mut records = Vec::with_capacity(coldkeys.len());
    for coldkey in coldkeys {
        if let Some(lock) = lock_record(&snapshot, coldkey, netuid, hotkey_ss58, &symbols).await? {
            records.push(HotkeyLock { coldkey, lock });
        }
    }
    Ok(records)
}

/// Whether bit zero of the coldkey's native u128 account flags is set.
// py: sdk/python/bittensor/reads/locks.py:131-132
pub async fn accepts_locked_alpha<V: View + ?Sized>(
    view: &V,
    coldkey_ss58: AccountId,
) -> Result<bool, ReadError> {
    Ok(view
        .storage(&st::subtensor_module::ACCOUNT_FLAGS, (coldkey_ss58,))
        .await?
        & 1
        != 0)
}

/// Runtime-rolled hotkey conviction, discarding fractional rao exactly.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct HotkeyConviction {
    /// Whole rao of alpha conviction.
    pub conviction_alpha: DynBalance,
}

/// One hotkey's runtime conviction with its U64F64 fraction discarded.
// py: sdk/python/bittensor/reads/locks.py:150-154
pub async fn hotkey_conviction<V: View + ?Sized>(
    view: &V,
    hotkey_ss58: AccountId,
    netuid: NetUid,
) -> Result<HotkeyConviction, ReadError> {
    let snapshot = view.pin().await?;
    let value = snapshot
        .call(
            &api::stake_info_runtime_api::GET_HOTKEY_CONVICTION,
            (hotkey_ss58, netuid),
        )
        .await?;
    Ok(HotkeyConviction {
        conviction_alpha: balance(
            (value.bits >> 64) as u64,
            netuid,
            &snapshot.client().token_symbols().await,
        ),
    })
}

// Python preserves integer mass in perpetual buckets; turning it into f64
// would corrupt u64::MAX and strict owner-gate comparisons before projection.
#[derive(Clone, Copy, Debug)]
enum Number {
    Integer(u128),
    Float(f64),
}

impl Number {
    fn float(self) -> f64 {
        match self {
            Self::Integer(n) => n as f64,
            Self::Float(n) => n,
        }
    }

    fn add(self, other: Self) -> Self {
        match (self, other) {
            (Self::Integer(a), Self::Integer(b)) => Self::Integer(a + b),
            (a, b) => Self::Float(a.float() + b.float()),
        }
    }

    fn rao(self) -> Result<u64, ReadError> {
        match self {
            Self::Integer(n) => u64::try_from(n).map_err(|_| {
                surface(
                    "subnet_convictions",
                    "projection exceeds native u64 balance",
                )
            }),
            Self::Float(n) => checked_float_rao(n, "subnet_convictions"),
        }
    }
}

// CPython 3.12.11 Python/bltinmodule.c::builtin_sum_impl on the pinned LP64
// oracle: integer prefixes remain exact. Its float fast path compensates float
// items, but adds C-long integers directly. Integer-prefix overflow, or an
// oversized integer in the float path, switches permanently to generic adds.
#[derive(Clone, Copy, Debug)]
enum PythonSum {
    Integer(u128),
    Float { sum: f64, correction: f64 },
    Generic(Number),
}

impl Default for PythonSum {
    fn default() -> Self {
        Self::Integer(0)
    }
}

fn corrected(sum: f64, correction: f64) -> f64 {
    if correction != 0.0 && correction.is_finite() {
        sum + correction
    } else {
        sum
    }
}

impl PythonSum {
    fn add(&mut self, item: Number) {
        const LONG_MAX: u128 = i64::MAX as u128;
        *self = match (*self, item) {
            (Self::Integer(sum), Number::Integer(value)) if value <= LONG_MAX - sum => {
                Self::Integer(sum + value)
            }
            (Self::Integer(sum), Number::Integer(value)) => {
                Self::Generic(Number::Integer(sum + value))
            }
            (Self::Integer(sum), Number::Float(value)) => Self::Float {
                sum: sum as f64 + value,
                correction: 0.0,
            },
            (Self::Float { sum, correction }, Number::Float(value)) => {
                let next = sum + value;
                let correction = correction
                    + if sum.abs() >= value.abs() {
                        (sum - next) + value
                    } else {
                        (value - next) + sum
                    };
                Self::Float {
                    sum: next,
                    correction,
                }
            }
            (Self::Float { sum, correction }, Number::Integer(value)) if value <= LONG_MAX => {
                Self::Float {
                    sum: sum + value as f64,
                    correction,
                }
            }
            (Self::Float { sum, correction }, Number::Integer(value)) => {
                Self::Generic(Number::Float(corrected(sum, correction) + value as f64))
            }
            (Self::Generic(sum), value) => Self::Generic(sum.add(value)),
        };
    }

    fn finish(self) -> Number {
        match self {
            Self::Integer(sum) => Number::Integer(sum),
            Self::Float { sum, correction } => Number::Float(corrected(sum, correction)),
            Self::Generic(sum) => sum,
        }
    }
}

fn python_sum(values: impl IntoIterator<Item = Number>) -> Number {
    let mut sum = PythonSum::default();
    for value in values {
        sum.add(value);
    }
    sum.finish()
}

fn checked_float_rao(value: f64, descriptor: &'static str) -> Result<u64, ReadError> {
    let value = value.trunc();
    if !(0.0..18_446_744_073_709_551_616.0).contains(&value) {
        return Err(surface(
            descriptor,
            "derived amount exceeds native u64 balance",
        ));
    }
    Ok(value as u64)
}

#[derive(Clone, Copy, Debug)]
struct LockBucket {
    mass: Number,
    conviction: Number,
    perpetual: bool,
    owner: bool,
}

fn eligible_alpha(alpha_out: u64, protocol_alpha: u64, burned: u64) -> u64 {
    // py: sdk/python/bittensor/reads/locks.py:26-30
    alpha_out
        .saturating_sub(protocol_alpha)
        .saturating_sub(burned)
}

fn integer_ratio(numerator: u128, denominator: u64) -> f64 {
    // All callers' block deltas are bounded by ten u64 timescales, so the
    // numerator fits i128. Preserve Python integer division's single rounding.
    rtao_primitives::ratio::integer_division(numerator as i128, denominator).unwrap_or(f64::NAN)
}

fn exp_decay(dt: u128, tau: u64) -> f64 {
    // py: sdk/python/bittensor/reads/locks.py:188-193
    if dt == 0 {
        1.0
    } else if tau == 0 {
        0.0
    } else {
        (-integer_ratio(dt, tau).min(40.0)).exp()
    }
}

fn project(bucket: LockBucket, dt: u128, unlock: u64, maturity: u64) -> LockBucket {
    // py: sdk/python/bittensor/reads/locks.py:196-226
    let unlock_decay = exp_decay(dt, unlock);
    let maturity_decay = exp_decay(dt, maturity);
    let mass = if bucket.perpetual {
        bucket.mass
    } else {
        Number::Float(bucket.mass.float() * unlock_decay)
    };
    let from_mass = if bucket.perpetual {
        bucket.mass.float() * (1.0 - maturity_decay)
    } else if unlock == maturity {
        if maturity == 0 {
            0.0
        } else {
            bucket.mass.float() * integer_ratio(dt, maturity) * maturity_decay
        }
    } else if unlock == 0 || maturity == 0 {
        0.0
    } else {
        let difference = i128::from(unlock) - i128::from(maturity);
        let gamma = unlock as f64 * (unlock_decay - maturity_decay) / difference as f64;
        if gamma > 0.0 {
            bucket.mass.float() * gamma
        } else {
            0.0
        }
    };
    let conviction = if bucket.owner {
        mass
    } else {
        Number::Float(bucket.conviction.float() * maturity_decay + from_mass)
    };
    LockBucket {
        mass,
        conviction,
        ..bucket
    }
}

fn conviction_at(buckets: &[LockBucket], dt: u128, unlock: u64, maturity: u64) -> Number {
    python_sum(
        buckets
            .iter()
            .map(|b| project(*b, dt, unlock, maturity).conviction),
    )
}

fn clears_gate(conviction: Number, eligible: u64) -> bool {
    // py: sdk/python/bittensor/reads/locks.py:242-245. The integer RHS stays
    // exact, including Python's float-vs-int comparison near u64::MAX.
    let rhs = u128::from(eligible) * 18;
    match conviction {
        Number::Integer(n) => n > rhs / 100,
        Number::Float(n) => {
            let lhs = n * 100.0;
            if lhs.is_nan() || lhs <= 0.0 {
                return false;
            }
            if lhs >= u128::MAX as f64 {
                return true;
            }
            let integer = lhs as u128;
            integer > rhs || integer == rhs && lhs.fract() > 0.0
        }
    }
}

fn blocks_until(buckets: &[LockBucket], eligible: u64, unlock: u64, maturity: u64) -> Option<u128> {
    // py: sdk/python/bittensor/reads/locks.py:248-286
    if eligible == 0 {
        return None;
    }
    let clears = |dt: u128| clears_gate(conviction_at(buckets, dt, unlock, maturity), eligible);
    if clears(0) {
        return Some(0);
    }
    let horizon = u128::from(unlock.max(maturity).max(1)) * 10;
    let step = (horizon / 4000).max(1);
    let mut previous = 0;
    let mut dt = step;
    while dt <= horizon {
        if clears(dt) {
            let (mut low, mut high) = (previous, dt);
            while high - low > 1 {
                let mid = (low + high) / 2;
                if clears(mid) {
                    high = mid;
                } else {
                    low = mid;
                }
            }
            return Some(high);
        }
        previous = dt;
        dt += step;
    }
    None
}

/// Per-hotkey rolled lock totals and its individual ownership-gate projection.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct ConvictionEntry {
    /// Target hotkey.
    pub hotkey: AccountId,
    /// Whether this is the current owner's hotkey.
    pub is_owner: bool,
    /// Rolled locked mass, truncated only after summing buckets.
    pub locked_alpha: DynBalance,
    /// Rolled conviction, truncated only after summing buckets.
    pub conviction_alpha: DynBalance,
    /// Conviction divided by the unrounded 18% threshold, absent if zero.
    pub pct_of_threshold: Option<f64>,
    /// First strict gate crossing, absent if never reached under this projection.
    pub blocks_to_threshold: Option<u128>,
}

/// Subnet accounting and per-hotkey ownership projections at one block.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct SubnetConvictions {
    /// Queried subnet.
    pub netuid: NetUid,
    /// Pinned block number.
    pub block: u64,
    /// Alpha outstanding.
    pub alpha_out: DynBalance,
    /// Alpha held by the protocol.
    pub protocol_alpha: DynBalance,
    /// Burned alpha.
    pub alpha_burned: DynBalance,
    /// Outstanding minus protocol and burned alpha, saturating at zero.
    pub eligible_alpha: DynBalance,
    /// Whole-rao presentation of the 18% threshold.
    pub threshold_alpha: DynBalance,
    /// Rolled locked mass across all buckets.
    pub total_locked_alpha: DynBalance,
    /// Rolled conviction across all buckets; never used as a takeover gate.
    pub total_conviction_alpha: DynBalance,
    /// First hotkey after stable descending whole-rao conviction sorting.
    pub leader_hotkey: Option<AccountId>,
    /// Individual gate crossing of the current leader.
    pub leader_blocks_to_threshold: Option<u128>,
    /// Effective unlock timescale in blocks.
    pub unlock_rate: u64,
    /// Effective maturity timescale in blocks.
    pub maturity_rate: u64,
    /// Raw owner hotkey, including the metadata's all-zero default.
    pub owner_hotkey: Option<AccountId>,
    /// Native registration block counter.
    pub registered_at: u64,
    /// Registration plus the pallet's one-year age, without monetary narrowing.
    pub ownership_changeable_at_block: u128,
    /// Hotkeys sorted by descending whole-rao conviction, preserving scan ties.
    pub hotkeys: Vec<ConvictionEntry>,
}

#[derive(Default)]
struct LockGroups {
    positions: BTreeMap<AccountId, usize>,
    groups: Vec<(AccountId, Vec<LockBucket>)>,
}

impl LockGroups {
    fn add(&mut self, hotkey: AccountId, bucket: LockBucket) {
        let index = *self.positions.entry(hotkey).or_insert_with(|| {
            self.groups.push((hotkey, Vec::new()));
            self.groups.len() - 1
        });
        self.groups[index].1.push(bucket);
    }
}

fn rolled(
    state: models::LockState,
    now: u64,
    unlock: u64,
    maturity: u64,
    perpetual: bool,
    owner: bool,
) -> LockBucket {
    let bucket = LockBucket {
        mass: Number::Integer(u128::from(state.locked_mass.0)),
        conviction: Number::Float(state.conviction.to_f64()),
        perpetual,
        owner,
    };
    project(
        bucket,
        u128::from(now.saturating_sub(state.last_update)),
        unlock,
        maturity,
    )
}

/// Subnet-wide lock accounting and the pinned Python float projection.
// py: sdk/python/bittensor/reads/locks.py:312-419
pub async fn subnet_convictions<V: View + ?Sized>(
    view: &V,
    netuid: NetUid,
) -> Result<SubnetConvictions, ReadError> {
    let snapshot = view.pin().await?;
    let hotkey_rows: Vec<_> = snapshot
        .storage_iter(&st::subtensor_module::HOTKEY_LOCK, (netuid,))
        .try_collect()
        .await?;
    let decaying_rows: Vec<_> = snapshot
        .storage_iter(&st::subtensor_module::DECAYING_HOTKEY_LOCK, (netuid,))
        .try_collect()
        .await?;
    let (
        owner_lock,
        decaying_owner_lock,
        owner_hotkey,
        alpha_out,
        protocol_alpha,
        unlock_rate,
        maturity_rate,
        registered_at,
    ) = snapshot
        .fetch((
            st::subtensor_module::OWNER_LOCK.at((netuid,)),
            st::subtensor_module::DECAYING_OWNER_LOCK.at((netuid,)),
            st::subtensor_module::SUBNET_OWNER_HOTKEY.at((netuid,)),
            st::subtensor_module::SUBNET_ALPHA_OUT.at((netuid,)),
            st::subtensor_module::SUBNET_PROTOCOL_ALPHA.at((netuid,)),
            st::subtensor_module::UNLOCK_RATE.at(()),
            st::subtensor_module::MATURITY_RATE.at(()),
            st::subtensor_module::NETWORK_REGISTERED_AT.at((netuid,)),
        ))
        .await?;
    // AlphaAssets is outside the generated Rust storage registry; the dynamic
    // named read still authenticates and decodes through this snapshot's metadata.
    let alpha_burned = snapshot
        .storage_dyn(
            "AlphaAssets",
            "AlphaBurned",
            &[Value::Uint(u128::from(netuid.0))],
        )
        .await?;
    let alpha_burned = alpha_burned
        .as_u128()
        .and_then(|n| u64::try_from(n).ok())
        .ok_or_else(|| {
            surface(
                "AlphaAssets.AlphaBurned",
                "amount is not a native u64 balance",
            )
        })?;
    let unlock_rate = if unlock_rate == 0 {
        DEFAULT_LOCK_RATE
    } else {
        unlock_rate
    };
    let maturity_rate = if maturity_rate == 0 {
        DEFAULT_LOCK_RATE
    } else {
        maturity_rate
    };
    let now = snapshot.block().number;
    // Python str(AccountId(0)) is nonempty; retain raw default rather than
    // interpreting it as the typed API's absent-owner convention.
    let owner = *owner_hotkey.raw();
    let owner_hotkey = Some(owner);
    let mut groups = LockGroups::default();
    for (rows, perpetual) in [(hotkey_rows, true), (decaying_rows, false)] {
        for (keys, state) in rows {
            let [key] = keys.as_slice() else {
                return Err(surface("subnet_convictions", "expected hotkey remainder"));
            };
            groups.add(
                account(key, "subnet_convictions")?,
                rolled(state, now, unlock_rate, maturity_rate, perpetual, false),
            );
        }
    }
    for (state, perpetual) in [(owner_lock, true), (decaying_owner_lock, false)] {
        if let Some(state) = state {
            groups.add(
                owner,
                rolled(state, now, unlock_rate, maturity_rate, perpetual, true),
            );
        }
    }
    let eligible = eligible_alpha(alpha_out.0, protocol_alpha.0, alpha_burned);
    let threshold = rtao_primitives::ratio::integer_division(i128::from(eligible) * 18, 100)
        .ok_or_else(|| surface("subnet_convictions", "invalid threshold denominator"))?;
    let symbols = snapshot.client().token_symbols().await;
    let amount = |n| balance(n, netuid, &symbols);
    let mut hotkeys = Vec::with_capacity(groups.groups.len());
    let mut total_mass = PythonSum::default();
    let mut total_conviction = PythonSum::default();
    for (hotkey, buckets) in groups.groups {
        let mut mass = PythonSum::default();
        let mut conviction = PythonSum::default();
        for bucket in &buckets {
            mass.add(bucket.mass);
            conviction.add(bucket.conviction);
            total_mass.add(bucket.mass);
            total_conviction.add(bucket.conviction);
        }
        let mass = mass.finish();
        let conviction = conviction.finish();
        hotkeys.push(ConvictionEntry {
            hotkey,
            is_owner: owner_hotkey == Some(hotkey),
            locked_alpha: amount(mass.rao()?),
            conviction_alpha: amount(conviction.rao()?),
            pct_of_threshold: (threshold != 0.0).then(|| conviction.float() / threshold),
            blocks_to_threshold: blocks_until(&buckets, eligible, unlock_rate, maturity_rate),
        });
    }
    hotkeys.sort_by_key(|entry| std::cmp::Reverse(entry.conviction_alpha.rao));
    Ok(SubnetConvictions {
        netuid,
        block: now,
        alpha_out: amount(alpha_out.0),
        protocol_alpha: amount(protocol_alpha.0),
        alpha_burned: amount(alpha_burned),
        eligible_alpha: amount(eligible),
        threshold_alpha: amount(checked_float_rao(threshold, "subnet_convictions")?),
        total_locked_alpha: amount(total_mass.finish().rao()?),
        total_conviction_alpha: amount(total_conviction.finish().rao()?),
        leader_hotkey: hotkeys.first().map(|entry| entry.hotkey),
        leader_blocks_to_threshold: hotkeys.first().and_then(|entry| entry.blocks_to_threshold),
        unlock_rate,
        maturity_rate,
        owner_hotkey,
        registered_at,
        ownership_changeable_at_block: u128::from(registered_at) + ONE_YEAR_BLOCKS,
        hotkeys,
    })
}

/// Collateral position and its derived draining/capture accounting.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct MinerCollateral {
    /// Miner hotkey.
    pub hotkey: AccountId,
    /// Bond-owning coldkey, which may differ from a nominator's coldkey.
    pub coldkey: AccountId,
    /// Bond currency.
    pub netuid: NetUid,
    /// Current locked amount.
    pub locked_alpha: DynBalance,
    /// Self-maintained collateral floor.
    pub min_locked_alpha: DynBalance,
    /// Lifetime emission earned by this position.
    pub earned_alpha: DynBalance,
    /// Snapshot alpha released per alpha earned.
    pub drain_ratio: f64,
    /// Positive lock excess above the floor.
    pub headroom_alpha: DynBalance,
    /// Positive floor excess above the lock.
    pub shortfall_alpha: DynBalance,
    /// Whole rao of emission needed to release the headroom.
    pub releasable_work_alpha: DynBalance,
}

/// Collateral record annotated with its current optional UID.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct SubnetCollateral {
    /// Position accounting.
    #[serde(flatten)]
    pub collateral: MinerCollateral,
    /// Registration UID; absence preserves collateral after deregistration.
    pub uid: Option<u16>,
}

fn collateral_record(
    netuid: NetUid,
    hotkey: AccountId,
    coldkey: AccountId,
    state: models::MinerCollateralState,
    symbols: &BTreeMap<NetUid, String>,
) -> Result<MinerCollateral, ReadError> {
    // py: sdk/python/bittensor/reads/collateral.py:28-56
    let headroom = state.locked.0.saturating_sub(state.min_locked.0);
    let shortfall = state.min_locked.0.saturating_sub(state.locked.0);
    let drain_ratio = state.drain_ratio.to_f64();
    let releasable = if drain_ratio > 0.0 {
        checked_float_rao(
            headroom as f64 / drain_ratio,
            "SubtensorModule.MinerCollateral",
        )?
    } else {
        0
    };
    let amount = |n| balance(n, netuid, symbols);
    Ok(MinerCollateral {
        hotkey,
        coldkey,
        netuid,
        locked_alpha: amount(state.locked.0),
        min_locked_alpha: amount(state.min_locked.0),
        earned_alpha: amount(state.earned.0),
        drain_ratio,
        headroom_alpha: amount(headroom),
        shortfall_alpha: amount(shortfall),
        releasable_work_alpha: amount(releasable),
    })
}

/// A specific bonded position; omitted coldkey resolves through the hotkey owner.
// py: sdk/python/bittensor/reads/collateral.py:86-93
pub async fn miner_collateral<V: View + ?Sized>(
    view: &V,
    netuid: NetUid,
    hotkey_ss58: AccountId,
    coldkey_ss58: Option<AccountId>,
) -> Result<Option<MinerCollateral>, ReadError> {
    let snapshot = view.pin().await?;
    let coldkey = match coldkey_ss58 {
        Some(coldkey) => coldkey,
        None => *snapshot
            .storage(&st::subtensor_module::OWNER, (hotkey_ss58,))
            .await?
            .raw(),
    };
    let state = snapshot
        .storage(
            &st::subtensor_module::MINER_COLLATERAL,
            (netuid, hotkey_ss58, coldkey),
        )
        .await?;
    let symbols = snapshot.client().token_symbols().await;
    state
        .map(|state| collateral_record(netuid, hotkey_ss58, coldkey, state, &symbols))
        .transpose()
}

/// Standing subnet collateral, including deregistered hotkeys, sorted by lock.
// py: sdk/python/bittensor/reads/collateral.py:111-129
pub async fn subnet_collateral<V: View + ?Sized>(
    view: &V,
    netuid: NetUid,
) -> Result<Vec<SubnetCollateral>, ReadError> {
    let snapshot = view.pin().await?;
    let rows: Vec<_> = snapshot
        .storage_iter(&st::subtensor_module::MINER_COLLATERAL, (netuid,))
        .try_collect()
        .await?;
    let symbols = snapshot.client().token_symbols().await;
    let mut records = Vec::with_capacity(rows.len());
    let mut uids = ReadPlan::new();
    for (keys, state) in rows {
        let [hotkey, coldkey] = keys.as_slice() else {
            return Err(surface(
                "SubtensorModule.MinerCollateral",
                "expected hotkey/coldkey remainder",
            ));
        };
        let hotkey = account(hotkey, "SubtensorModule.MinerCollateral")?;
        let coldkey = account(coldkey, "SubtensorModule.MinerCollateral")?;
        let collateral = collateral_record(netuid, hotkey, coldkey, state, &symbols)?;
        uids.push(
            st::subtensor_module::UIDS.surface(),
            vec![
                Value::Uint(u128::from(netuid.0)),
                Value::Str(hotkey.to_string()),
            ],
        );
        records.push(collateral);
    }
    let uids = snapshot.fetch(uids).await?;
    let mut records = records
        .into_iter()
        .zip(uids)
        .map(|(collateral, uid)| {
            let uid = if matches!(uid.as_core(), Value::Null) {
                None
            } else {
                Some(
                    uid.as_u128()
                        .and_then(|n| u16::try_from(n).ok())
                        .ok_or_else(|| surface("SubtensorModule.Uids", "uid is not u16"))?,
                )
            };
            Ok(SubnetCollateral { collateral, uid })
        })
        .collect::<Result<Vec<_>, ReadError>>()?;
    records.sort_by_key(|record| std::cmp::Reverse(record.collateral.locked_alpha.rao));
    Ok(records)
}

/// Effective registration collateral settings.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct CollateralPolicy {
    /// Queried subnet.
    pub netuid: NetUid,
    /// Fraction of registration price locked rather than burned.
    pub lock_share: f64,
    /// Alpha released per alpha earned.
    pub drain_ratio: f64,
}

/// Read both collateral parameters at one immutable block.
// py: sdk/python/bittensor/reads/collateral.py:146-156
pub async fn collateral_policy<V: View + ?Sized>(
    view: &V,
    netuid: NetUid,
) -> Result<CollateralPolicy, ReadError> {
    let (lock_share, drain_ratio) = view
        .fetch((
            st::subtensor_module::COLLATERAL_LOCK_SHARE.at((netuid,)),
            st::subtensor_module::COLLATERAL_DRAIN_RATIO.at((netuid,)),
        ))
        .await?;
    Ok(CollateralPolicy {
        netuid,
        lock_share: f64::from(lock_share) / f64::from(u16::MAX),
        drain_ratio: drain_ratio.to_f64(),
    })
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    fn bucket(mass: u64, conviction: f64, perpetual: bool, owner: bool) -> LockBucket {
        LockBucket {
            mass: Number::Integer(u128::from(mass)),
            conviction: Number::Float(conviction),
            perpetual,
            owner,
        }
    }

    // pyt: unit/test_v446_accounting_and_commitments.py::test_eligible_alpha_excludes_protocol_and_burned_balances
    #[test]
    fn eligible_alpha_excludes_protocol_and_burned_balances() {
        assert_eq!(eligible_alpha(8_000_000, 1_000_000, 1_000_000), 6_000_000);
        assert_eq!(eligible_alpha(100, 80, 30), 0);
        assert_eq!(eligible_alpha(u64::MAX, u64::MAX, u64::MAX), 0);
        assert_eq!(eligible_alpha(u64::MAX, 1, 2), u64::MAX - 3);
    }

    // pyt: unit/test_lock_ownership_gate.py::test_gate_is_strictly_above_18_percent
    #[test]
    fn gate_is_strictly_above_18_percent() {
        for n in [1799, 1800, 1801] {
            assert_eq!(clears_gate(Number::Integer(n), 10_000), n == 1801);
            assert_eq!(clears_gate(Number::Float(n as f64), 10_000), n == 1801);
        }
        // Exact owner masses and RHS products must not round at 2^53 or 2^64.
        let eligible = u64::MAX;
        let boundary = u128::from(eligible) * 18 / 100;
        assert!(!clears_gate(Number::Integer(boundary), eligible));
        assert!(clears_gate(Number::Integer(boundary + 1), eligible));
        // Python compares a represented float with an exact integer RHS.
        assert!(clears_gate(Number::Float(1.0), 5));
        assert!(!clears_gate(Number::Float(f64::NAN), 5));
    }

    // pyt: unit/test_lock_ownership_gate.py::test_conviction_pinned_at_exactly_18_percent_never_clears
    #[test]
    fn conviction_pinned_at_exactly_18_percent_never_clears() {
        assert_eq!(
            blocks_until(&[bucket(1800, 1800.0, true, true)], 10_000, 1000, 1000),
            None
        );
        assert_eq!(
            blocks_until(&[bucket(1801, 1801.0, true, true)], 10_000, 1000, 1000),
            Some(0)
        );
    }

    // pyt: unit/test_lock_ownership_gate.py::test_crossing_block_is_first_strict_clearance
    #[test]
    fn crossing_block_is_first_strict_clearance() {
        let buckets = [bucket(3600, 0.0, true, false)];
        let crossing = blocks_until(&buckets, 10_000, 1000, 1000).unwrap();
        // 3600 * (1-exp(-t/1000)) > 1800 first holds at ceil(1000 ln 2).
        assert_eq!(crossing, 694);
        assert!(clears_gate(
            conviction_at(&buckets, crossing, 1000, 1000),
            10_000
        ));
        assert!(!clears_gate(
            conviction_at(&buckets, crossing - 1, 1000, 1000),
            10_000
        ));
    }

    // pyt: unit/test_lock_ownership_gate.py::test_zero_eligible_alpha_never_clears
    #[test]
    fn zero_eligible_alpha_never_clears() {
        assert_eq!(
            blocks_until(&[bucket(500, 500.0, true, true)], 0, 1000, 1000),
            None
        );
    }

    #[test]
    fn projections_cover_decay_equal_rates_distinct_rates_owner_and_degenerate_rates() {
        assert_eq!(exp_decay(0, 0), 1.0);
        assert_eq!(exp_decay(1, 0), 0.0);
        assert_eq!(exp_decay(1000, 1), (-40.0f64).exp());
        let input = bucket(1000, 100.0, false, false);
        let equal = project(input, 100, 100, 100);
        assert!((equal.mass.float() - 1000.0 / std::f64::consts::E).abs() < 1e-10);
        assert!((equal.conviction.float() - 1100.0 / std::f64::consts::E).abs() < 1e-10);
        let distinct = project(input, 100, 200, 100);
        let analytical = 100.0 * (-1.0f64).exp() + 2000.0 * ((-0.5f64).exp() - (-1.0f64).exp());
        assert!((distinct.conviction.float() - analytical).abs() < 1e-10);
        let owner = project(bucket(1000, 1.0, false, true), 100, 100, 200);
        assert_eq!(owner.mass.float(), owner.conviction.float());
        assert_eq!(project(input, 10, 0, 0).conviction.float(), 0.0);
        let peak = [bucket(10_000, 0.0, false, false)];
        assert!(blocks_until(&peak, 10_000, 1000, 1000).is_some());
        // A decaying lock peaks below the gate even though its initial mass exceeds it.
        assert_eq!(
            blocks_until(&[bucket(2000, 0.0, false, false)], 10_000, 1000, 1000),
            None
        );
    }

    #[test]
    fn perpetual_owner_mass_and_checked_projection_amounts_preserve_native_bounds() {
        let projected = project(bucket(u64::MAX, 0.0, true, true), 1000, 1000, 1000);
        assert_eq!(projected.mass.rao().unwrap(), u64::MAX);
        assert_eq!(projected.conviction.rao().unwrap(), u64::MAX);
        assert!(Number::Integer(u128::from(u64::MAX) + 1).rao().is_err());
        assert_eq!(checked_float_rao(17.9, "test").unwrap(), 17);
        assert!(checked_float_rao(18_446_744_073_709_551_616.0, "test").is_err());
        assert!(checked_float_rao(f64::INFINITY, "test").is_err());
    }

    #[test]
    fn adjacent_native_rates_stay_distinct_and_long_projection_counters_do_not_narrow() {
        // Both rates round to the same f64, but Python compares their exact ints.
        let distinct = project(bucket(1000, 0.0, false, false), 1, u64::MAX, u64::MAX - 1);
        assert_eq!(distinct.conviction.float(), 0.0);
        let equal = project(bucket(1000, 0.0, false, false), 1, u64::MAX, u64::MAX);
        assert!(equal.conviction.float() > 0.0);

        let buckets = [bucket(2000, 0.0, true, false)];
        let crossing = blocks_until(&buckets, 10_000, u64::MAX, u64::MAX).unwrap();
        assert!(crossing > u128::from(u64::MAX) * 2);
        assert!(crossing < u128::from(u64::MAX) * 3);
        assert!(clears_gate(
            conviction_at(&buckets, crossing, u64::MAX, u64::MAX),
            10_000
        ));
        assert!(!clears_gate(
            conviction_at(&buckets, crossing - 1, u64::MAX, u64::MAX),
            10_000
        ));
    }

    #[test]
    fn pinned_python_sum_compensates_floats_and_retains_mixed_integer_fast_paths() {
        let large = 1u128 << 53;
        let floats = [
            Number::Float(large as f64),
            Number::Float(1.0),
            Number::Float(1.0),
        ];
        assert_eq!(python_sum(floats).rao().unwrap(), (1u64 << 53) + 2);
        // CPython 3.12.11 adds an integer item directly in its float fast path.
        let mixed = [
            Number::Float(large as f64),
            Number::Integer(1),
            Number::Float(1.0),
        ];
        assert_eq!(python_sum(mixed).rao().unwrap(), 1u64 << 53);
        // Before the first float, the integer prefix itself must remain exact.
        let prefix = [
            Number::Integer(large),
            Number::Integer(1),
            Number::Float(1.0),
        ];
        assert_eq!(python_sum(prefix).rao().unwrap(), 1u64 << 53);
        assert!(
            matches!(python_sum([Number::Integer(u128::from(u64::MAX) - 1), Number::Integer(1)]), Number::Integer(n) if n == u128::from(u64::MAX))
        );

        // Both transitions to CPython's generic loop disable later compensation.
        let overflowed_prefix = [
            Number::Integer(i64::MAX as u128),
            Number::Integer(1),
            Number::Float(1024.0),
            Number::Float(1024.0),
        ];
        assert_eq!(python_sum(overflowed_prefix).rao().unwrap(), 1u64 << 63);
        let oversized_item = [
            Number::Float(0.0),
            Number::Integer(1u128 << 63),
            Number::Float(1024.0),
            Number::Float(1024.0),
        ];
        assert_eq!(python_sum(oversized_item).rao().unwrap(), 1u64 << 63);
        assert_eq!(
            python_sum([Number::Float(f64::MAX), Number::Float(f64::MAX)]).float(),
            f64::INFINITY
        );

        // The ownership search has its own Python sum, independently of display totals.
        let buckets = [
            bucket(0, large as f64, true, false),
            bucket(0, 1.0, true, false),
            bucket(0, 1.0, true, false),
        ];
        assert_eq!(
            conviction_at(&buckets, 0, 1000, 1000).rao().unwrap(),
            (1u64 << 53) + 2
        );
    }
}
