// Derived from RaoFoundation/subtensor sdk/python/bittensor/intents/gamma.py @ b6a81d144078eac18bd25c3af7cb31579bb670f3 (Apache-2.0); modified by rtao.
//! Gamma intents: minting, deposits, spenders and daily spend caps, and spending.
//!
//! Gamma is not a balance unit: amounts are exact decimal strings of gamma,
//! [`rtao_primitives::gamma::GAMMA_UNITS`] chain units per gamma.

use rtao_client::{View, ViewExt};
use rtao_core::DynValue;
use rtao_core::codec::Value as CoreValue;
use rtao_primitives::{
    AccountId, AlphaBalance, Amount, DynBalance, Money, NetUid, Ratio, Spend, Ss58Address,
    TaoBalance, gamma::gamma_from_units, pyfmt,
};
use rtao_runtime::{
    calls::subtensor_module as calls, runtime_apis::stake_info_runtime_api as stake_api,
    storage::subtensor_module as st,
};
use rtao_tx::{
    BuildCx, Built, ChainError, FactsTitle, Intent, IntentError, IntentPreflight, PreflightCx,
};
use serde_json::{Map, Value, json};

pub use crate::{
    CloseGammaDeposit, MintGamma, MintGammaWithStake, MoveGamma, SetGammaDepositDailySpendCap,
    SetGammaDepositSpender, SpendGamma,
};

/// Most entries in one `spend_gamma` call (`MAX_GAMMA_SPENDS`).
pub const MAX_GAMMA_SPENDS: usize = 512;

/// The spender a new deposit names when none is given.
pub const DEFAULT_SPENDER: &str =
    "the subnet owner's service spender, or the owner (a leased subnet's beneficiary)";

const U64_MAX: u64 = u64::MAX;

fn validation(message: impl Into<String>) -> IntentError {
    IntentError::validation(message)
}

fn units(value: u128) -> DynValue {
    DynValue::new(CoreValue::Uint(value))
}

// py: sdk/python/bittensor/intents/staking.py:101-111 (can_disable=False)
fn check_rate_tolerance(tolerance: Ratio) -> Result<(), IntentError> {
    if !(0.0..1.0).contains(&tolerance.get()) {
        return Err(validation(format!(
            "rate_tolerance must be a fraction in [0, 1), got {} (0.05 = 5%).",
            pyfmt::float_repr(tolerance.get())
        )));
    }
    Ok(())
}

// py: sdk/python/bittensor/intents/_money.py:89-110
/// A gamma amount in chain units; `None` for `all` when it is allowed.
fn gamma_units(amount: &Amount, allow_all: bool) -> Result<Option<u64>, IntentError> {
    match amount {
        Amount::All if allow_all => Ok(None),
        Amount::All => Err(validation("gamma amount does not accept 'all'.")),
        Amount::Exact(Money::Untagged { rao }) => Ok(Some(*rao)),
        Amount::Exact(Money::Tagged(balance)) => Err(validation(format!(
            "gamma amounts are plain numbers, not a Balance: {balance}."
        ))),
    }
}

fn gamma_string(amount: &Amount) -> String {
    match amount {
        Amount::All => "all".into(),
        Amount::Exact(money) => money.to_decimal_string(),
    }
}

// py: sdk/python/bittensor/intents/gamma.py:131-132
/// `"1,234.5 gamma"`: Python's `f"{Decimal(text):,f}"`.
fn gamma_text(units: u128) -> String {
    let text = gamma_from_units(units);
    let (whole, fraction) = text.split_once('.').unwrap_or((&text, ""));
    let grouped = whole
        .parse::<u128>()
        .map_or_else(|_| whole.to_owned(), pyfmt::fmt_grouped);
    if fraction.is_empty() {
        format!("{grouped} gamma")
    } else {
        format!("{grouped}.{fraction} gamma")
    }
}

fn cap_text(cap: Option<u64>) -> String {
    cap.map_or_else(
        || "no daily spend cap".into(),
        |cap| format!("a daily spend cap of {}", gamma_text(u128::from(cap))),
    )
}

// py: sdk/python/bittensor/intents/gamma.py:69-70
fn min_out(gamma: u128, tolerance: Ratio) -> u128 {
    if tolerance.get() == 0.0 {
        // Python's integer zero tolerance keeps the quote exact (D-132).
        return gamma;
    }
    // Python's int(gamma * (1 - tolerance)): a binary64 product, truncated.
    (gamma as f64 * (1.0 - tolerance.get())) as u128
}

/// One gamma deposit's stored record.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Deposit {
    coldkey: AccountId,
    spender: AccountId,
    balance: u64,
    cap: Option<u64>,
}

fn surface(detail: &str) -> IntentError {
    validation(format!(
        "SubtensorModule.GammaDeposits record changed shape: {detail}"
    ))
}

fn json_account(value: &Value, name: &str) -> Result<AccountId, IntentError> {
    value
        .get(name)
        .and_then(Value::as_str)
        .and_then(|text| text.parse().ok())
        .ok_or_else(|| surface(&format!("{name} is not an account")))
}

fn json_u64(value: &Value) -> Option<u64> {
    value.as_u64().or_else(|| {
        value
            .as_array()
            .filter(|items| items.len() == 1)
            .and_then(|items| items[0].as_u64())
    })
}

impl Deposit {
    fn decode(value: &DynValue) -> Result<Self, IntentError> {
        let record = value.to_json();
        let cap = match record.get("daily_spend_cap") {
            None | Some(Value::Null) => None,
            Some(cap) => Some(
                cap.get("cap")
                    .and_then(json_u64)
                    .ok_or_else(|| surface("daily_spend_cap.cap is not a u64"))?,
            ),
        };
        Ok(Self {
            coldkey: json_account(&record, "coldkey")?,
            spender: json_account(&record, "spender")?,
            balance: record
                .get("balance")
                .and_then(json_u64)
                .ok_or_else(|| surface("balance is not a u64"))?,
            cap,
        })
    }
}

async fn deposit<V: View + ?Sized>(
    view: &V,
    netuid: NetUid,
    hotkey: AccountId,
) -> Result<Option<Deposit>, IntentError> {
    view.storage(&st::GAMMA_DEPOSITS, (netuid, hotkey))
        .await?
        .as_ref()
        .map(Deposit::decode)
        .transpose()
}

// py: sdk/python/bittensor/intents/gamma.py:96-101
async fn service_spender<V: View + ?Sized>(
    view: &V,
    netuid: NetUid,
    hotkey: &Ss58Address,
) -> Result<AccountId, IntentError> {
    let Some(service) = view
        .storage(&st::SERVICES, (netuid, hotkey.account()))
        .await?
    else {
        return Err(validation(format!(
            "{hotkey} publishes no service on netuid {netuid}"
        )));
    };
    json_account(&service.to_json(), "spender")
}

// py: sdk/python/bittensor/intents/gamma.py:104-108
async fn named_spender<V: View + ?Sized>(
    view: &V,
    netuid: NetUid,
    spender: Option<&Ss58Address>,
    service: Option<&Ss58Address>,
) -> Result<Option<AccountId>, IntentError> {
    match service {
        None => Ok(spender.map(Ss58Address::account)),
        Some(service) => service_spender(view, netuid, service).await.map(Some),
    }
}

// py: sdk/python/bittensor/intents/gamma.py:111-113
fn check_spender_or_service(
    op: &str,
    spender: Option<&Ss58Address>,
    service: Option<&Ss58Address>,
) -> Result<(), IntentError> {
    if spender.is_some() && service.is_some() {
        return Err(validation(format!(
            "{op}: name a spender or a service, not both"
        )));
    }
    Ok(())
}

// py: sdk/python/bittensor/intents/gamma.py:97-110 (v489)
/// What stops `signer` opening a new deposit at `hotkey`, as the runtime's
/// `Owner::try_get` decides it (runtime 489): an unassociated hotkey (no
/// `Owner` entry) may be used, a hotkey with a stored owner only by that
/// coldkey, and a stored zero account is still an owner. Owners compare as
/// account IDs, so the same coldkey written with another SS58 prefix passes.
async fn new_deposit_blocks<V: View + ?Sized>(
    view: &V,
    hotkey: &Ss58Address,
    signer: AccountId,
) -> Result<Vec<String>, IntentError> {
    let snapshot = view.pin().await?;
    let key = [CoreValue::Bytes(hotkey.account().0.to_vec())];
    if !snapshot
        .storage_exists("SubtensorModule", "Owner", &key)
        .await?
    {
        return Ok(Vec::new());
    }
    let owner = snapshot.storage(&st::OWNER, (hotkey.account(),)).await?;
    if owner.get().copied().unwrap_or(AccountId::ZERO) != signer {
        return Ok(vec![format!(
            "the hotkey {hotkey} belongs to another coldkey; use your own or an unassociated hotkey"
        )]);
    }
    Ok(Vec::new())
}

// py: sdk/python/bittensor/intents/gamma.py:125-131
fn own_deposit_blocks(
    deposit: Option<&Deposit>,
    signer: AccountId,
    hotkey: &Ss58Address,
) -> Vec<String> {
    match deposit {
        None => vec![format!(
            "there is no gamma deposit at {hotkey} on this subnet"
        )],
        Some(deposit) if deposit.coldkey != signer => vec![format!(
            "the gamma deposit at {hotkey} belongs to another coldkey"
        )],
        Some(_) => Vec::new(),
    }
}

/// A mint quote: alpha burned, its TAO value and gamma minted, in chain units.
struct Quote {
    alpha: u64,
    value: u64,
    gamma: u128,
}

fn read_error(error: rtao_reads::ReadError) -> IntentError {
    match error {
        rtao_reads::ReadError::Client(error) => IntentError::Client(error),
        rtao_reads::ReadError::Chain { name, message } => {
            IntentError::Chain(ChainError::new(message, name))
        }
        error => IntentError::Chain(ChainError::with_code(error.to_string(), None, error.code())),
    }
}

// py: sdk/python/bittensor/intents/gamma.py:61-66
async fn quote<V: View + ?Sized>(
    view: &V,
    netuid: NetUid,
    with_stake: bool,
    amount: u64,
) -> Result<Quote, IntentError> {
    let quote = rtao_reads::reads::gamma::sim_mint(view, netuid, with_stake, amount)
        .await
        .map_err(read_error)?;
    Ok(Quote {
        alpha: quote.0,
        value: quote.1,
        gamma: quote.2,
    })
}

/// The deposit-naming fields shared by both mints.
struct MintTerms<'a> {
    netuid: NetUid,
    hotkey: &'a Ss58Address,
    rate_tolerance: Ratio,
    spender: Option<&'a Ss58Address>,
    service: Option<&'a Ss58Address>,
    cap: Option<u64>,
}

// py: sdk/python/bittensor/intents/gamma.py:146-207
/// A mint's preview: its quote, the deposit it credits and the hard stops
/// the chain would refuse it with.
async fn mint_preflight<V: View + ?Sized>(
    terms: &MintTerms<'_>,
    mut preview: IntentPreflight,
    view: &V,
    signer: AccountId,
    quote: Quote,
) -> Result<IntentPreflight, IntentError> {
    let deposit = deposit(view, terms.netuid, terms.hotkey.account()).await?;
    let named = named_spender(view, terms.netuid, terms.spender, terms.service).await?;
    let mut cap = terms.cap;
    let hotkey = terms.hotkey;
    match &deposit {
        Some(deposit) if deposit.coldkey != signer => preview.blocks.push(format!(
            "the gamma deposit at {hotkey} belongs to another coldkey; mint to a fresh hotkey"
        )),
        Some(deposit) => {
            if named.is_some_and(|named| named != deposit.spender) {
                preview.blocks.push(format!(
                    "the gamma deposit at {hotkey} is spent by {}; mint without naming a spender, or set its spender first",
                    deposit.spender
                ));
            }
            if cap.is_some_and(|cap| Some(cap) != deposit.cap) {
                preview.blocks.push(format!(
                    "the gamma deposit at {hotkey} has {}; mint without a cap, or set its cap first",
                    cap_text(deposit.cap)
                ));
            }
        }
        None => preview
            .blocks
            .extend(new_deposit_blocks(view, hotkey, signer).await?),
    }
    let spender = match (&deposit, named) {
        (Some(deposit), None) => Some(deposit.spender),
        (_, named) => named,
    };
    if let Some(deposit) = &deposit {
        cap = deposit.cap;
    }
    let minimum = min_out(quote.gamma, terms.rate_tolerance);
    let alpha = DynBalance::from_rao(quote.alpha, terms.netuid);
    let value = DynBalance::from_rao(quote.value, NetUid::ROOT);
    let mut facts = vec![
        ("gamma".to_owned(), format!("~{}", gamma_text(quote.gamma))),
        ("minimum".to_owned(), gamma_text(minimum)),
        ("alpha burned".to_owned(), format!("~{alpha}")),
        ("value".to_owned(), format!("~{value}")),
        (
            "deposit".to_owned(),
            deposit.as_ref().map_or_else(
                || "new".to_owned(),
                |deposit| format!("holds {}", gamma_text(u128::from(deposit.balance))),
            ),
        ),
        (
            "spender".to_owned(),
            spender.map_or_else(|| DEFAULT_SPENDER.to_owned(), |spender| spender.to_string()),
        ),
        (
            "daily spend cap".to_owned(),
            cap.map_or_else(|| "none".to_owned(), |cap| gamma_text(u128::from(cap))),
        ),
    ];
    facts.append(&mut preview.facts);
    preview.facts = facts;
    preview.facts_title = FactsTitle::Quote;
    let mut fields = Map::new();
    fields.insert("gamma".into(), json!(gamma_from_units(quote.gamma)));
    fields.insert("minimum_gamma".into(), json!(gamma_from_units(minimum)));
    fields.insert(
        "alpha_burned".into(),
        json!(gamma_from_units(u128::from(quote.alpha))),
    );
    fields.insert(
        "tao_value".into(),
        json!(gamma_from_units(u128::from(quote.value))),
    );
    fields.insert(
        "deposit_balance_gamma".into(),
        json!(
            deposit
                .as_ref()
                .map(|deposit| gamma_from_units(u128::from(deposit.balance)))
        ),
    );
    fields.insert(
        "spender_ss58".into(),
        json!(spender.map(|spender| spender.to_string())),
    );
    fields.insert(
        "daily_spend_cap_gamma".into(),
        json!(cap.map(|cap| gamma_from_units(u128::from(cap)))),
    );
    preview.preview = fields;
    Ok(preview)
}

fn tolerance_text(tolerance: Ratio) -> String {
    format!("{:.2}%", tolerance.get() * 100.0)
}

async fn base_preflight<I: Intent, V: View + ?Sized>(
    intent: &I,
    cx: &PreflightCx<'_, V>,
) -> Result<IntentPreflight, IntentError> {
    Ok(IntentPreflight {
        effects: intent.effects(cx).await?,
        warnings: intent.warnings(cx).await?,
        blocks: intent.blocks(cx).await?,
        ..Default::default()
    })
}

impl MintGamma {
    fn amount(&self) -> Result<u64, IntentError> {
        Ok(crate::arguments::tao_amount(&self.amount_tao)?.rao())
    }

    fn cap(&self) -> Result<Option<u64>, IntentError> {
        self.daily_spend_cap_gamma
            .as_ref()
            .map(|cap| gamma_units(cap, false).map(Option::unwrap_or_default))
            .transpose()
    }

    fn terms(&self) -> Result<MintTerms<'_>, IntentError> {
        Ok(MintTerms {
            netuid: self.netuid,
            hotkey: &self.hotkey_ss58,
            rate_tolerance: self.rate_tolerance,
            spender: self.spender_ss58.as_ref(),
            service: self.service_hotkey_ss58.as_ref(),
            cap: self.cap()?,
        })
    }
}

// py: sdk/python/bittensor/intents/gamma.py:210-289
impl Intent for MintGamma {
    fn summary(&self) -> String {
        let amount = match self.amount_tao.resolve(NetUid::ROOT, false) {
            Ok(rtao_primitives::ResolvedAmount::Exact(amount)) => amount.to_string(),
            _ => gamma_string(&self.amount_tao),
        };
        format!(
            "mint gamma with {amount} on netuid {} to {} (fails if the quote falls >{})",
            self.netuid,
            self.hotkey_ss58,
            tolerance_text(self.rate_tolerance)
        )
    }

    async fn build<V: View + ?Sized>(&self, cx: &BuildCx<'_, V>) -> Result<Built, IntentError> {
        self.validate()?;
        let amount = self.amount()?;
        let quote = quote(cx.view, self.netuid, false, amount).await?;
        let spender = named_spender(
            cx.view,
            self.netuid,
            self.spender_ss58.as_ref(),
            self.service_hotkey_ss58.as_ref(),
        )
        .await?;
        Ok(Built::new(
            calls::mint_gamma(
                self.hotkey_ss58.account(),
                self.netuid,
                TaoBalance(amount),
                units(min_out(quote.gamma, self.rate_tolerance)),
                spender,
                self.cap()?.map(|cap| units(u128::from(cap))),
            )
            .compose(cx.runtime)?,
        ))
    }

    async fn preflight<V: View + ?Sized>(
        &self,
        cx: &PreflightCx<'_, V>,
    ) -> Result<IntentPreflight, IntentError> {
        let preview = base_preflight(self, cx).await?;
        let quote = quote(cx.view, self.netuid, false, self.amount()?).await?;
        mint_preflight(&self.terms()?, preview, cx.view, cx.dispatch_origin, quote).await
    }

    fn spend(&self) -> Spend {
        crate::arguments::tao_amount(&self.amount_tao).map_or(Spend::Unbounded, Spend::Bounded)
    }

    fn validate(&self) -> Result<(), IntentError> {
        crate::arguments::tao_amount(&self.amount_tao)?;
        check_rate_tolerance(self.rate_tolerance)?;
        check_spender_or_service(
            "mint_gamma",
            self.spender_ss58.as_ref(),
            self.service_hotkey_ss58.as_ref(),
        )?;
        self.cap()?;
        Ok(())
    }
}

impl MintGammaWithStake {
    /// The requested alpha; `None` for `all`.
    fn requested(&self) -> Result<Option<u64>, IntentError> {
        match &self.amount_alpha {
            Amount::All => Ok(None),
            Amount::Exact(money) => Ok(Some(money.resolve(self.netuid)?.rao)),
        }
    }

    // py: sdk/python/bittensor/intents/gamma.py:331-333
    /// The alpha the chain burns: at most the stake held.
    fn burned(&self, staked: u64) -> Result<u64, IntentError> {
        Ok(self
            .requested()?
            .map_or(staked, |requested| requested.min(staked)))
    }

    fn cap(&self) -> Result<Option<u64>, IntentError> {
        self.daily_spend_cap_gamma
            .as_ref()
            .map(|cap| gamma_units(cap, false).map(Option::unwrap_or_default))
            .transpose()
    }

    async fn staked<V: View + ?Sized>(
        &self,
        view: &V,
        coldkey: AccountId,
    ) -> Result<u64, IntentError> {
        let position = view
            .call(
                &stake_api::GET_STAKE_INFO_FOR_HOTKEY_COLDKEY_NETUID,
                (self.origin_hotkey_ss58.account(), coldkey, self.netuid),
            )
            .await?;
        Ok(position.map_or(0, |position| position.stake.0))
    }
}

// py: sdk/python/bittensor/intents/gamma.py:292-391
impl Intent for MintGammaWithStake {
    fn summary(&self) -> String {
        let amount = match self.requested() {
            Ok(None) => "all".to_owned(),
            Ok(Some(rao)) => DynBalance::from_rao(rao, self.netuid).to_string(),
            Err(_) => gamma_string(&self.amount_alpha),
        };
        format!(
            "burn {amount} of stake into gamma on netuid {} to {} (fails if the quote falls >{})",
            self.netuid,
            self.hotkey_ss58,
            tolerance_text(self.rate_tolerance)
        )
    }

    async fn build<V: View + ?Sized>(&self, cx: &BuildCx<'_, V>) -> Result<Built, IntentError> {
        self.validate()?;
        let staked = self.staked(cx.view, cx.coldkey_address()?).await?;
        if staked == 0 {
            return Err(validation(format!(
                "nothing to burn into gamma: no stake on {} at netuid {}",
                self.origin_hotkey_ss58, self.netuid
            )));
        }
        let quote = quote(cx.view, self.netuid, true, self.burned(staked)?).await?;
        let spender = named_spender(
            cx.view,
            self.netuid,
            self.spender_ss58.as_ref(),
            self.service_hotkey_ss58.as_ref(),
        )
        .await?;
        Ok(Built::new(
            calls::mint_gamma_with_stake(
                self.origin_hotkey_ss58.account(),
                self.hotkey_ss58.account(),
                self.netuid,
                AlphaBalance(self.requested()?.unwrap_or(U64_MAX)),
                units(min_out(quote.gamma, self.rate_tolerance)),
                spender,
                self.cap()?.map(|cap| units(u128::from(cap))),
            )
            .compose(cx.runtime)?,
        ))
    }

    async fn warnings<V: View + ?Sized>(
        &self,
        _cx: &PreflightCx<'_, V>,
    ) -> Result<Vec<String>, IntentError> {
        Ok(if matches!(self.amount_alpha, Amount::All) {
            vec!["burns the entire stake on this hotkey on this subnet".into()]
        } else {
            Vec::new()
        })
    }

    async fn preflight<V: View + ?Sized>(
        &self,
        cx: &PreflightCx<'_, V>,
    ) -> Result<IntentPreflight, IntentError> {
        let mut preview = base_preflight(self, cx).await?;
        let staked = self.staked(cx.view, cx.dispatch_origin).await?;
        let alpha = self.burned(staked)?;
        if self.requested()?.is_some_and(|requested| alpha < requested) {
            preview.warnings.push(format!(
                "only {} is staked; that is burned",
                DynBalance::from_rao(staked, self.netuid)
            ));
        }
        let quote = quote(cx.view, self.netuid, true, alpha).await?;
        let terms = MintTerms {
            netuid: self.netuid,
            hotkey: &self.hotkey_ss58,
            rate_tolerance: self.rate_tolerance,
            spender: self.spender_ss58.as_ref(),
            service: self.service_hotkey_ss58.as_ref(),
            cap: self.cap()?,
        };
        mint_preflight(&terms, preview, cx.view, cx.dispatch_origin, quote).await
    }

    fn spend(&self) -> Spend {
        // Destroys stake; not TAO-denominated, so a spend cap must block it.
        Spend::Unbounded
    }

    fn validate(&self) -> Result<(), IntentError> {
        self.requested()?;
        check_rate_tolerance(self.rate_tolerance)?;
        check_spender_or_service(
            "mint_gamma_with_stake",
            self.spender_ss58.as_ref(),
            self.service_hotkey_ss58.as_ref(),
        )?;
        self.cap()?;
        Ok(())
    }
}

impl MoveGamma {
    async fn deposits<V: View + ?Sized>(
        &self,
        view: &V,
    ) -> Result<(Option<Deposit>, Option<Deposit>), IntentError> {
        Ok((
            deposit(view, self.netuid, self.origin_hotkey_ss58.account()).await?,
            deposit(view, self.netuid, self.destination_hotkey_ss58.account()).await?,
        ))
    }
}

// py: sdk/python/bittensor/intents/gamma.py:394-482
impl Intent for MoveGamma {
    fn summary(&self) -> String {
        format!(
            "move {} gamma on netuid {} from {} to {}",
            gamma_string(&self.amount_gamma),
            self.netuid,
            self.origin_hotkey_ss58,
            self.destination_hotkey_ss58
        )
    }

    async fn build<V: View + ?Sized>(&self, cx: &BuildCx<'_, V>) -> Result<Built, IntentError> {
        self.validate()?;
        let amount = gamma_units(&self.amount_gamma, true)?.unwrap_or(U64_MAX);
        Ok(Built::new(
            calls::move_gamma(
                self.origin_hotkey_ss58.account(),
                self.destination_hotkey_ss58.account(),
                self.netuid,
                units(u128::from(amount)),
            )
            .compose(cx.runtime)?,
        ))
    }

    async fn blocks<V: View + ?Sized>(
        &self,
        cx: &PreflightCx<'_, V>,
    ) -> Result<Vec<String>, IntentError> {
        let signer = cx.dispatch_origin;
        let (origin, destination) = self.deposits(cx.view).await?;
        let mut blocks = own_deposit_blocks(origin.as_ref(), signer, &self.origin_hotkey_ss58);
        if blocks.is_empty() && origin.as_ref().is_some_and(|origin| origin.balance == 0) {
            blocks.push(format!(
                "the gamma deposit at {} holds no gamma",
                self.origin_hotkey_ss58
            ));
        }
        match &destination {
            Some(_) => blocks.extend(own_deposit_blocks(
                destination.as_ref(),
                signer,
                &self.destination_hotkey_ss58,
            )),
            None => blocks
                .extend(new_deposit_blocks(cx.view, &self.destination_hotkey_ss58, signer).await?),
        }
        Ok(blocks)
    }

    async fn warnings<V: View + ?Sized>(
        &self,
        cx: &PreflightCx<'_, V>,
    ) -> Result<Vec<String>, IntentError> {
        let (origin, destination) = self.deposits(cx.view).await?;
        let Some(origin) = origin else {
            return Ok(Vec::new());
        };
        let mut warnings = Vec::new();
        let amount = gamma_units(&self.amount_gamma, true)?;
        if amount.is_none_or(|amount| amount >= origin.balance) {
            warnings.push("moves the entire balance, which closes the origin deposit".into());
        }
        let Some(destination) = destination else {
            return Ok(warnings);
        };
        if destination.spender != origin.spender {
            warnings.push(format!(
                "the moved gamma becomes spendable by {}, the destination deposit's spender, instead of {}",
                destination.spender, origin.spender
            ));
        }
        if destination.cap != origin.cap {
            let text = |cap: Option<u64>| {
                cap.map_or_else(|| "none".to_owned(), |cap| gamma_text(u128::from(cap)))
            };
            warnings.push(format!(
                "the moved gamma falls under the destination deposit's daily spend cap, {}, instead of {}",
                text(destination.cap),
                text(origin.cap)
            ));
        }
        Ok(warnings)
    }

    fn spend(&self) -> Spend {
        // Hands the moved gamma to whoever holds the destination hotkey.
        Spend::Unbounded
    }

    fn validate(&self) -> Result<(), IntentError> {
        if self.origin_hotkey_ss58.account() == self.destination_hotkey_ss58.account() {
            return Err(validation(
                "move_gamma: the origin and destination hotkeys are the same",
            ));
        }
        if gamma_units(&self.amount_gamma, true)? == Some(0) {
            return Err(validation("move_gamma: the amount must be above 0"));
        }
        Ok(())
    }
}

// py: sdk/python/bittensor/intents/gamma.py:485-518
impl Intent for CloseGammaDeposit {
    fn summary(&self) -> String {
        format!(
            "close the gamma deposit at {} on netuid {}",
            self.hotkey_ss58, self.netuid
        )
    }

    async fn build<V: View + ?Sized>(&self, cx: &BuildCx<'_, V>) -> Result<Built, IntentError> {
        Ok(Built::new(
            calls::close_gamma_deposit(self.hotkey_ss58.account(), self.netuid)
                .compose(cx.runtime)?,
        ))
    }

    async fn blocks<V: View + ?Sized>(
        &self,
        cx: &PreflightCx<'_, V>,
    ) -> Result<Vec<String>, IntentError> {
        let deposit = deposit(cx.view, self.netuid, self.hotkey_ss58.account()).await?;
        let mut blocks =
            own_deposit_blocks(deposit.as_ref(), cx.dispatch_origin, &self.hotkey_ss58);
        if let Some(deposit) = deposit.as_ref().filter(|deposit| deposit.balance != 0)
            && blocks.is_empty()
        {
            blocks.push(format!(
                "the deposit holds {}; move it out first, or close it once its spender has spent it",
                gamma_text(u128::from(deposit.balance))
            ));
        }
        Ok(blocks)
    }
}

// py: sdk/python/bittensor/intents/gamma.py:521-580
impl Intent for SetGammaDepositSpender {
    fn summary(&self) -> String {
        let spender = match (&self.service_hotkey_ss58, &self.spender_ss58) {
            (Some(service), _) => format!("the spender of the service at {service}"),
            (None, Some(spender)) => spender.to_string(),
            (None, None) => DEFAULT_SPENDER.to_owned(),
        };
        format!(
            "let {spender} spend the gamma deposit at {} on netuid {}",
            self.hotkey_ss58, self.netuid
        )
    }

    async fn build<V: View + ?Sized>(&self, cx: &BuildCx<'_, V>) -> Result<Built, IntentError> {
        self.validate()?;
        let spender = named_spender(
            cx.view,
            self.netuid,
            self.spender_ss58.as_ref(),
            self.service_hotkey_ss58.as_ref(),
        )
        .await?;
        Ok(Built::new(
            calls::set_gamma_deposit_spender(self.hotkey_ss58.account(), self.netuid, spender)
                .compose(cx.runtime)?,
        ))
    }

    async fn blocks<V: View + ?Sized>(
        &self,
        cx: &PreflightCx<'_, V>,
    ) -> Result<Vec<String>, IntentError> {
        let deposit = deposit(cx.view, self.netuid, self.hotkey_ss58.account()).await?;
        Ok(own_deposit_blocks(
            deposit.as_ref(),
            cx.dispatch_origin,
            &self.hotkey_ss58,
        ))
    }

    fn spend(&self) -> Spend {
        // Hands the deposit's whole balance to the new spender.
        Spend::Unbounded
    }

    fn validate(&self) -> Result<(), IntentError> {
        let named = usize::from(self.spender_ss58.is_some())
            + usize::from(self.service_hotkey_ss58.is_some())
            + usize::from(self.default_spender);
        if named != 1 {
            return Err(validation(
                "set_gamma_deposit_spender: set exactly one of spender_ss58, service_hotkey_ss58 or default_spender",
            ));
        }
        Ok(())
    }
}

impl SetGammaDepositDailySpendCap {
    fn cap(&self) -> Result<Option<u64>, IntentError> {
        self.daily_spend_cap_gamma
            .as_ref()
            .map(|cap| gamma_units(cap, false).map(Option::unwrap_or_default))
            .transpose()
    }
}

// py: sdk/python/bittensor/intents/gamma.py:583-627
impl Intent for SetGammaDepositDailySpendCap {
    fn summary(&self) -> String {
        let cap = self.daily_spend_cap_gamma.as_ref().map_or_else(
            || "no cap".to_owned(),
            |cap| format!("a cap of {} gamma a day", gamma_string(cap)),
        );
        format!(
            "set {cap} on the gamma deposit at {} on netuid {}",
            self.hotkey_ss58, self.netuid
        )
    }

    async fn build<V: View + ?Sized>(&self, cx: &BuildCx<'_, V>) -> Result<Built, IntentError> {
        self.validate()?;
        Ok(Built::new(
            calls::set_gamma_deposit_daily_spend_cap(
                self.hotkey_ss58.account(),
                self.netuid,
                self.cap()?.map(|cap| units(u128::from(cap))),
            )
            .compose(cx.runtime)?,
        ))
    }

    async fn blocks<V: View + ?Sized>(
        &self,
        cx: &PreflightCx<'_, V>,
    ) -> Result<Vec<String>, IntentError> {
        let deposit = deposit(cx.view, self.netuid, self.hotkey_ss58.account()).await?;
        Ok(own_deposit_blocks(
            deposit.as_ref(),
            cx.dispatch_origin,
            &self.hotkey_ss58,
        ))
    }

    fn validate(&self) -> Result<(), IntentError> {
        if self.daily_spend_cap_gamma.is_none() != self.remove_cap {
            return Err(validation(
                "set_gamma_deposit_daily_spend_cap: set exactly one of daily_spend_cap_gamma or remove_cap",
            ));
        }
        self.cap()?;
        Ok(())
    }
}

/// One normalized `spend_gamma` entry.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GammaSpend {
    /// Hotkey of the deposit spent from.
    pub hotkey: Ss58Address,
    /// Exact decimal gamma amount.
    pub amount_gamma: String,
    /// Chain units of `amount_gamma`.
    pub units: u64,
    /// The 32-byte reference repeated in the spend's event.
    pub reference: [u8; 32],
}

// py: sdk/python/bittensor/intents/gamma.py:74-84
fn reference(value: Option<&Value>) -> Result<[u8; 32], IntentError> {
    let Some(value) = value else {
        return Ok([0; 32]);
    };
    let text = match value {
        Value::String(text) => text.clone(),
        other => other.to_string(),
    };
    let raw = pyfmt::bytes_fromhex(text.strip_prefix("0x").unwrap_or(&text)).unwrap_or_default();
    raw.try_into().map_err(|_| {
        validation(format!(
            "a spend reference is 32 bytes as hex, not {}",
            match value {
                Value::String(text) => pyfmt::string_repr(text),
                other => other.to_string(),
            }
        ))
    })
}

impl SpendGamma {
    // py: sdk/python/bittensor/intents/gamma.py:661-683
    /// The entries, normalized and checked as Python's constructor does.
    pub fn entries(&self) -> Result<Vec<GammaSpend>, IntentError> {
        if !(1..=MAX_GAMMA_SPENDS).contains(&self.spends.len()) {
            return Err(validation(format!(
                "spend_gamma takes 1 to {MAX_GAMMA_SPENDS} spends"
            )));
        }
        self.spends
            .iter()
            .enumerate()
            .map(|(index, spend)| {
                let object = spend.as_object().filter(|object| {
                    object.contains_key("hotkey_ss58")
                        && object.contains_key("amount_gamma")
                        && object
                            .keys()
                            .all(|key| matches!(key.as_str(), "hotkey_ss58" | "amount_gamma" | "reference"))
                });
                let Some(object) = object else {
                    return Err(validation(format!(
                        "spend_gamma: entry {} must be an object of hotkey_ss58, amount_gamma and an optional reference, not {spend}",
                        index + 1
                    )));
                };
                let hotkey: Ss58Address = object["hotkey_ss58"]
                    .as_str()
                    .and_then(|text| text.parse().ok())
                    .ok_or_else(|| {
                        validation(format!(
                            "hotkey_ss58 must be an SS58 address, not {}",
                            object["hotkey_ss58"]
                        ))
                    })?;
                let amount_gamma = rtao_primitives::gamma::gamma_amount_json(
                    &object["amount_gamma"],
                    false,
                )
                .map_err(|error| validation(error.message()))?;
                let units = u64::try_from(
                    rtao_primitives::gamma::gamma_units(&amount_gamma)
                        .map_err(|error| validation(error.message()))?,
                )
                .map_err(|_| validation(format!("gamma amount {amount_gamma} exceeds u64 units")))?;
                Ok(GammaSpend {
                    hotkey,
                    amount_gamma,
                    units,
                    reference: reference(object.get("reference"))?,
                })
            })
            .collect()
    }
}

/// Replace the given entries with Python's normalized dictionaries.
pub(crate) fn canonicalize(intent: &mut SpendGamma) -> Result<(), IntentError> {
    intent.spends = intent
        .entries()?
        .into_iter()
        .map(|spend| {
            json!({
                "hotkey_ss58": spend.hotkey.as_str(),
                "amount_gamma": spend.amount_gamma,
                "reference": format!("0x{}", hex::encode(spend.reference)),
            })
        })
        .collect();
    Ok(())
}

// py: sdk/python/bittensor/intents/gamma.py:630-725
impl Intent for SpendGamma {
    fn summary(&self) -> String {
        let Ok(entries) = self.entries() else {
            return format!("spend gamma on netuid {}", self.netuid);
        };
        let total: u128 = entries.iter().map(|spend| u128::from(spend.units)).sum();
        format!(
            "spend {} gamma in {} {} on netuid {}",
            gamma_from_units(total),
            entries.len(),
            if entries.len() == 1 {
                "entry"
            } else {
                "entries"
            },
            self.netuid
        )
    }

    async fn build<V: View + ?Sized>(&self, cx: &BuildCx<'_, V>) -> Result<Built, IntentError> {
        let spends = self
            .entries()?
            .into_iter()
            .map(|spend| {
                (
                    spend.hotkey.account(),
                    units(u128::from(spend.units)),
                    spend.reference,
                )
            })
            .collect();
        Ok(Built::new(
            calls::spend_gamma(self.netuid, spends).compose(cx.runtime)?,
        ))
    }

    async fn warnings<V: View + ?Sized>(
        &self,
        cx: &PreflightCx<'_, V>,
    ) -> Result<Vec<String>, IntentError> {
        let entries = self.entries()?;
        let mut skipped = Vec::new();
        for spend in &entries {
            let deposit = deposit(cx.view, self.netuid, spend.hotkey.account()).await?;
            if deposit.is_none_or(|deposit| deposit.spender != cx.dispatch_origin) {
                skipped.push(spend.hotkey.to_string());
            }
        }
        if skipped.is_empty() {
            return Ok(Vec::new());
        }
        if skipped.len() == entries.len() {
            return Ok(vec![
                "nothing would be spent: no entry is a deposit you are the spender of".into(),
            ]);
        }
        Ok(vec![format!(
            "{} of {} entries would be skipped, having no deposit you are the spender of: {}",
            skipped.len(),
            entries.len(),
            skipped.join(", ")
        )])
    }

    fn validate(&self) -> Result<(), IntentError> {
        self.entries().map(|_| ())
    }
}
