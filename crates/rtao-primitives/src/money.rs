// Derived from RaoFoundation/subtensor sdk/python/bittensor/balance.py @ 850f2755bb05a3851f948489f0ab27b0645cebce (Apache-2.0); modified by rtao.
// Derived from RaoFoundation/subtensor sdk/python/bittensor/intents/_money.py @ 850f2755bb05a3851f948489f0ab27b0645cebce (Apache-2.0); modified by rtao.
// Derived from RaoFoundation/subtensor sdk/python/tests/unit/test_balance_properties.py @ 850f2755bb05a3851f948489f0ab27b0645cebce (Apache-2.0); modified by rtao.
//! Exact amounts and currency-checked balances.
//!
//! All balances store the chain's native non-negative `u64` rao. Arithmetic
//! reports overflow and underflow instead of wrapping, saturating, or extending
//! the range. Signed and arbitrary-size Python balance behavior is excluded
//! by the user's native-balance decision (Q25; D-035).
//!
//! Ported from `sdk/python/bittensor/balance.py` and
//! `sdk/python/bittensor/intents/_money.py` at upstream `v478`, commit
//! `850f2755bb05a3851f948489f0ab27b0645cebce` (Apache-2.0). Property-test provenance
//! follows `sdk/python/tests/unit/test_balance_properties.py` at that commit.

#![deny(clippy::arithmetic_side_effects)]

use std::borrow::Cow;
use std::cmp::Ordering;
use std::fmt;
use std::hash::{Hash, Hasher};

use crate::{AlphaBalance, NetUid, TaoBalance, UnitError};

/// Number of base units in one TAO or one alpha token.
pub const RAO_PER_TAO: u64 = 1_000_000_000;

mod sealed {
    pub trait Sealed {}
}

/// Currency marker for a [`Balance`].
///
/// The sealed implementations keep TAO and subnet alpha distinct at compile
/// time. Alpha additionally carries the subnet identifier at runtime.
pub trait Unit: sealed::Sealed + Copy + Send + Sync + 'static {
    /// Context that identifies a currency within this unit family.
    type Ctx: Copy + Eq + Hash + fmt::Debug + Send + Sync;
}

/// Marker for TAO, the currency of root netuid zero.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Tao {}

/// Marker for alpha, the currency of a non-root subnet.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Alpha {}

impl sealed::Sealed for Tao {}
impl sealed::Sealed for Alpha {}
impl Unit for Tao {
    type Ctx = ();
}
impl Unit for Alpha {
    type Ctx = NetUid;
}

/// A non-negative integer amount in a statically selected currency family.
///
/// Arithmetic is checked and explicit; `Add`, `Sub` and `Sum` are deliberately
/// absent. TAO and alpha therefore cannot be combined by an operator.
///
/// ```compile_fail
/// use rtao_primitives::{Alpha, Balance, NetUid, Tao};
/// let tao = Balance::<Tao>::from_rao(1);
/// let alpha = Balance::<Alpha>::new(NetUid(1), 1).unwrap();
/// let _ = tao.checked_add(alpha);
/// ```
pub struct Balance<U: Unit> {
    rao: u64,
    ctx: U::Ctx,
}

impl<U: Unit> Copy for Balance<U> {}
impl<U: Unit> Clone for Balance<U> {
    fn clone(&self) -> Self {
        *self
    }
}
impl<U: Unit> PartialEq for Balance<U> {
    fn eq(&self, other: &Self) -> bool {
        self.rao == other.rao && self.ctx == other.ctx
    }
}
impl<U: Unit> Eq for Balance<U> {}
impl<U: Unit> Hash for Balance<U> {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.rao.hash(state);
        self.ctx.hash(state);
    }
}
impl<U: Unit> fmt::Debug for Balance<U> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Balance")
            .field("rao", &self.rao)
            .field("ctx", &self.ctx)
            .finish()
    }
}
impl<U: Unit> Balance<U> {
    /// Exact integer amount in base units.
    pub const fn rao(self) -> u64 {
        self.rao
    }

    /// Whether this amount is zero.
    pub const fn is_zero(self) -> bool {
        self.rao == 0
    }

    /// Exact token amount, with nine fractional digits and no float conversion.
    pub fn to_decimal_string(self) -> String {
        decimal_string(self.rao)
    }
}

impl Balance<Tao> {
    /// Zero TAO.
    pub const ZERO: Self = Self::from_rao(0);

    /// Construct an exact TAO amount from integer rao.
    pub const fn from_rao(rao: u64) -> Self {
        Self { rao, ctx: () }
    }

    /// Parse a non-negative decimal TAO amount with at most nine fractional digits.
    ///
    /// Signs, exponents, whitespace and digit separators are not decimal strings.
    pub fn parse(input: &str) -> Result<Self, UnitError> {
        parse_decimal(input).map(Self::from_rao)
    }

    /// Convert a float TAO amount, explicitly accepting its precision loss.
    ///
    /// Rounds to the nearest rao, breaking ties toward the even integer. Rejects
    /// negative, non-finite and out-of-range amounts.
    pub fn from_tao_lossy(amount: f64) -> Result<Self, UnitError> {
        lossy_rao(amount).map(Self::from_rao)
    }

    /// Add two TAO amounts, reporting native overflow.
    pub fn checked_add(self, other: Self) -> Result<Self, UnitError> {
        self.rao
            .checked_add(other.rao)
            .map(Self::from_rao)
            .ok_or(UnitError::Overflow)
    }

    /// Subtract two TAO amounts, reporting native underflow.
    pub fn checked_sub(self, other: Self) -> Result<Self, UnitError> {
        self.rao
            .checked_sub(other.rao)
            .map(Self::from_rao)
            .ok_or(UnitError::Underflow)
    }

    /// Subtract a TAO amount, saturating at zero.
    pub fn saturating_sub(self, other: Self) -> Self {
        Self::from_rao(self.rao.saturating_sub(other.rao))
    }

    /// Sum TAO amounts, reporting overflow; an empty sum is zero.
    pub fn try_sum(amounts: impl IntoIterator<Item = Self>) -> Result<Self, UnitError> {
        amounts.into_iter().try_fold(Self::ZERO, Self::checked_add)
    }

    /// Convert to the chain's TAO wire amount.
    pub const fn to_wire(self) -> TaoBalance {
        TaoBalance(self.rao)
    }
}

impl PartialOrd for Balance<Tao> {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}
impl Ord for Balance<Tao> {
    fn cmp(&self, other: &Self) -> Ordering {
        self.rao.cmp(&other.rao)
    }
}
impl TryFrom<u128> for Balance<Tao> {
    type Error = UnitError;

    fn try_from(rao: u128) -> Result<Self, Self::Error> {
        u64::try_from(rao)
            .map(Self::from_rao)
            .map_err(|_| UnitError::Overflow)
    }
}
impl TryFrom<i128> for Balance<Tao> {
    type Error = UnitError;

    fn try_from(rao: i128) -> Result<Self, Self::Error> {
        checked_signed_rao(rao).map(Self::from_rao)
    }
}

impl Balance<Alpha> {
    /// Construct an alpha amount; netuid zero denotes TAO and is rejected.
    pub fn new(netuid: NetUid, rao: u64) -> Result<Self, UnitError> {
        non_root(netuid)?;
        Ok(Self { rao, ctx: netuid })
    }

    /// Parse an exact decimal amount in a non-root subnet's alpha currency.
    pub fn parse(input: &str, netuid: NetUid) -> Result<Self, UnitError> {
        Self::new(netuid, parse_decimal(input)?)
    }

    /// Convert a float alpha amount with explicit precision loss.
    pub fn from_alpha_lossy(amount: f64, netuid: NetUid) -> Result<Self, UnitError> {
        Self::new(netuid, lossy_rao(amount)?)
    }

    /// Subnet whose alpha currency this amount denotes.
    pub const fn netuid(self) -> NetUid {
        self.ctx
    }

    /// Add amounts in the same subnet's alpha currency.
    ///
    /// Currency mismatch and native integer overflow are explicit errors.
    pub fn checked_add(self, other: Self) -> Result<Self, UnitError> {
        same_unit(self.ctx, other.ctx)?;
        self.rao
            .checked_add(other.rao)
            .map(|rao| Self { rao, ctx: self.ctx })
            .ok_or(UnitError::Overflow)
    }

    /// Subtract amounts in the same subnet's alpha currency.
    ///
    /// Currency mismatch and native integer underflow are explicit errors.
    pub fn checked_sub(self, other: Self) -> Result<Self, UnitError> {
        same_unit(self.ctx, other.ctx)?;
        self.rao
            .checked_sub(other.rao)
            .map(|rao| Self { rao, ctx: self.ctx })
            .ok_or(UnitError::Underflow)
    }

    /// Subtract matching alpha amounts, saturating at zero.
    pub fn saturating_sub(self, other: Self) -> Result<Self, UnitError> {
        same_unit(self.ctx, other.ctx)?;
        Ok(Self {
            rao: self.rao.saturating_sub(other.rao),
            ctx: self.ctx,
        })
    }

    /// Compare amounts, rejecting different subnet currencies.
    pub fn checked_cmp(self, other: Self) -> Result<Ordering, UnitError> {
        same_unit(self.ctx, other.ctx)?;
        Ok(self.rao.cmp(&other.rao))
    }

    /// Sum a subnet's alpha amounts; an empty sum is zero in that currency.
    ///
    /// Every item is unit-checked even after overflow, so a later mismatch
    /// cannot be hidden by an earlier arithmetic failure.
    pub fn try_sum(
        netuid: NetUid,
        amounts: impl IntoIterator<Item = Self>,
    ) -> Result<Self, UnitError> {
        let zero = Self::new(netuid, 0)?;
        let mut total = Ok(zero);
        for amount in amounts {
            same_unit(netuid, amount.ctx)?;
            total = total.and_then(|value| value.checked_add(amount));
        }
        total
    }

    /// Convert to a chain alpha amount after checking the call's subnet context.
    pub fn to_wire_for(self, call_netuid: NetUid) -> Result<AlphaBalance, UnitError> {
        same_unit(call_netuid, self.ctx)?;
        Ok(AlphaBalance(self.rao))
    }
}

/// Chain-registered currency symbol, used only for rendering a balance.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(transparent))]
pub struct Symbol(String);

impl Symbol {
    /// Preserve a chain-provided symbol, including a possibly empty string.
    pub fn new(symbol: impl Into<String>) -> Self {
        Self(symbol.into())
    }

    /// Borrow the symbol text.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}
impl From<String> for Symbol {
    fn from(value: String) -> Self {
        Self(value)
    }
}
impl From<&str> for Symbol {
    fn from(value: &str) -> Self {
        Self(value.to_owned())
    }
}
impl fmt::Display for Symbol {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// A native u64 balance tagged with the netuid whose currency it denotes.
///
/// Symbol text affects display only. Currency identity, arithmetic, and ordering
/// use netuid. JSON retains Python formatting for valid chain amounts; negative
/// and arbitrary-size local balances are excluded by Q25 / D-035.
#[derive(Clone, Debug)]
pub struct DynBalance {
    /// Exact native chain base units.
    pub rao: u64,
    /// Zero for TAO; otherwise the subnet whose alpha this balance denotes.
    pub netuid: NetUid,
    /// Optional chain-registered display symbol.
    pub symbol: Option<Symbol>,
}

impl DynBalance {
    /// Construct an exact native balance with its supplied currency tag.
    ///
    /// Typed conversion and intent resolution validate the netuid range.
    pub const fn from_rao(rao: u64, netuid: NetUid) -> Self {
        Self {
            rao,
            netuid,
            symbol: None,
        }
    }

    /// Check a signed external amount and currency before entering the domain.
    pub fn try_from_rao(rao: i128, netuid: NetUid) -> Result<Self, UnitError> {
        NetUid::new(netuid.get())?;
        checked_signed_rao(rao).map(|rao| Self::from_rao(rao, netuid))
    }

    /// Check an unsigned external amount before narrowing to native chain rao.
    pub fn try_from_unsigned_rao(rao: u128, netuid: NetUid) -> Result<Self, UnitError> {
        NetUid::new(netuid.get())?;
        u64::try_from(rao)
            .map(|rao| Self::from_rao(rao, netuid))
            .map_err(|_| UnitError::Overflow)
    }

    /// Parse a non-negative exact decimal token amount with at most nine places.
    pub fn parse(input: &str, netuid: NetUid) -> Result<Self, UnitError> {
        NetUid::new(netuid.get())?;
        parse_decimal(input).map(|rao| Self::from_rao(rao, netuid))
    }

    /// Construct TAO from exact numeric or decimal-string input.
    ///
    /// Values requiring rounding or exceeding the native range are rejected.
    /// Use [`Balance::from_tao_lossy`] for explicitly lossy float conversion.
    pub fn from_tao(input: impl fmt::Display) -> Result<Self, UnitError> {
        Self::from_amount(input, NetUid::ROOT)
    }

    /// Construct exact alpha, rejecting the root TAO currency.
    pub fn from_alpha(input: impl fmt::Display, netuid: NetUid) -> Result<Self, UnitError> {
        non_root(netuid)?;
        Self::from_amount(input, netuid)
    }

    /// Construct an exact numeric token amount where its unit varies at runtime.
    pub fn from_amount(input: impl fmt::Display, netuid: NetUid) -> Result<Self, UnitError> {
        NetUid::new(netuid.get())?;
        parse_amount_text(&input.to_string(), true).map(|rao| Self::from_rao(rao, netuid))
    }

    /// Attach a display-only chain symbol.
    pub fn with_symbol(mut self, symbol: impl Into<Symbol>) -> Self {
        self.symbol = Some(symbol.into());
        self
    }

    /// Convert to typed TAO, rejecting any alpha currency.
    pub fn tao(&self) -> Result<Balance<Tao>, UnitError> {
        same_unit(NetUid::ROOT, self.netuid)?;
        Ok(Balance::<Tao>::from_rao(self.rao))
    }

    /// Convert to typed alpha, rejecting TAO and invalid subnet identifiers.
    pub fn alpha(&self) -> Result<Balance<Alpha>, UnitError> {
        Balance::<Alpha>::new(self.netuid, self.rao)
    }

    /// Numeric own-currency amount with Python's correctly rounded division.
    pub fn amount(&self) -> f64 {
        crate::pyfmt::rao_to_f64(self.rao)
    }

    /// Numeric TAO amount; rejects an alpha currency.
    pub fn tao_amount(&self) -> Result<f64, UnitError> {
        same_unit(NetUid::ROOT, self.netuid)?;
        Ok(self.amount())
    }

    /// Numeric alpha amount; rejects root TAO and invalid subnet identifiers.
    pub fn alpha_amount(&self) -> Result<f64, UnitError> {
        non_root(self.netuid)?;
        Ok(self.amount())
    }

    /// Whether the exact native amount is zero.
    pub const fn is_zero(&self) -> bool {
        self.rao == 0
    }

    /// Compare against native integer base units.
    ///
    /// ```compile_fail
    /// use rtao_primitives::{DynBalance, NetUid};
    /// let balance = DynBalance::from_rao(1, NetUid::ROOT);
    /// let _ = balance.cmp_rao(0.5);
    /// ```
    ///
    /// ```compile_fail
    /// use rtao_primitives::{DynBalance, NetUid};
    /// let balance = DynBalance::from_rao(1, NetUid::ROOT);
    /// let _ = balance.cmp_rao(true);
    /// ```
    ///
    /// ```compile_fail
    /// use rtao_primitives::{DynBalance, NetUid};
    /// let balance = DynBalance::from_rao(1, NetUid::ROOT);
    /// let _ = balance == 1_u64;
    /// ```
    pub fn cmp_rao(&self, rao: u64) -> Ordering {
        self.rao.cmp(&rao)
    }

    /// Display currency: `τ`, a chain symbol, or `α` plus subscript netuid.
    pub fn unit(&self) -> Cow<'_, str> {
        if self.netuid == NetUid::ROOT {
            return Cow::Borrowed("τ");
        }
        if let Some(symbol) = &self.symbol
            && !symbol.as_str().is_empty()
        {
            return Cow::Borrowed(symbol.as_str());
        }
        Cow::Owned(alpha_unit(self.netuid))
    }

    /// Exact token amount with nine fractional digits.
    pub fn to_decimal_string(&self) -> String {
        decimal_string(self.rao)
    }

    /// Python's `str(Balance)` for the native chain range.
    pub fn to_python_string(&self) -> String {
        crate::pyfmt::balance_str(
            self.rao,
            self.netuid,
            self.symbol.as_ref().map(Symbol::as_str),
        )
    }

    /// Add matching currencies, reporting native integer overflow.
    pub fn checked_add(&self, other: &Self) -> Result<Self, UnitError> {
        same_unit(self.netuid, other.netuid)?;
        self.rao
            .checked_add(other.rao)
            .map(|rao| self.result_with(rao, other))
            .ok_or(UnitError::Overflow)
    }

    /// Subtract matching currencies, reporting native integer underflow.
    pub fn checked_sub(&self, other: &Self) -> Result<Self, UnitError> {
        same_unit(self.netuid, other.netuid)?;
        self.rao
            .checked_sub(other.rao)
            .map(|rao| self.result_with(rao, other))
            .ok_or(UnitError::Underflow)
    }

    /// Add native integer base units, preserving the currency and symbol.
    pub fn checked_add_rao(&self, rao: u64) -> Result<Self, UnitError> {
        self.rao
            .checked_add(rao)
            .map(|rao| Self {
                rao,
                ..self.clone()
            })
            .ok_or(UnitError::Overflow)
    }

    /// Subtract native integer base units, preserving the currency and symbol.
    pub fn checked_sub_rao(&self, rao: u64) -> Result<Self, UnitError> {
        self.rao
            .checked_sub(rao)
            .map(|rao| Self {
                rao,
                ..self.clone()
            })
            .ok_or(UnitError::Underflow)
    }

    /// Explicitly saturating subtraction after checking currency tags.
    pub fn saturating_sub(&self, other: &Self) -> Result<Self, UnitError> {
        same_unit(self.netuid, other.netuid)?;
        Ok(self.result_with(self.rao.saturating_sub(other.rao), other))
    }

    /// Compare balances after checking their currencies.
    pub fn checked_cmp(&self, other: &Self) -> Result<Ordering, UnitError> {
        same_unit(self.netuid, other.netuid)?;
        Ok(self.rao.cmp(&other.rao))
    }

    /// Sum native amounts in one currency; an empty sum is zero.
    ///
    /// All currency tags are checked even after an arithmetic overflow.
    pub fn try_sum(
        netuid: NetUid,
        amounts: impl IntoIterator<Item = Self>,
    ) -> Result<Self, UnitError> {
        NetUid::new(netuid.get())?;
        let mut total = Ok(Self::from_rao(0, netuid));
        for amount in amounts {
            same_unit(netuid, amount.netuid)?;
            total = total.and_then(|value| value.checked_add(&amount));
        }
        total
    }

    fn result_with(&self, rao: u64, other: &Self) -> Self {
        let symbol = self
            .symbol
            .as_ref()
            .filter(|symbol| !symbol.as_str().is_empty())
            .or(other.symbol.as_ref())
            .cloned();
        Self {
            rao,
            netuid: self.netuid,
            symbol,
        }
    }
}

impl PartialEq for DynBalance {
    fn eq(&self, other: &Self) -> bool {
        self.rao == other.rao && self.netuid == other.netuid
    }
}
impl Eq for DynBalance {}
impl Hash for DynBalance {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.rao.hash(state);
        self.netuid.hash(state);
    }
}
impl fmt::Display for DynBalance {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.to_python_string())
    }
}
impl From<Balance<Tao>> for DynBalance {
    fn from(value: Balance<Tao>) -> Self {
        Self::from_rao(value.rao, NetUid::ROOT)
    }
}
impl From<Balance<Alpha>> for DynBalance {
    fn from(value: Balance<Alpha>) -> Self {
        Self::from_rao(value.rao, value.ctx)
    }
}
impl TryFrom<DynBalance> for Balance<Tao> {
    type Error = UnitError;

    fn try_from(value: DynBalance) -> Result<Self, Self::Error> {
        value.tao()
    }
}
impl TryFrom<DynBalance> for Balance<Alpha> {
    type Error = UnitError;

    fn try_from(value: DynBalance) -> Result<Self, Self::Error> {
        value.alpha()
    }
}

/// Exact intent money input before a field supplies its expected currency.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Money {
    /// An exact amount whose unit is supplied by the consuming intent field.
    Untagged {
        /// Exact integer amount in base units.
        rao: u64,
    },
    /// An amount whose currency must match the consuming intent field.
    Tagged(DynBalance),
}

impl Money {
    /// Construct an untagged exact base-unit amount.
    pub const fn from_rao(rao: u64) -> Self {
        Self::Untagged { rao }
    }

    /// Parse an untagged exact decimal token amount.
    pub fn parse(input: &str) -> Result<Self, UnitError> {
        parse_decimal(input).map(Self::from_rao)
    }

    /// Exact base-unit amount, independent of whether its unit is tagged.
    pub fn rao(&self) -> Result<u64, UnitError> {
        match self {
            Self::Untagged { rao } => Ok(*rao),
            Self::Tagged(balance) => Ok(balance.rao),
        }
    }

    /// Exact decimal token amount for Python's intent serialization.
    pub fn to_decimal_string(&self) -> String {
        match self {
            Self::Untagged { rao } => money_decimal_string(*rao),
            Self::Tagged(balance) => money_decimal_string(balance.rao),
        }
    }

    /// Attach the consuming field's netuid or validate an existing currency tag.
    ///
    /// Root alpha operations intentionally pass netuid zero and resolve to TAO,
    /// matching Python's root branches. Runtime call identity and sibling-netuid
    /// selection belong to the intent layer.
    pub fn resolve(&self, expected_netuid: NetUid) -> Result<DynBalance, UnitError> {
        NetUid::new(expected_netuid.get())?;
        match self {
            Self::Untagged { rao } => Ok(DynBalance::from_rao(*rao, expected_netuid)),
            Self::Tagged(balance) => {
                same_unit(expected_netuid, balance.netuid)?;

                Ok(balance.clone())
            }
        }
    }
}
impl From<DynBalance> for Money {
    fn from(value: DynBalance) -> Self {
        Self::Tagged(value)
    }
}
impl From<Balance<Tao>> for Money {
    fn from(value: Balance<Tao>) -> Self {
        Self::Tagged(value.into())
    }
}
impl From<Balance<Alpha>> for Money {
    fn from(value: Balance<Alpha>) -> Self {
        Self::Tagged(value.into())
    }
}

/// Intent amount, including the opt-in request to drain all available funds.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Amount {
    /// A concrete exact amount awaiting field-specific unit resolution.
    Exact(Money),
    /// Resolve the concrete amount from chain state at build time.
    All,
}

impl Amount {
    /// Parse decimal input or Python's case-insensitive, trimmed `"all"` sentinel.
    ///
    /// Whether a particular field supports `"all"` is checked by [`Self::resolve`].
    pub fn parse(input: &str) -> Result<Self, UnitError> {
        if input.trim().eq_ignore_ascii_case("all") {
            Ok(Self::All)
        } else {
            Money::parse(input).map(Self::Exact)
        }
    }

    /// Resolve the expected currency and enforce the field's `"all"` opt-in.
    pub fn resolve(
        &self,
        expected_netuid: NetUid,
        allow_all: bool,
    ) -> Result<ResolvedAmount, UnitError> {
        match self {
            Self::Exact(money) => money.resolve(expected_netuid).map(ResolvedAmount::Exact),
            Self::All if allow_all => Ok(ResolvedAmount::All),
            Self::All => Err(UnitError::AllNotAllowed),
        }
    }
}
impl From<Money> for Amount {
    fn from(value: Money) -> Self {
        Self::Exact(value)
    }
}
impl From<DynBalance> for Amount {
    fn from(value: DynBalance) -> Self {
        Self::Exact(value.into())
    }
}
impl From<Balance<Tao>> for Amount {
    fn from(value: Balance<Tao>) -> Self {
        Self::Exact(value.into())
    }
}
impl From<Balance<Alpha>> for Amount {
    fn from(value: Balance<Alpha>) -> Self {
        Self::Exact(value.into())
    }
}

/// An intent field after its expected unit and sibling netuid are resolved.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum ResolvedAmount {
    /// A concrete currency-tagged amount.
    Exact(DynBalance),
    /// An opt-in amount to resolve from chain state at build time.
    All,
}

/// Upper bound on value leaving a transaction's signer.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Spend {
    /// The operation sends or burns no value.
    None,
    /// An exact bound denominated in TAO.
    Bounded(Balance<Tao>),
    /// The spend cannot be cheaply bounded; a configured cap must reject it.
    Unbounded,
}

fn same_unit(expected: NetUid, actual: NetUid) -> Result<(), UnitError> {
    if expected == actual {
        Ok(())
    } else {
        Err(UnitError::UnitMismatch { expected, actual })
    }
}

fn non_root(netuid: NetUid) -> Result<(), UnitError> {
    NetUid::new(netuid.get())?;
    alpha_netuid(netuid)
}

fn alpha_netuid(netuid: NetUid) -> Result<(), UnitError> {
    if netuid == NetUid::ROOT {
        Err(UnitError::UnitMismatch {
            expected: NetUid(1),
            actual: netuid,
        })
    } else {
        Ok(())
    }
}

fn checked_signed_rao(rao: i128) -> Result<u64, UnitError> {
    if rao < 0 {
        return Err(UnitError::Negative);
    }
    u64::try_from(rao).map_err(|_| UnitError::Overflow)
}

fn decimal_string(rao: u64) -> String {
    // The divisor is the nonzero constant 10^9.
    let integer = rao.checked_div(RAO_PER_TAO).unwrap_or_default();
    let fraction = rao.checked_rem(RAO_PER_TAO).unwrap_or_default();
    format!("{integer}.{fraction:09}")
}

// Python Intent._encode uses format(balance.decimal, "f"). Integer division
// by 10^9 in Decimal retains only significant fractional digits; presentation
// strings for balances still use their separate fixed-nine-digit formatter.
fn money_decimal_string(rao: u64) -> String {
    let text = decimal_string(rao);
    text.trim_end_matches('0').trim_end_matches('.').to_owned()
}

fn parse_decimal(input: &str) -> Result<u64, UnitError> {
    parse_amount_text(input, false)
}

fn parse_amount_text(input: &str, allow_exponent: bool) -> Result<u64, UnitError> {
    let invalid = || UnitError::InvalidDecimal(input.to_owned());
    let (negative, unsigned) = match input.strip_prefix('-') {
        Some(rest) if allow_exponent => (true, rest),
        Some(_) => return Err(UnitError::Negative),
        None => (false, input),
    };
    let (mantissa, exponent) = if allow_exponent {
        match unsigned.find(['e', 'E']) {
            Some(index) => {
                let (mantissa, exponent) = unsigned.split_at(index);
                let exponent = exponent.get(1..).ok_or_else(invalid)?;
                if exponent.is_empty() {
                    return Err(invalid());
                }
                let value = exponent.parse::<i64>().map_err(|_| {
                    if exponent.starts_with('-') {
                        UnitError::Precision
                    } else {
                        UnitError::Overflow
                    }
                })?;
                (mantissa, value)
            }
            None => (unsigned, 0),
        }
    } else {
        (unsigned, 0)
    };
    let (integer, fraction) = match mantissa.split_once('.') {
        Some((integer, fraction)) if !fraction.is_empty() => (integer, fraction),
        Some(_) => return Err(invalid()),
        None => (mantissa, ""),
    };
    if integer.is_empty()
        || !integer.bytes().all(|byte| byte.is_ascii_digit())
        || !fraction.bytes().all(|byte| byte.is_ascii_digit())
    {
        return Err(invalid());
    }
    let nonzero = integer
        .bytes()
        .chain(fraction.bytes())
        .any(|byte| byte != b'0');
    if negative && nonzero {
        return Err(UnitError::Negative);
    }
    let fractional_digits = i64::try_from(fraction.len()).map_err(|_| UnitError::Precision)?;
    let effective_fraction = fractional_digits
        .checked_sub(exponent)
        .ok_or(UnitError::Precision)?;
    if effective_fraction > 9 {
        return Err(UnitError::Precision);
    }
    let mut rao = 0_u64;
    for byte in integer.bytes().chain(fraction.bytes()) {
        let digit = u64::from(byte.checked_sub(b'0').ok_or_else(invalid)?);
        rao = rao
            .checked_mul(10)
            .and_then(|value| value.checked_add(digit))
            .ok_or(UnitError::Overflow)?;
    }
    if rao == 0 {
        return Ok(0);
    }
    let scale = 9_i64
        .checked_sub(effective_fraction)
        .ok_or(UnitError::Overflow)?;
    let scale = u32::try_from(scale).map_err(|_| UnitError::Overflow)?;
    let multiplier = 10_u64.checked_pow(scale).ok_or(UnitError::Overflow)?;
    rao.checked_mul(multiplier).ok_or(UnitError::Overflow)
}

fn lossy_rao(amount: f64) -> Result<u64, UnitError> {
    if !amount.is_finite() {
        return Err(UnitError::NonFinite);
    }
    if amount < 0.0 {
        return Err(UnitError::Negative);
    }
    let rao = (amount * (RAO_PER_TAO as f64)).round_ties_even();
    // u64::MAX rounds to 2^64 as f64; using `< 2^64` avoids a saturating cast.
    if !rao.is_finite() || rao >= 18_446_744_073_709_551_616.0 {
        return Err(UnitError::Overflow);
    }
    Ok(rao as u64)
}

fn alpha_unit(netuid: NetUid) -> String {
    let mut unit = String::from("α");
    for digit in netuid.get().to_string().chars() {
        unit.push(match digit {
            '0' => '₀',
            '1' => '₁',
            '2' => '₂',
            '3' => '₃',
            '4' => '₄',
            '5' => '₅',
            '6' => '₆',
            '7' => '₇',
            '8' => '₈',
            '9' => '₉',
            _ => continue,
        });
    }
    unit
}

#[cfg(feature = "serde")]
mod serde_impl {
    use super::*;
    use serde::{Deserialize, Deserializer, Serialize, Serializer, de};

    #[cfg_attr(docsrs, doc(cfg(feature = "serde")))]
    impl Serialize for DynBalance {
        fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
            let text = self.to_python_string();
            serializer.serialize_str(&text)
        }
    }

    #[cfg_attr(docsrs, doc(cfg(feature = "serde")))]
    impl<U: Unit> Serialize for Balance<U> {
        fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
            serializer.serialize_str(&decimal_string(self.rao))
        }
    }

    #[cfg_attr(docsrs, doc(cfg(feature = "serde")))]
    impl Serialize for Money {
        fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
            self.rao().map_err(serde::ser::Error::custom)?;
            serializer.serialize_str(&self.to_decimal_string())
        }
    }

    #[cfg_attr(docsrs, doc(cfg(feature = "serde")))]
    impl<'de> Deserialize<'de> for Money {
        fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
            // Value-first is required with serde_json's arbitrary_precision;
            // Number::as_str keeps decimals and integers beyond f64 exact.
            let value = serde_json::Value::deserialize(deserializer)?;
            match value {
                serde_json::Value::Number(number) => parse_amount_text(number.as_str(), true)
                    .map(Self::from_rao)
                    .map_err(de::Error::custom),
                serde_json::Value::String(text) => Self::parse(&text).map_err(de::Error::custom),
                _ => Err(de::Error::custom(
                    "money must be a non-negative number or exact decimal string",
                )),
            }
        }
    }

    #[cfg_attr(docsrs, doc(cfg(feature = "serde")))]
    impl Serialize for Amount {
        fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
            match self {
                Self::Exact(money) => money.serialize(serializer),
                Self::All => serializer.serialize_str("all"),
            }
        }
    }

    #[cfg_attr(docsrs, doc(cfg(feature = "serde")))]
    impl<'de> Deserialize<'de> for Amount {
        fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
            let value = serde_json::Value::deserialize(deserializer)?;
            match value {
                serde_json::Value::String(text) => Self::parse(&text).map_err(de::Error::custom),
                serde_json::Value::Number(number) => parse_amount_text(number.as_str(), true)
                    .map(|rao| Self::Exact(Money::from_rao(rao)))
                    .map_err(de::Error::custom),
                _ => Err(de::Error::custom(
                    "amount must be a number, exact decimal string, or 'all'",
                )),
            }
        }
    }

    #[cfg_attr(docsrs, doc(cfg(feature = "serde")))]
    impl Serialize for ResolvedAmount {
        fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
            match self {
                Self::Exact(balance) => {
                    serializer.serialize_str(&money_decimal_string(balance.rao))
                }
                Self::All => serializer.serialize_str("all"),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;
    use std::collections::hash_map::DefaultHasher;

    // pyt: unit/test_balance_properties.py::RAO
    // pyt: unit/test_balance_properties.py::SIGNED_RAO
    // Q25 / D-035: adapted to the native 0..=u64::MAX domain. Python's signed
    // and arbitrary-size arithmetic, negation, and permissive rounding inputs
    // are explicitly excluded; these are not full Python strategy ports.
    fn native_rao() -> impl Strategy<Value = u64> {
        any::<u64>()
    }
    // pyt: unit/test_balance_properties.py::NETUID (unchanged 0..=4095)
    fn netuid() -> impl Strategy<Value = NetUid> {
        (0_u16..=4095).prop_map(NetUid)
    }
    // pyt: unit/test_balance_properties.py::SUBNET (unchanged 1..=4095)
    fn subnet() -> impl Strategy<Value = NetUid> {
        (1_u16..=4095).prop_map(NetUid)
    }

    fn hash(value: &impl Hash) -> u64 {
        let mut state = DefaultHasher::new();
        value.hash(&mut state);
        state.finish()
    }

    proptest! {
        // pyt: unit/test_balance_properties.py::test_rao_roundtrip_is_exact
        #[test]
        fn native_rao_roundtrip_is_exact(rao in native_rao(), netuid in netuid()) {
            let balance = DynBalance::from_rao(rao, netuid);
            prop_assert_eq!(balance.rao, rao);
            prop_assert_eq!(DynBalance::try_from_rao(i128::from(rao), netuid)?, balance);
        }

        // pyt: unit/test_balance_properties.py::test_decimal_serialization_roundtrip_is_exact
        // pyt: unit/test_balance_properties.py::test_from_tao_decimal_string_is_exact_where_float_is_not
        #[test]
        fn native_decimal_serialization_roundtrip_is_exact(rao in native_rao(), netuid in netuid()) {
            let balance = DynBalance::from_rao(rao, netuid);
            let encoded = balance.to_decimal_string();
            prop_assert_eq!(DynBalance::parse(&encoded, netuid)?, balance);
            prop_assert_eq!(DynBalance::from_amount(&encoded, netuid)?.rao, rao);
            prop_assert_eq!(Balance::<Tao>::parse(&encoded)?.rao(), rao);
        }

        // pyt: unit/test_balance_properties.py::test_addition_is_integer_arithmetic_on_rao
        #[test]
        fn native_arithmetic_matches_checked_u64(a in native_rao(), b in native_rao(), netuid in subnet()) {
            let expected_add = a.checked_add(b).ok_or(UnitError::Overflow);
            let expected_sub = a.checked_sub(b).ok_or(UnitError::Underflow);
            let tao_a = Balance::<Tao>::from_rao(a);
            let tao_b = Balance::<Tao>::from_rao(b);
            prop_assert_eq!(tao_a.checked_add(tao_b).map(Balance::rao), expected_add.clone());
            prop_assert_eq!(tao_a.checked_sub(tao_b).map(Balance::rao), expected_sub.clone());
            let alpha_a = Balance::<Alpha>::new(netuid, a)?;
            let alpha_b = Balance::<Alpha>::new(netuid, b)?;
            prop_assert_eq!(alpha_a.checked_add(alpha_b).map(Balance::rao), expected_add.clone());
            prop_assert_eq!(alpha_a.checked_sub(alpha_b).map(Balance::rao), expected_sub.clone());
            let dynamic_a = DynBalance::from_rao(a, netuid).with_symbol("β");
            let dynamic_b = DynBalance::from_rao(b, netuid);
            prop_assert_eq!(dynamic_a.checked_add(&dynamic_b).map(|value| value.rao), expected_add.clone());
            prop_assert_eq!(dynamic_a.checked_sub(&dynamic_b).map(|value| value.rao), expected_sub.clone());
            prop_assert_eq!(dynamic_a.checked_add_rao(b).map(|value| value.rao), expected_add);
            prop_assert_eq!(dynamic_a.checked_sub_rao(b).map(|value| value.rao), expected_sub);
            if let Ok(total) = dynamic_a.checked_add(&dynamic_b) {
                prop_assert_eq!(total.checked_sub(&dynamic_b)?, dynamic_a);
            }
        }

        // pyt: unit/test_balance_properties.py::test_cross_unit_arithmetic_always_raises
        #[test]
        fn native_cross_unit_arithmetic_always_raises(rao in native_rao(), first in netuid(), second in netuid()) {
            if first != second {
                let x = DynBalance::from_rao(rao, first);
                let y = DynBalance::from_rao(rao, second);
                let mismatch = UnitError::UnitMismatch { expected: first, actual: second };
                prop_assert_eq!(x.checked_add(&y), Err(mismatch.clone()));
                prop_assert_eq!(x.checked_sub(&y), Err(mismatch.clone()));
                prop_assert_eq!(x.checked_cmp(&y), Err(mismatch));
                prop_assert_ne!(x, y);
            }
        }

        // pyt: unit/test_balance_properties.py::test_ordering_matches_rao
        #[test]
        fn native_ordering_matches_rao(a in native_rao(), b in native_rao(), netuid in netuid()) {
            let x = DynBalance::from_rao(a, netuid);
            let y = DynBalance::from_rao(b, netuid);
            prop_assert_eq!(x.checked_cmp(&y)?, a.cmp(&b));
            prop_assert_eq!(x.cmp_rao(b), a.cmp(&b));
            prop_assert_eq!(x == y, a == b);
        }

        // pyt: unit/test_balance_properties.py::test_eq_hash_consistency
        // Negation is excluded by Q25 / D-035; symbol remains display-only.
        #[test]
        fn native_equality_hash_ignore_display_symbols(rao in native_rao(), netuid in netuid()) {
            let x = DynBalance::from_rao(rao, netuid);
            let y = x.clone().with_symbol("display only");
            prop_assert_eq!(hash(&x), hash(&y));
            prop_assert_eq!(x, y);
        }

        // pyt: unit/test_balance_properties.py::test_unit_accessors_are_segregated
        #[test]
        fn native_unit_accessors_are_segregated(rao in native_rao(), netuid in subnet()) {
            let alpha = DynBalance::from_rao(rao, netuid);
            let tao = DynBalance::from_rao(rao, NetUid::ROOT);
            prop_assert!(alpha.tao().is_err());
            prop_assert!(tao.alpha().is_err());
            prop_assert!(alpha.tao_amount().is_err());
            prop_assert!(tao.alpha_amount().is_err());
            prop_assert_eq!(alpha.alpha_amount()?, tao.tao_amount()?);
            prop_assert_eq!(alpha.alpha()?.rao(), tao.tao()?.rao());
        }

        // pyt: unit/test_balance_properties.py::test_float_comparison_is_rejected
        // Float and bool rejection is checked by compile-fail doctests.
        #[test]
        fn native_half_tao_ordering_alternative(rao in native_rao()) {
            let balance = DynBalance::from_rao(rao, NetUid::ROOT);
            let half = DynBalance::parse("0.5", NetUid::ROOT)?;
            prop_assert_eq!(balance.checked_cmp(&half)? == Ordering::Greater, rao > 500_000_000);
        }

        // pyt: unit/test_balance_properties.py::test_from_alpha_rejects_tao_netuid
        #[test]
        fn native_from_alpha_rejects_tao_netuid(rao in native_rao(), netuid in subnet()) {
            let exact = decimal_string(rao);
            prop_assert_eq!(DynBalance::from_alpha(&exact, netuid)?.rao, rao);
            prop_assert!(DynBalance::from_alpha(&exact, NetUid::ROOT).is_err());
        }
    }

    #[test]
    fn native_balance_rejects_negative_and_oversized_input() -> Result<(), UnitError> {
        for rao in [i128::MIN, -(1_i128 << 80), -1] {
            assert_eq!(
                DynBalance::try_from_rao(rao, NetUid::ROOT),
                Err(UnitError::Negative)
            );
            assert_eq!(Balance::<Tao>::try_from(rao), Err(UnitError::Negative));
        }
        for rao in [
            i128::from(u64::MAX)
                .checked_add(1)
                .ok_or(UnitError::Overflow)?,
            1_i128 << 80,
            i128::MAX,
        ] {
            assert_eq!(
                DynBalance::try_from_rao(rao, NetUid::ROOT),
                Err(UnitError::Overflow)
            );
            assert_eq!(Balance::<Tao>::try_from(rao), Err(UnitError::Overflow));
        }
        for rao in [
            u128::from(u64::MAX)
                .checked_add(1)
                .ok_or(UnitError::Overflow)?,
            1_u128 << 80,
            u128::MAX,
        ] {
            assert_eq!(
                DynBalance::try_from_unsigned_rao(rao, NetUid::ROOT),
                Err(UnitError::Overflow)
            );
            assert_eq!(Balance::<Tao>::try_from(rao), Err(UnitError::Overflow));
        }
        assert_eq!(
            DynBalance::try_from_rao(i128::from(u64::MAX), NetUid::ROOT)?.rao,
            u64::MAX
        );
        for input in ["-1", "-0.000000001", "-1208925819614629.174706176"] {
            assert_eq!(
                DynBalance::parse(input, NetUid::ROOT),
                Err(UnitError::Negative)
            );
            assert_eq!(Money::parse(input), Err(UnitError::Negative));
            assert!(DynBalance::from_tao(input).is_err());
        }
        for input in [
            "18446744073.709551616",
            "1208925819614629.174706176",
            "12345678901234567890123456789",
        ] {
            assert_eq!(
                DynBalance::parse(input, NetUid::ROOT),
                Err(UnitError::Overflow)
            );
            assert_eq!(Money::parse(input), Err(UnitError::Overflow));
        }
        Ok(())
    }

    #[test]
    fn native_balance_arithmetic_reports_overflow_and_underflow() -> Result<(), UnitError> {
        let maximum = DynBalance::from_rao(u64::MAX, NetUid::ROOT);
        let zero = DynBalance::from_rao(0, NetUid::ROOT);
        let one = DynBalance::from_rao(1, NetUid::ROOT);
        assert_eq!(maximum.checked_add(&one), Err(UnitError::Overflow));
        assert_eq!(zero.checked_sub(&one), Err(UnitError::Underflow));
        assert_eq!(maximum.checked_add_rao(1), Err(UnitError::Overflow));
        assert_eq!(zero.checked_sub_rao(1), Err(UnitError::Underflow));
        assert_eq!(maximum.checked_add(&zero)?, maximum);
        assert_eq!(maximum.checked_sub(&maximum)?, zero);
        assert_eq!(
            Balance::<Tao>::from_rao(u64::MAX).checked_add(Balance::<Tao>::from_rao(1)),
            Err(UnitError::Overflow)
        );
        assert_eq!(
            Balance::<Tao>::ZERO.checked_sub(Balance::<Tao>::from_rao(1)),
            Err(UnitError::Underflow)
        );
        let subnet = NetUid(42);
        assert_eq!(
            Balance::<Alpha>::new(subnet, u64::MAX)?.checked_add(Balance::<Alpha>::new(subnet, 1)?),
            Err(UnitError::Overflow)
        );
        assert_eq!(
            Balance::<Alpha>::new(subnet, 0)?.checked_sub(Balance::<Alpha>::new(subnet, 1)?),
            Err(UnitError::Underflow)
        );
        assert_eq!(
            DynBalance::try_sum(
                subnet,
                [
                    DynBalance::from_rao(u64::MAX, subnet),
                    DynBalance::from_rao(1, subnet)
                ]
            ),
            Err(UnitError::Overflow)
        );
        assert_eq!(
            Balance::<Tao>::try_sum([
                Balance::<Tao>::from_rao(u64::MAX),
                Balance::<Tao>::from_rao(1)
            ]),
            Err(UnitError::Overflow)
        );
        assert_eq!(
            Balance::<Alpha>::try_sum(
                subnet,
                [
                    Balance::<Alpha>::new(subnet, u64::MAX)?,
                    Balance::<Alpha>::new(subnet, 1)?
                ]
            ),
            Err(UnitError::Overflow)
        );
        assert_eq!(
            DynBalance::try_sum(subnet, [])?,
            DynBalance::from_rao(0, subnet)
        );
        assert_eq!(Balance::<Tao>::try_sum([])?, Balance::<Tao>::ZERO);
        Ok(())
    }

    #[test]
    fn native_balance_currency_checks_precede_arithmetic_errors() -> Result<(), UnitError> {
        let subnet = NetUid(42);
        let other = NetUid(43);
        let mismatch = UnitError::UnitMismatch {
            expected: subnet,
            actual: other,
        };
        let maximum = DynBalance::from_rao(u64::MAX, subnet);
        let wrong = DynBalance::from_rao(1, other);
        assert_eq!(maximum.checked_add(&wrong), Err(mismatch.clone()));
        assert_eq!(
            DynBalance::from_rao(0, subnet).checked_sub(&wrong),
            Err(mismatch.clone())
        );
        assert_eq!(
            DynBalance::try_sum(subnet, [maximum, DynBalance::from_rao(1, subnet), wrong]),
            Err(mismatch.clone())
        );
        assert_eq!(
            Balance::<Alpha>::try_sum(
                subnet,
                [
                    Balance::<Alpha>::new(subnet, u64::MAX)?,
                    Balance::<Alpha>::new(subnet, 1)?,
                    Balance::<Alpha>::new(other, 0)?
                ]
            ),
            Err(mismatch)
        );
        Ok(())
    }

    #[test]
    fn native_balance_subnet_conversion_rejects_invalid_ids() {
        let invalid = NetUid(4096);
        assert!(DynBalance::from_rao(1, invalid).alpha().is_err());
        assert!(DynBalance::parse("1", invalid).is_err());
        assert!(DynBalance::try_from_rao(1, invalid).is_err());
        assert!(Money::from_rao(1).resolve(invalid).is_err());
        assert!(Balance::<Alpha>::new(invalid, 1).is_err());
    }

    #[test]
    fn native_decimal_boundaries_are_exact() -> Result<(), UnitError> {
        for (input, rao) in [
            ("0", 0),
            ("0.000000001", 1),
            ("1.5", 1_500_000_000),
            ("9007199.254740993", 9_007_199_254_740_993),
            ("18446744073.709551615", u64::MAX),
        ] {
            assert_eq!(DynBalance::parse(input, NetUid::ROOT)?.rao, rao);
            assert_eq!(Balance::<Tao>::parse(input)?.rao(), rao);
            assert_eq!(Money::parse(input)?.rao()?, rao);
        }
        for input in ["0.0000000005", "0.0000000015", "1.0000000000"] {
            assert_eq!(DynBalance::from_tao(input), Err(UnitError::Precision));
        }
        for input in [
            "", " 1", "1 ", "+1", ".1", "1.", "1e0", "1,000", "١", "NaN", "Infinity", "1.2.3",
        ] {
            assert!(DynBalance::parse(input, NetUid::ROOT).is_err(), "{input}");
        }
        assert_eq!(DynBalance::from_tao("1e-9")?.rao, 1);
        assert_eq!(DynBalance::from_tao(1.5_f64)?.rao, 1_500_000_000);
        assert_eq!(DynBalance::from_tao("1e1000000"), Err(UnitError::Overflow));
        assert!(DynBalance::from_tao(true).is_err());
        Ok(())
    }

    #[test]
    fn native_balance_float_input_is_explicitly_lossy() -> Result<(), UnitError> {
        assert_eq!(Balance::<Tao>::from_tao_lossy(1.5)?.rao(), 1_500_000_000);
        assert_eq!(
            Balance::<Tao>::from_tao_lossy(-1.0),
            Err(UnitError::Negative)
        );
        for value in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
            assert_eq!(
                Balance::<Tao>::from_tao_lossy(value),
                Err(UnitError::NonFinite)
            );
        }
        assert_eq!(
            Balance::<Tao>::from_tao_lossy(18_446_744_073.709_553),
            Err(UnitError::Overflow)
        );
        Ok(())
    }

    #[test]
    fn native_balance_wire_and_intent_currency_resolution() -> Result<(), UnitError> {
        let alpha = Balance::<Alpha>::new(NetUid(42), 7)?;
        assert_eq!(alpha.to_wire_for(NetUid(42))?, AlphaBalance(7));
        assert!(alpha.to_wire_for(NetUid(43)).is_err());
        assert_eq!(Balance::<Tao>::from_rao(7).to_wire(), TaoBalance(7));
        let money = Money::parse("1.5")?;
        assert_eq!(money.resolve(NetUid::ROOT)?.tao()?.rao(), 1_500_000_000);
        assert_eq!(money.resolve(NetUid(42))?.alpha()?.rao(), 1_500_000_000);
        let tagged = Money::from(DynBalance::from_rao(7, NetUid(42)).with_symbol("β"));
        assert!(tagged.resolve(NetUid::ROOT).is_err());
        assert_eq!(tagged.resolve(NetUid(42))?.symbol, Some(Symbol::from("β")));
        assert_eq!(
            Amount::All.resolve(NetUid::ROOT, true)?,
            ResolvedAmount::All
        );
        assert_eq!(
            Amount::All.resolve(NetUid::ROOT, false),
            Err(UnitError::AllNotAllowed)
        );
        assert!(
            Amount::from(Balance::<Tao>::from_rao(1))
                .resolve(NetUid(1), false)
                .is_err()
        );
        Ok(())
    }

    #[test]
    fn native_balance_symbols_do_not_change_currency_identity() -> Result<(), UnitError> {
        let left = DynBalance::from_rao(1, NetUid(42)).with_symbol("");
        let right = DynBalance::from_rao(2, NetUid(42)).with_symbol("β");
        assert_eq!(left.checked_add(&right)?.symbol, Some(Symbol::from("β")));
        assert_eq!(
            DynBalance::from_rao(0, NetUid::ROOT)
                .with_symbol("ignored")
                .unit(),
            "τ"
        );
        assert_eq!(
            DynBalance::from_rao(0, NetUid(42)).with_symbol("").unit(),
            "α₄₂"
        );
        assert_eq!(left, left.clone().with_symbol("different"));
        Ok(())
    }

    #[cfg(feature = "serde")]
    #[test]
    fn native_balance_keeps_valid_chain_json() -> Result<(), Box<dyn std::error::Error>> {
        // Pinned intents/base.py::_encode and Balance.decimal define amount
        // strings independently of fixed-nine-place human balance displays.
        for (rao, expected) in [
            (0, "0"),
            (1, "0.000000001"),
            (10, "0.00000001"),
            (100_000_000, "0.1"),
            (1_000_000_000, "1"),
            (1_250_000_000, "1.25"),
            (u64::MAX, "18446744073.709551615"),
        ] {
            for money in [
                Money::from_rao(rao),
                Money::Tagged(DynBalance::from_rao(rao, NetUid(42))),
            ] {
                assert_eq!(serde_json::to_value(&money)?, expected);
                assert_eq!(serde_json::to_value(Amount::Exact(money))?, expected);
            }
            assert_eq!(
                serde_json::to_value(ResolvedAmount::Exact(DynBalance::from_rao(rao, NetUid(42))))?,
                expected
            );
        }
        let money = Money::parse("18446744073.709551615")?;
        let encoded = serde_json::to_string(&money)?;
        assert_eq!(encoded, "\"18446744073.709551615\"");
        assert_eq!(crate::json::from_str::<Money>(&encoded)?, money);
        let balance = DynBalance::from_rao(u64::MAX, NetUid(42)).with_symbol("β");
        assert_eq!(
            serde_json::to_value(&balance)?,
            serde_json::Value::String(balance.to_python_string())
        );
        assert_eq!(
            serde_json::to_value(ResolvedAmount::Exact(balance))?,
            serde_json::Value::String("18446744073.709551615".into())
        );
        assert_eq!(serde_json::to_string(&Amount::All)?, "\"all\"");
        for input in [
            "-1",
            "18446744073.709551616",
            "1208925819614629.174706176",
            "0.0000000001",
            "true",
            "null",
        ] {
            assert!(crate::json::from_str::<Money>(input).is_err(), "{input}");
        }
        for (input, rao) in [
            ("9007199.254740993", 9_007_199_254_740_993),
            ("18446744073.709551615", u64::MAX),
            ("1e-5", 10_000),
            ("-0.0", 0),
        ] {
            assert_eq!(crate::json::from_str::<Money>(input)?.rao()?, rao);
        }
        Ok(())
    }
}
