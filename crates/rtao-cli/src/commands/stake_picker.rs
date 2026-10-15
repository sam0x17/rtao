// Derived from RaoFoundation/subtensor sdk/python/bittensor/cli/stake_picker.py @ 850f2755bb05a3851f948489f0ab27b0645cebce (Apache-2.0); modified by rtao.
// Additional source at the same commit/license: sdk/python/bittensor/cli/root_helpers.py.
//! Public-only stake source, validator target and destination pickers.

use crate::{
    books::{AccountLookup, FsAccountLookup, NameCandidate, address_cli_name},
    context::AppContext,
    error::CliError,
    prompt::{PromptError, PromptHistory, Terminal, python_strip},
};
use rtao::{
    client::Client,
    primitives::{AccountId, DynBalance, NetUid},
    reads::reads::{
        m6_locks::{self, ColdkeyLock},
        m6_staking::{self, StakeAvailability},
        owner::{self, StakePosition, StakeValuation},
    },
};
use serde_json::{Map, Value, json};
use std::collections::{BTreeMap, BTreeSet};

/// Spot TAO below which a position is omitted from the displayed choices.
pub const DUST_RAO: u64 = 1_000_000;

fn failure(error: impl std::fmt::Display) -> CliError {
    CliError::failure(error.to_string())
}
fn prompt_error(error: PromptError) -> CliError {
    CliError {
        code: error.exit_code(),
        message: error.to_string(),
        help: None,
    }
}
fn message(ctx: &AppContext, text: &str) -> Result<(), CliError> {
    let mut terminal = ctx.terminal.clone();
    terminal.message(text).map_err(prompt_error)
}
fn read(ctx: &AppContext, label: &str) -> Result<String, CliError> {
    let mut terminal = ctx.terminal.clone();
    if !ctx.prompt_context().interactive(&terminal) {
        return Err(prompt_error(PromptError::Unavailable));
    }
    terminal
        .read_line(label)
        .map(|raw| python_strip(&raw).to_owned())
        .map_err(prompt_error)
}

fn numbered(raw: &str) -> bool {
    !raw.is_empty()
        && raw.chars().all(|ch| {
            unicode_general_category::get_general_category(ch)
                == unicode_general_category::GeneralCategory::DecimalNumber
        })
}
fn choice_index(raw: &str) -> Option<usize> {
    rtao::primitives::pyfmt::parse_integer(raw)?
        .to_string()
        .parse::<usize>()
        .ok()?
        .checked_sub(1)
}

/// Resolve the stake owner without asking an account to sign anything.
/// Proxies and saved multisigs precede external or ordinary wallet accounts.
pub async fn owner(ctx: &AppContext) -> Result<AccountId, CliError> {
    if let Some(account) = ctx.dispatch_proxy(None)? {
        return Ok(account);
    }
    if ctx.books.multisig(&ctx.globals.wallet_name).is_some() {
        return ctx
            .resolve_address("coldkey_ss58", None)?
            .ok_or_else(|| CliError::input("saved multisig has no account"));
    }
    if ctx.globals.uses_external_signer() {
        if let Some(account) = ctx.external_signer_address()? {
            return Ok(account);
        }
        if ctx.prompt_context().extension_signer {
            return Err(CliError::input(
                "stake selection with the extension signer requires --signer-address",
            ));
        }
        return ctx
            .external_signer_public_account()
            .await?
            .ok_or_else(|| CliError::input("external signer requires --signer-address"));
    }
    ctx.resolve_address("coldkey_ss58", None)?
        .ok_or_else(|| CliError::input("wallet has no usable coldkey"))
}

/// Show the free balance before the command's ordinary amount prompt.
/// The caller retains its normal amount parser and records its own answer.
pub async fn free_balance(ctx: &AppContext, client: &Client) -> Result<DynBalance, CliError> {
    let account = owner(ctx).await?;
    let balance = owner::balance(client, account).await.map_err(failure)?;
    message(ctx, &format!("free balance of {account}: {balance}"))?;
    Ok(balance)
}

/// Convert alpha to spot TAO using Python's float product and truncation.
/// Root amounts stay exact and every result is checked against native u64 rao.
pub fn spot_value(valuation: &StakeValuation, stake: &DynBalance) -> Result<DynBalance, CliError> {
    let rao = if stake.netuid == NetUid::ROOT {
        stake.rao
    } else {
        let price = valuation
            .tao_per_alpha
            .get(&stake.netuid)
            .copied()
            .unwrap_or(0.0);
        let product = (stake.rao as f64 * price).trunc();
        if !(0.0..18_446_744_073_709_551_616.0).contains(&product) {
            return Err(CliError::input("spot value exceeds native u64 rao"));
        }
        product as u64
    };
    Ok(DynBalance::from_rao(rao, NetUid::ROOT))
}

fn sum(left: u64, right: u64) -> Result<u64, CliError> {
    left.checked_add(right)
        .ok_or_else(|| CliError::input("total spot value exceeds native u64 rao"))
}

/// One selectable source position, including hidden dust positions.
#[derive(Clone, Debug, PartialEq)]
pub struct SourceRow {
    /// Staking hotkey.
    pub hotkey: AccountId,
    /// Currency when this row represents exactly one subnet.
    pub netuid: Option<NetUid>,
    /// Subnet or aggregate description.
    pub location: String,
    /// Exact native stake display or an aggregate spot value.
    pub stake: String,
    /// Checked spot TAO used for sorting and dust filtering.
    pub value: DynBalance,
    /// Alpha stake also shows the approximate TAO value.
    pub show_value: bool,
    /// Conviction lock held on a different hotkey.
    pub note: String,
}

/// One subnet option, selected by actual netuid rather than display position.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SubnetChoice {
    /// Runtime subnet ID.
    pub netuid: NetUid,
    /// Positive positions on this subnet.
    pub count: usize,
    /// Checked total spot TAO in native rao.
    pub value_rao: u64,
}

/// Group positive positions by subnet, stably sorting descending by spot value.
pub fn subnet_choices(
    positions: &[StakePosition],
    valuation: &StakeValuation,
) -> Result<Vec<SubnetChoice>, CliError> {
    let mut choices: Vec<SubnetChoice> = Vec::new();
    let mut indices = BTreeMap::new();
    for position in positions.iter().filter(|position| position.stake.rao > 0) {
        let index = *indices.entry(position.netuid).or_insert_with(|| {
            choices.push(SubnetChoice {
                netuid: position.netuid,
                count: 0,
                value_rao: 0,
            });
            choices.len() - 1
        });
        choices[index].value_rao = sum(
            choices[index].value_rao,
            spot_value(valuation, &position.stake)?.rao,
        )?;
        choices[index].count += 1;
    }
    choices.sort_by(|a, b| b.value_rao.cmp(&a.value_rao));
    Ok(choices)
}

/// Build position or hotkey rows, retaining runtime order when values tie.
/// A lock hint is displayed for positions on every other hotkey.
pub fn source_rows(
    positions: &[StakePosition],
    valuation: &StakeValuation,
    per_position: bool,
    lock: Option<(&ColdkeyLock, &StakeAvailability)>,
    names: &BTreeMap<AccountId, String>,
) -> Result<Vec<SourceRow>, CliError> {
    let lock_hotkey = lock
        .filter(|(_, availability)| availability.locked.rao > 0)
        .map(|(lock, _)| lock.hotkey);
    let note = |hotkey: AccountId| match lock_hotkey.filter(|locked| *locked != hotkey) {
        Some(locked) => format!(
            "lock on {}",
            names
                .get(&locked)
                .cloned()
                .unwrap_or_else(|| locked.to_string())
        ),
        None => String::new(),
    };
    let mut rows: Vec<SourceRow> = Vec::new();
    let mut indices = BTreeMap::new();
    let mut counts: Vec<usize> = Vec::new();
    for position in positions.iter().filter(|position| position.stake.rao > 0) {
        let value = spot_value(valuation, &position.stake)?;
        if !per_position && let Some(index) = indices.get(&position.hotkey).copied() {
            let row: &mut SourceRow = &mut rows[index];
            row.value.rao = sum(row.value.rao, value.rao)?;
            counts[index] += 1;
            row.netuid = None;
            row.location = format!("{} subnets", counts[index]);
            row.stake = format!("≈ {}", row.value);
            row.show_value = false;
            continue;
        }
        indices.insert(position.hotkey, rows.len());
        counts.push(1);
        rows.push(SourceRow {
            hotkey: position.hotkey,
            netuid: Some(position.netuid),
            location: if per_position {
                String::new()
            } else {
                format!("netuid {}", position.netuid)
            },
            stake: position.stake.to_string(),
            value,
            show_value: position.netuid != NetUid::ROOT,
            note: note(position.hotkey),
        });
    }
    rows.sort_by(|a, b| b.value.rao.cmp(&a.value.rao));
    Ok(rows)
}

fn local_candidates(ctx: &AppContext) -> Result<Vec<NameCandidate>, CliError> {
    let lookup = FsAccountLookup::new(ctx.globals.wallet_path.clone()).map_err(failure)?;
    let mut candidates = lookup.candidates();
    sort_candidates(&mut candidates);
    Ok(candidates)
}
fn sort_candidates(candidates: &mut [NameCandidate]) {
    candidates.sort_by(|a, b| {
        let (aw, ah) = a.name.split_once('/').unwrap_or((&a.name, ""));
        let (bw, bh) = b.name.split_once('/').unwrap_or((&b.name, ""));
        super::wallet::natural_cmp(aw, bw).then_with(|| super::wallet::natural_cmp(ah, bh))
    });
}
fn local_names(ctx: &AppContext) -> Result<BTreeMap<AccountId, String>, CliError> {
    let mut names = BTreeMap::new();
    for candidate in local_candidates(ctx)? {
        if candidate.source == "hotkey"
            && let Ok(account) = candidate.address.parse()
        {
            names.entry(account).or_insert_with(|| {
                candidate
                    .name
                    .rsplit('/')
                    .next()
                    .unwrap_or(&candidate.name)
                    .to_owned()
            });
        }
    }
    for entry in &ctx.books.addresses {
        if let (Some(address), Some(name)) = (entry["address"].as_str(), entry["name"].as_str())
            && !name.is_empty()
            && let Ok(account) = address.parse()
        {
            names.entry(account).or_insert_with(|| name.to_owned());
        }
    }
    Ok(names)
}
async fn fill_names(
    client: &Client,
    hotkeys: impl IntoIterator<Item = AccountId>,
    names: &mut BTreeMap<AccountId, String>,
) -> Result<(), CliError> {
    let missing: Vec<_> = hotkeys
        .into_iter()
        .filter(|hotkey| !names.contains_key(hotkey))
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    if missing.is_empty() {
        return Ok(());
    }
    for (hotkey, identity) in owner::hotkey_identities(client, missing)
        .await
        .map_err(failure)?
    {
        if let Some(name) = identity["name"].as_str().filter(|name| !name.is_empty()) {
            names.entry(hotkey).or_insert_with(|| name.to_owned());
        }
    }
    Ok(())
}
fn chosen(
    ctx: &AppContext,
    field: &str,
    account: AccountId,
    name: Option<&str>,
    answers: &mut Map<String, Value>,
    history: &mut PromptHistory,
) -> Result<(), CliError> {
    if let Some(name) = name {
        ctx.output
            .name_address(&account.to_string(), name)
            .map_err(failure)?;
    }
    answers.insert(field.into(), json!(account));
    history.record_answers(vec![address_cli_name(field), account.to_string()]);
    Ok(())
}
fn resolve(ctx: &AppContext, field: &str, raw: &str) -> Result<Option<AccountId>, CliError> {
    match ctx.resolve_address(field, Some(raw)) {
        Ok(account) => Ok(account),
        Err(error) => {
            message(ctx, &error.to_string())?;
            Ok(None)
        }
    }
}

/// Select existing stake owned by the current proxy, multisig or public signer.
pub async fn source(
    ctx: &AppContext,
    client: &Client,
    hotkey_field: &str,
    netuid_field: Option<&str>,
    answers: &mut Map<String, Value>,
    history: &mut PromptHistory,
) -> Result<(), CliError> {
    source_for_owner(
        ctx,
        client,
        owner(ctx).await?,
        hotkey_field,
        netuid_field,
        answers,
        history,
    )
    .await
}

/// Select stake for an explicitly resolved read target, including remote owners.
pub async fn source_for_owner(
    ctx: &AppContext,
    client: &Client,
    account: AccountId,
    hotkey_field: &str,
    netuid_field: Option<&str>,
    answers: &mut Map<String, Value>,
    history: &mut PromptHistory,
) -> Result<(), CliError> {
    let valuation = owner::stake_value_for_coldkey(client, account)
        .await
        .map_err(failure)?;
    let mut positions: Vec<_> = valuation
        .positions
        .iter()
        .filter(|p| p.stake.rao > 0)
        .cloned()
        .collect();
    let mut names = local_names(ctx)?;
    fill_names(client, positions.iter().map(|p| p.hotkey), &mut names).await?;
    let mut netuid = netuid_field
        .and_then(|field| answers.get(field))
        .filter(|value| !value.is_null())
        .map(|value| {
            value
                .as_u64()
                .or_else(|| value.as_str()?.parse().ok())
                .and_then(|value| u16::try_from(value).ok())
                .map(NetUid)
                .ok_or_else(|| CliError::input("stake source netuid must be a u16"))
        })
        .transpose()?;
    if let Some(field) = netuid_field
        && netuid.is_none()
        && !positions.is_empty()
    {
        let selected = ask_netuid(ctx, field, &subnet_choices(&positions, &valuation)?)?;
        answers.insert(field.into(), json!(selected.0));
        history.record_answers(vec![
            format!("--{}", field.replace('_', "-")),
            selected.to_string(),
        ]);
        netuid = Some(selected);
    }
    if let Some(netuid) = netuid {
        positions.retain(|position| position.netuid == netuid);
    }
    if positions.is_empty() {
        let location = netuid
            .map(|n| format!(" on netuid {n}"))
            .unwrap_or_default();
        return Err(
            CliError::failure(format!("{account} has no stake{location}"))
                .with_help("`rtao stake list` shows every position"),
        );
    }
    let lock_info = if let Some(netuid) = netuid {
        let (lock, availability) = tokio::try_join!(
            m6_locks::coldkey_lock(client, account, netuid),
            m6_staking::stake_availability(client, account, netuid),
        )
        .map_err(failure)?;
        if let Some(lock) = &lock {
            fill_names(client, [lock.hotkey], &mut names).await?;
        }
        Some((lock, availability))
    } else {
        None
    };
    let rows = source_rows(
        &positions,
        &valuation,
        netuid_field.is_some(),
        lock_info
            .as_ref()
            .and_then(|(lock, availability)| lock.as_ref().map(|lock| (lock, availability))),
        &names,
    )?;
    let mut visible: Vec<_> = rows
        .iter()
        .filter(|row| row.value.rao >= DUST_RAO)
        .collect();
    if visible.is_empty() {
        visible = rows.iter().collect();
    }
    let flag = address_cli_name(hotkey_field);
    message(
        ctx,
        &format!("{flag}: Where the stake comes from — pick a position below."),
    )?;
    for (index, row) in visible.iter().enumerate() {
        let label = names
            .get(&row.hotkey)
            .cloned()
            .unwrap_or_else(|| row.hotkey.to_string());
        let location = if row.location.starts_with("netuid ") {
            row.netuid
                .map(|n| format!("netuid {}", ctx.output.subnet_text(n, false)))
                .unwrap_or_else(|| row.location.clone())
        } else {
            row.location.clone()
        };
        let value = if row.show_value {
            format!("  ≈ {}", row.value)
        } else {
            String::new()
        };
        message(
            ctx,
            &format!(
                "  {}  {label}  {location}  {}{value}  {}",
                index + 1,
                row.stake,
                row.note
            ),
        )?;
    }
    if let Some((Some(lock), availability)) = &lock_info
        && availability.locked.rao > 0
    {
        let label = names
            .get(&lock.hotkey)
            .cloned()
            .unwrap_or_else(|| lock.hotkey.to_string());
        message(
            ctx,
            &format!(
                "netuid {}: {} locked · {} free · lock → {label}",
                lock.netuid, availability.locked, availability.available
            ),
        )?;
    }
    let hidden = rows.len() - visible.len();
    if hidden > 0 {
        message(
            ctx,
            &format!(
                "+ {hidden} dust position{} hidden (under τ0.001) — answer with a hotkey to use one",
                if hidden == 1 { "" } else { "s" }
            ),
        )?;
    }
    let selected = ask_source(ctx, hotkey_field, &rows, &visible)?;
    chosen(
        ctx,
        hotkey_field,
        selected,
        names.get(&selected).map(String::as_str),
        answers,
        history,
    )
}

fn ask_source(
    ctx: &AppContext,
    hotkey_field: &str,
    rows: &[SourceRow],
    visible: &[&SourceRow],
) -> Result<AccountId, CliError> {
    let flag = address_cli_name(hotkey_field);
    if rows.len() == 1 {
        message(ctx, "only staked position — selected")?;
        return Ok(rows[0].hotkey);
    }
    loop {
        let raw = read(ctx, &format!("  {} [1]: ", flag.trim_start_matches('-')))?;
        let raw = if raw.is_empty() { "1" } else { &raw };
        if numbered(raw) {
            if let Some(row) = choice_index(raw).and_then(|i| visible.get(i)) {
                return Ok(row.hotkey);
            }
            message(
                ctx,
                &format!("enter a number between 1 and {}", visible.len()),
            )?;
            continue;
        }
        if let Some(account) = resolve(ctx, hotkey_field, raw)? {
            if !rows.iter().any(|row| row.hotkey == account) {
                message(ctx, "note: no stake found on that hotkey — using it anyway")?;
            }
            return Ok(account);
        }
    }
}

fn ask_netuid(ctx: &AppContext, field: &str, choices: &[SubnetChoice]) -> Result<NetUid, CliError> {
    let first = choices
        .first()
        .ok_or_else(|| CliError::failure("no staked subnets"))?;
    let flag = format!("--{}", field.replace('_', "-"));
    message(
        ctx,
        &format!("{flag}: Subnet the stake comes from — answer with a netuid below."),
    )?;
    let all_dust = choices.iter().all(|choice| choice.value_rao < DUST_RAO);
    let visible: Vec<_> = choices
        .iter()
        .filter(|choice| all_dust || choice.value_rao >= DUST_RAO)
        .collect();
    for choice in &visible {
        let label = format!("netuid {}", ctx.output.subnet_text(choice.netuid, false));
        message(
            ctx,
            &format!(
                "  {label}  {} position{}  ≈ {}",
                choice.count,
                if choice.count == 1 { "" } else { "s" },
                DynBalance::from_rao(choice.value_rao, NetUid::ROOT)
            ),
        )?;
    }
    let hidden = choices.len() - visible.len();
    if hidden > 0 {
        message(
            ctx,
            &format!(
                "+ {hidden} subnet{} holding only dust hidden (under τ0.001) — answering with a hidden netuid still works",
                if hidden == 1 { "" } else { "s" }
            ),
        )?;
    }
    if choices.len() == 1 {
        message(ctx, "only staked subnet — selected")?;
        return Ok(first.netuid);
    }
    loop {
        let raw = read(
            ctx,
            &format!("  {} [{}]: ", flag.trim_start_matches('-'), first.netuid),
        )?;
        if raw.is_empty() {
            return Ok(first.netuid);
        }
        if let Some(netuid) = rtao::primitives::pyfmt::parse_integer(&raw)
            .and_then(|value| value.to_string().parse::<u16>().ok())
        {
            if choices.iter().any(|choice| choice.netuid == NetUid(netuid)) {
                return Ok(NetUid(netuid));
            }
            message(
                ctx,
                &format!("no stake on netuid {netuid} — pick one of the subnets above"),
            )?;
        } else {
            message(ctx, "answer with one of the netuids above")?;
        }
    }
}

/// Pick a validator from local hotkeys or any accepted hotkey reference.
pub fn target(
    ctx: &AppContext,
    field: &str,
    answers: &mut Map<String, Value>,
    history: &mut PromptHistory,
) -> Result<(), CliError> {
    let prefix = format!("{}/", ctx.globals.wallet_name);
    let choices = local_candidates(ctx)?
        .into_iter()
        .filter(|candidate| candidate.source == "hotkey" && candidate.name.starts_with(&prefix))
        .collect::<Vec<_>>();
    pick_account(ctx, field, &choices, true, answers, history)
}

/// Pick a recipient from contacts and other local coldkeys, accepting any name.
pub fn destination(
    ctx: &AppContext,
    field: &str,
    answers: &mut Map<String, Value>,
    history: &mut PromptHistory,
) -> Result<(), CliError> {
    let mut seen = BTreeSet::new();
    let mut choices = Vec::new();
    for row in &ctx.books.addresses {
        if let (Some(name), Some(address)) = (row["name"].as_str(), row["address"].as_str())
            && !address.is_empty()
            && seen.insert(address.to_owned())
        {
            choices.push(NameCandidate {
                name: name.into(),
                address: address.into(),
                source: "address-book".into(),
            });
        }
    }
    for candidate in local_candidates(ctx)? {
        if candidate.source == "wallet"
            && candidate.name != ctx.globals.wallet_name
            && seen.insert(candidate.address.clone())
        {
            choices.push(candidate);
        }
    }
    pick_account(ctx, field, &choices, false, answers, history)
}

/// Pick a root validator from the owner's non-dust principal and accrued yield.
/// An empty selection ends the command successfully without preparing a call.
pub async fn claim_root_source(
    ctx: &AppContext,
    client: &Client,
    field: &str,
    answers: &mut Map<String, Value>,
    history: &mut PromptHistory,
) -> Result<(), CliError> {
    let positions = crate::root_helpers::fetch_root_positions(client, owner(ctx).await?).await?;
    let held = held_root_positions(&positions)?;
    if held.is_empty() {
        ctx.output
            .message("no root position above dust (τ0.001) for this wallet")
            .map_err(failure)?;
        return Err(CliError {
            code: 0,
            message: String::new(),
            help: None,
        });
    }
    let mut names = local_names(ctx)?;
    fill_names(
        client,
        held.iter().map(|position| position.hotkey),
        &mut names,
    )
    .await?;
    let selected = ask_claim_root(ctx, field, &held, &names)?;
    chosen(
        ctx,
        field,
        selected,
        names.get(&selected).map(String::as_str),
        answers,
        history,
    )
}

fn held_root_positions(
    positions: &[crate::root_helpers::RootPosition],
) -> Result<Vec<&crate::root_helpers::RootPosition>, CliError> {
    let mut held = Vec::new();
    for position in positions {
        let total = position.total()?.rao;
        if total >= crate::root_helpers::DUST_POSITION_RAO {
            held.push((total, position));
        }
    }
    held.sort_by(|a, b| b.0.cmp(&a.0));
    Ok(held.into_iter().map(|(_, position)| position).collect())
}

fn ask_claim_root(
    ctx: &AppContext,
    field: &str,
    held: &[&crate::root_helpers::RootPosition],
    names: &BTreeMap<AccountId, String>,
) -> Result<AccountId, CliError> {
    let flag = address_cli_name(field);
    let first = held
        .first()
        .ok_or_else(|| CliError::failure("no root positions"))?;
    message(
        ctx,
        &format!("{flag}: Validator to claim from — answer with a number, name, or hotkey."),
    )?;
    let mut matchable = BTreeMap::new();
    for (index, position) in held.iter().enumerate() {
        let label = names
            .get(&position.hotkey)
            .cloned()
            .unwrap_or_else(|| position.hotkey.to_string());
        message(
            ctx,
            &format!(
                "  {}  {label}  total {}  (staked {}, accrued {})",
                index + 1,
                position.total()?,
                position.staked,
                position.accrued
            ),
        )?;
        matchable.insert((index + 1).to_string(), position.hotkey);
    }
    for position in held {
        matchable.insert(position.hotkey.to_string(), position.hotkey);
    }
    for position in held {
        let label = names
            .get(&position.hotkey)
            .cloned()
            .unwrap_or_else(|| position.hotkey.to_string());
        matchable.entry(label).or_insert(position.hotkey);
    }
    if held.len() == 1 {
        message(ctx, "only validator — selected")?;
        return Ok(first.hotkey);
    }
    loop {
        let raw = read(ctx, &format!("  {} [1]: ", flag.trim_start_matches('-')))?;
        if raw.is_empty() {
            return Ok(first.hotkey);
        }
        if let Some(account) = matchable.get(&raw) {
            return Ok(*account);
        }
        let close = matchable
            .iter()
            .filter(|(key, account)| key.starts_with(&raw) && account.to_string().starts_with(&raw))
            .map(|(_, account)| *account)
            .collect::<Vec<_>>();
        if close.len() == 1 {
            return Ok(close[0]);
        }
        message(ctx, "enter a list number or hotkey")?;
    }
}

fn pick_account(
    ctx: &AppContext,
    field: &str,
    choices: &[NameCandidate],
    hotkey: bool,
    answers: &mut Map<String, Value>,
    history: &mut PromptHistory,
) -> Result<(), CliError> {
    let flag = address_cli_name(field);
    let explanation = if hotkey {
        "Validator hotkey the stake goes to — it does not have to be a local key."
    } else {
        "Destination account — a coldkey, not a hotkey."
    };
    message(ctx, &format!("{flag}: {explanation}"))?;
    for (index, candidate) in choices.iter().enumerate() {
        let kind = if candidate.source == "address-book" || hotkey {
            ""
        } else {
            "  wallet"
        };
        message(
            ctx,
            &format!(
                "  {}  {}  {}{kind}",
                index + 1,
                candidate.name,
                candidate.address
            ),
        )?;
    }
    if choices.is_empty() {
        message(
            ctx,
            if hotkey {
                "no local hotkeys — paste the validator's ss58 address or an address-book name"
            } else {
                "no address-book contacts or other local wallets — paste the destination ss58"
            },
        )?;
    } else {
        message(
            ctx,
            if hotkey {
                "a number above, or any hotkey: ss58 address, address-book name, or WALLET/HOTKEY"
            } else {
                "a number above, or any account: ss58 address, address-book name, or local wallet name"
            },
        )?;
    }
    loop {
        let raw = read(ctx, &format!("  {}: ", flag.trim_start_matches('-')))?;
        if raw.is_empty() {
            message(ctx, "a value is required")?;
            continue;
        }
        if !choices.is_empty() && numbered(&raw) {
            if let Some(candidate) = choice_index(&raw).and_then(|i| choices.get(i)) {
                let account = candidate.address.parse().map_err(failure)?;
                return chosen(ctx, field, account, Some(&candidate.name), answers, history);
            }
            message(
                ctx,
                &format!("enter a number between 1 and {}", choices.len()),
            )?;
            continue;
        }
        if let Some(account) = resolve(ctx, field, &raw)? {
            return chosen(ctx, field, account, None, answers, history);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        books::Books,
        globals::Globals,
        output::{MemorySink, Output, OutputOptions},
    };
    use std::{
        collections::VecDeque,
        path::Path,
        sync::{Arc, Mutex},
    };

    type TestResult = Result<(), Box<dyn std::error::Error>>;
    type Messages = Arc<Mutex<Vec<String>>>;
    const COLD: AccountId = AccountId([7; 32]);
    const A: AccountId = AccountId([8; 32]);
    const B: AccountId = AccountId([9; 32]);
    const DUST: AccountId = AccountId([10; 32]);

    struct Script {
        answers: VecDeque<String>,
        messages: Messages,
    }
    impl Terminal for Script {
        fn stdin_is_tty(&self) -> bool {
            true
        }
        fn stderr_is_tty(&self) -> bool {
            true
        }
        fn read_line(&mut self, _: &str) -> Result<String, PromptError> {
            self.answers.pop_front().ok_or(PromptError::Aborted)
        }
        fn read_secret(&mut self, _: &str) -> Result<rtao::wallet::SecretString, PromptError> {
            Err(PromptError::Unavailable)
        }
        fn message(&mut self, text: &str) -> Result<(), PromptError> {
            self.messages
                .lock()
                .map_err(|_| PromptError::Io)?
                .push(text.into());
            Ok(())
        }
    }
    fn context(replies: &[&str]) -> Result<(AppContext, Messages), Box<dyn std::error::Error>> {
        let matches = crate::application::command_tree([clap::Command::new("fixture")])
            .try_get_matches_from(["rtao", "fixture"])?;
        let mut globals = Globals::resolve(
            &matches,
            &Map::new(),
            Path::new("/rtao-no-fixture-home"),
            |_| None,
        )?;
        globals.wallet_path = Path::new("/rtao-no-fixture-wallets").into();
        let messages = Arc::new(Mutex::new(Vec::new()));
        let script = Script {
            answers: replies.iter().map(|v| (*v).into()).collect(),
            messages: messages.clone(),
        };
        let mut ctx = AppContext::new(
            globals,
            Map::new(),
            Books::default(),
            Arc::new(Mutex::new(Box::new(script))),
        );
        ctx.output = Output::new(OutputOptions::default(), Arc::new(MemorySink::default()));
        Ok((ctx, messages))
    }
    fn position(hotkey: AccountId, netuid: u16, rao: u64) -> StakePosition {
        StakePosition {
            hotkey,
            coldkey: COLD,
            netuid: NetUid(netuid),
            stake: DynBalance::from_rao(rao, NetUid(netuid)),
            is_registered: true,
        }
    }
    fn valuation(positions: Vec<StakePosition>) -> StakeValuation {
        StakeValuation {
            coldkey: COLD,
            block: 10,
            positions,
            stake_value: DynBalance::from_rao(0, NetUid::ROOT),
            tao_per_alpha: BTreeMap::from([
                (NetUid(10), 0.5),
                (NetUid(20), 1.0),
                (NetUid(30), 1.0),
            ]),
        }
    }

    #[test]
    fn local_candidate_order_is_natural_per_wallet_then_per_hotkey() {
        let mut candidates = ["a2/hk1", "a/hk10", "a/hk2", "a/hk1"].map(|name| NameCandidate {
            name: name.into(),
            address: A.to_string(),
            source: "hotkey".into(),
        });
        sort_candidates(&mut candidates);
        assert_eq!(
            candidates.map(|row| row.name),
            ["a/hk1", "a/hk2", "a/hk10", "a2/hk1"]
        );
    }

    #[test]
    fn root_rao_is_exact_while_alpha_uses_python_product_truncation() -> TestResult {
        let mut value = valuation(Vec::new());
        assert_eq!(
            spot_value(&value, &DynBalance::from_rao(u64::MAX, NetUid::ROOT))?.rao,
            u64::MAX
        );
        assert_eq!(
            spot_value(&value, &DynBalance::from_rao(3, NetUid(10)))?.rao,
            1
        );
        assert_eq!(
            spot_value(&value, &DynBalance::from_rao(100, NetUid(999)))?.rao,
            0
        );
        value.tao_per_alpha.insert(NetUid(10), f64::NAN);
        assert!(spot_value(&value, &DynBalance::from_rao(3, NetUid(10))).is_err());
        value.tao_per_alpha.insert(NetUid(10), -1.0);
        assert!(spot_value(&value, &DynBalance::from_rao(3, NetUid(10))).is_err());
        value.tao_per_alpha.insert(NetUid(10), 1.0);
        // Converting u64::MAX to binary64 rounds it to 2^64. Casting must never saturate.
        assert!(spot_value(&value, &DynBalance::from_rao(u64::MAX, NetUid(10))).is_err());
        Ok(())
    }

    #[test]
    fn subnet_rows_retain_runtime_ties_and_aggregate_exact_values() -> TestResult {
        let value = valuation(vec![
            position(A, 20, 2_000_000),
            position(B, 10, 4_000_000),
            position(DUST, 30, 1),
            position(A, 20, 0),
        ]);
        let rows = subnet_choices(&value.positions, &value)?;
        assert_eq!(
            rows,
            vec![
                SubnetChoice {
                    netuid: NetUid(20),
                    count: 1,
                    value_rao: 2_000_000
                },
                SubnetChoice {
                    netuid: NetUid(10),
                    count: 1,
                    value_rao: 2_000_000
                },
                SubnetChoice {
                    netuid: NetUid(30),
                    count: 1,
                    value_rao: 1
                }
            ]
        );
        let overflow = valuation(vec![position(A, 0, u64::MAX), position(B, 0, 1)]);
        assert!(subnet_choices(&overflow.positions, &overflow).is_err());
        Ok(())
    }

    #[test]
    fn position_and_aggregate_rows_keep_currencies_stable_values_and_lock_hints() -> TestResult {
        let value = valuation(vec![
            position(A, 10, 4_000_000),
            position(B, 20, 2_000_000),
            position(A, 20, 3),
            position(DUST, 30, 1),
        ]);
        let lock = ColdkeyLock {
            hotkey: B,
            netuid: NetUid(20),
            locked_alpha: DynBalance::from_rao(5, NetUid(20)),
            is_perpetual: true,
        };
        let availability = StakeAvailability {
            netuid: NetUid(20),
            total: DynBalance::from_rao(10, NetUid(20)),
            locked: lock.locked_alpha.clone(),
            available: DynBalance::from_rao(5, NetUid(20)),
        };
        let names = BTreeMap::from([(B, "locked validator".into())]);
        let per = source_rows(
            &value.positions,
            &value,
            true,
            Some((&lock, &availability)),
            &names,
        )?;
        assert_eq!(
            per.iter().map(|r| r.hotkey).collect::<Vec<_>>(),
            vec![A, B, A, DUST]
        );
        assert_eq!(per[0].netuid, Some(NetUid(10)));
        assert!(per[0].location.is_empty());
        assert!(per[0].show_value);
        assert_eq!(per[0].note, "lock on locked validator");
        assert!(per[1].note.is_empty());
        let aggregate = source_rows(&value.positions, &value, false, None, &names)?;
        assert_eq!(aggregate[0].hotkey, A);
        assert_eq!(aggregate[0].value.rao, 2_000_003);
        assert_eq!(aggregate[0].location, "2 subnets");
        assert_eq!(aggregate[0].netuid, None);
        assert!(!aggregate[0].show_value);
        assert_eq!(aggregate[1].location, "netuid 20");
        let overflow = valuation(vec![position(A, 0, u64::MAX), position(A, 20, 1)]);
        assert!(source_rows(&overflow.positions, &overflow, false, None, &names).is_err());
        Ok(())
    }

    #[test]
    fn subnet_prompt_uses_actual_netuids_defaults_and_hidden_dust_not_row_numbers() -> TestResult {
        let choices = [
            SubnetChoice {
                netuid: NetUid(20),
                count: 1,
                value_rao: 2_000_000,
            },
            SubnetChoice {
                netuid: NetUid(10),
                count: 1,
                value_rao: 1,
            },
        ];
        let (ctx, messages) = context(&["1", "1_0"])?;
        assert_eq!(ask_netuid(&ctx, "origin_netuid", &choices)?, NetUid(10));
        let messages = messages.lock().map_err(|_| "messages")?.join("\n");
        assert!(messages.contains("no stake on netuid 1"));
        assert!(messages.contains("answering with a hidden netuid still works"));
        let (ctx, _) = context(&[""])?;
        assert_eq!(ask_netuid(&ctx, "netuid", &choices)?, NetUid(20));
        let (ctx, _) = context(&["１０"])?;
        assert_eq!(ask_netuid(&ctx, "netuid", &choices)?, NetUid(10));
        Ok(())
    }

    #[test]
    fn source_prompt_can_select_hidden_dust_by_contact_or_default_to_largest() -> TestResult {
        let value = valuation(vec![position(A, 20, 2_000_000), position(DUST, 30, 1)]);
        let rows = source_rows(&value.positions, &value, false, None, &BTreeMap::new())?;
        let visible = vec![&rows[0]];
        let (mut ctx, _) = context(&["dust contact"])?;
        ctx.books.addresses = vec![json!({"name":"dust contact","address":DUST})];
        assert_eq!(ask_source(&ctx, "hotkey_ss58", &rows, &visible)?, DUST);
        let (ctx, _) = context(&[""])?;
        assert_eq!(ask_source(&ctx, "hotkey_ss58", &rows, &visible)?, A);
        let (ctx, _) = context(&["١"])?;
        assert_eq!(ask_source(&ctx, "hotkey_ss58", &rows, &visible)?, A);
        Ok(())
    }

    #[test]
    fn source_prompt_allows_explicit_unstaked_hotkey_and_aborts_at_eof() -> TestResult {
        let value = valuation(vec![position(A, 20, 2_000_000), position(DUST, 30, 1)]);
        let rows = source_rows(&value.positions, &value, false, None, &BTreeMap::new())?;
        let visible = rows.iter().collect::<Vec<_>>();
        let target = B.to_string();
        let (ctx, messages) = context(&[&target])?;
        assert_eq!(ask_source(&ctx, "hotkey_ss58", &rows, &visible)?, B);
        assert!(
            messages
                .lock()
                .map_err(|_| "messages")?
                .join("\n")
                .contains("no stake found")
        );
        let (ctx, _) = context(&[])?;
        assert_eq!(
            ask_source(&ctx, "hotkey_ss58", &rows, &visible)
                .err()
                .ok_or("must abort")?
                .code,
            130
        );
        Ok(())
    }

    #[test]
    fn destination_contact_numbers_deduplicate_and_replay_canonical_accounts() -> TestResult {
        let (mut ctx, _) = context(&["٢"])?;
        ctx.books.addresses = vec![
            json!({"name":"first","address":A}),
            json!({"name":"duplicate","address":A}),
            json!({"name":"second","address":B}),
        ];
        let mut answers = Map::new();
        let mut history = PromptHistory::default();
        destination(&ctx, "dest_coldkey_ss58", &mut answers, &mut history)?;
        assert_eq!(answers["dest_coldkey_ss58"], json!(B));
        let replay = history.replay_command(&["rtao".into(), "stake".into(), "transfer".into()]);
        assert!(replay.contains(&format!("--dest-coldkey {B}")));
        assert!(!replay.contains("duplicate"));
        Ok(())
    }

    #[test]
    fn target_accepts_arbitrary_public_contact_without_local_hotkey_or_signing() -> TestResult {
        let (mut ctx, messages) = context(&["remote validator"])?;
        ctx.books.addresses = vec![json!({"name":"remote validator","address":B})];
        let mut answers = Map::new();
        target(
            &ctx,
            "dest_hotkey_ss58",
            &mut answers,
            &mut PromptHistory::default(),
        )?;
        assert_eq!(answers["dest_hotkey_ss58"], json!(B));
        assert!(
            messages
                .lock()
                .map_err(|_| "messages")?
                .join("\n")
                .contains("no local hotkeys")
        );
        Ok(())
    }

    #[test]
    fn claim_root_choices_use_total_including_accrued_dust_boundary_and_exact_overflow()
    -> TestResult {
        let root_position = |hotkey, staked, accrued| crate::root_helpers::RootPosition {
            hotkey,
            staked: DynBalance::from_rao(staked, NetUid::ROOT),
            accrued: DynBalance::from_rao(accrued, NetUid::ROOT),
            wallet: None,
            coldkey: None,
        };
        let positions = [
            root_position(A, 500_000, 500_000),
            root_position(B, 2_000_000, 0),
            root_position(DUST, 0, 999_999),
        ];
        let held = held_root_positions(&positions)?;
        assert_eq!(
            held.iter().map(|p| p.hotkey).collect::<Vec<_>>(),
            vec![B, A]
        );
        let (ctx, _) = context(&[""])?;
        assert_eq!(
            ask_claim_root(&ctx, "hotkey_ss58", &held, &BTreeMap::new())?,
            B
        );
        let (ctx, _) = context(&["2"])?;
        assert_eq!(
            ask_claim_root(&ctx, "hotkey_ss58", &held, &BTreeMap::new())?,
            A
        );
        assert!(held_root_positions(&[root_position(A, u64::MAX, 1)]).is_err());
        Ok(())
    }

    #[test]
    fn claim_root_accepts_identity_name_and_unique_address_prefix_but_rejects_unknown() -> TestResult
    {
        let positions = [A, B].map(|hotkey| crate::root_helpers::RootPosition {
            hotkey,
            staked: DynBalance::from_rao(2_000_000, NetUid::ROOT),
            accrued: DynBalance::from_rao(0, NetUid::ROOT),
            wallet: None,
            coldkey: None,
        });
        let held = held_root_positions(&positions)?;
        let names = BTreeMap::from([(B, "chain validator".into())]);
        let (ctx, messages) = context(&["unknown", "chain validator"])?;
        assert_eq!(ask_claim_root(&ctx, "hotkey_ss58", &held, &names)?, B);
        assert!(
            messages
                .lock()
                .map_err(|_| "messages")?
                .join("\n")
                .contains("enter a list number or hotkey")
        );
        let prefix = B.to_string();
        let (ctx, _) = context(&[&prefix[..prefix.len() - 1]])?;
        assert_eq!(ask_claim_root(&ctx, "hotkey_ss58", &held, &names)?, B);
        Ok(())
    }

    #[tokio::test]
    async fn owner_proxy_then_saved_multisig_then_pinned_external_account() -> TestResult {
        let (mut ctx, _) = context(&[])?;
        ctx.globals.wallet_name = "team".into();
        ctx.globals.signer_backend = Some("extension".into());
        ctx.globals.signer_address = Some(DUST.to_string());
        ctx.books.multisigs = vec![json!({"name":"team","threshold":2,"signatories":[A,B]})];
        ctx.globals.proxy_for = Some(COLD.to_string());
        assert_eq!(owner(&ctx).await?, COLD);
        ctx.globals.proxy_for = Some("self".into());
        let derived = ctx
            .resolve_address("coldkey_ss58", None)?
            .ok_or("multisig account")?;
        assert_ne!(derived, DUST);
        assert_eq!(owner(&ctx).await?, derived);
        ctx.books.multisigs.clear();
        assert_eq!(owner(&ctx).await?, DUST);
        ctx.globals.signer_address = None;
        assert!(
            owner(&ctx)
                .await
                .err()
                .ok_or("missing address")?
                .message
                .contains("--signer-address")
        );
        Ok(())
    }
}
