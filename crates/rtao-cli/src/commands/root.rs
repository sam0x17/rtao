// Derived from RaoFoundation/subtensor sdk/python/bittensor/cli/commands/root.py @ 850f2755bb05a3851f948489f0ab27b0645cebce (Apache-2.0); modified by rtao.
// Derived from RaoFoundation/subtensor sdk/python/bittensor/cli/root_helpers.py @ 850f2755bb05a3851f948489f0ab27b0645cebce (Apache-2.0); modified by rtao.
// Derived from RaoFoundation/subtensor sdk/python/bittensor/cli/commands/root_move.py @ 850f2755bb05a3851f948489f0ab27b0645cebce (Apache-2.0); modified by rtao.
// Derived from RaoFoundation/subtensor sdk/python/bittensor/cli/commands/root_swap.py @ 850f2755bb05a3851f948489f0ab27b0645cebce (Apache-2.0); modified by rtao.
//! Root baskets, accrued-yield claims and root position moves.

use super::generated::{self, TxRequest};
use crate::{
    books::{AccountLookup, FsAccountLookup},
    context::{AppContext, CommandReview, ExternalSignerFactory, SubmissionOptions},
    error::CliError,
    globals::{Tier, with_globals},
    output::{Output, ReviewSection, TableSpec},
    prompt::{PromptError, PromptHistory, PromptSpec, PromptValidation, Terminal, ask_with},
    root_helpers::{self, RootPosition, basket_index},
};
use clap::{Arg, ArgAction, ArgMatches, Command};
use rtao::{
    client::{BlockRef, Client, ReadPlan, ViewExt},
    core::codec::Value as DynValue,
    primitives::{AccountId, Amount, DynBalance, Money, NetUid, ResolvedAmount},
    reads::reads::{m6_root, m6_staking, owner},
    tx::ExtrinsicResult,
};
use rtao_runtime::storage::subtensor_module as st;
use serde_json::{Map, Value, json};
use std::collections::{BTreeMap, BTreeSet};

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
fn optional<'a>(matches: &'a ArgMatches, field: &str) -> Option<&'a str> {
    matches
        .try_get_one::<String>(field)
        .ok()
        .flatten()
        .map(String::as_str)
}
fn flag(matches: &ArgMatches, field: &str) -> bool {
    matches
        .try_get_one::<bool>(field)
        .ok()
        .flatten()
        .copied()
        .unwrap_or(false)
}
fn account_arg(field: &'static str, long: &'static str, help: &'static str) -> Arg {
    Arg::new(field).long(long).help(help)
}
fn slippage(raw: &str) -> Result<f64, String> {
    raw.parse::<f64>()
        .ok()
        .filter(|value| value.is_finite() && (0.0..=100.0).contains(value))
        .ok_or_else(|| "max slippage must be finite and within 0..100 percent".into())
}

/// Build the source-shaped root command family.
pub fn command() -> Result<Command, CliError> {
    let hotkey = || {
        account_arg(
            "hotkey_ss58",
            "hotkey",
            "Validator hotkey or local account name.",
        )
    };
    let coldkey = || {
        account_arg(
            "coldkey_ss58",
            "coldkey",
            "Public coldkey whose positions to inspect.",
        )
    };
    let mut command = Command::new("root")
        .about("Root network: validator baskets, swaps, and your TAO positions.")
        .subcommand_required(true)
        .arg_required_else_help(true)
        .subcommand(
            Command::new("list")
                .about("Validator funds, one validator, or your root positions.")
                .arg(Arg::new("validator").index(1))
                .arg(
                    Arg::new("mine")
                        .long("mine")
                        .short('m')
                        .action(ArgAction::SetTrue),
                )
                .arg(
                    Arg::new("all_wallets")
                        .long("all")
                        .short('a')
                        .action(ArgAction::SetTrue),
                )
                .arg(coldkey())
                .arg(Arg::new("dust").long("dust").action(ArgAction::SetTrue)),
        )
        .subcommand(
            Command::new("claim")
                .about("Redeem accrued basket yield into root stake; principal stays staked.")
                .arg(hotkey())
                .arg(coldkey()),
        )
        .subcommand(
            Command::new("allocate")
                .about("Buy basket shares with free TAO.")
                .arg(hotkey())
                .arg(Arg::new("amount").long("amount"))
                .arg(
                    Arg::new("all_amount")
                        .long("all")
                        .action(ArgAction::SetTrue),
                ),
        )
        .subcommand(
            Command::new("move")
                .about("Claim yield, then move the whole root position to another validator.")
                .arg(Arg::new("origin").long("from"))
                .arg(Arg::new("dest").long("to"))
                .arg(coldkey()),
        )
        .subcommand(
            Command::new("swap")
                .about("Rebalance a validator basket between subnet holdings.")
                .arg(hotkey())
                .arg(
                    Arg::new("origin_netuid")
                        .long("from")
                        .required(true)
                        .value_parser(clap::value_parser!(u16).range(0..4096)),
                )
                .arg(
                    Arg::new("dest_netuid")
                        .long("to")
                        .required(true)
                        .value_parser(clap::value_parser!(u16).range(0..4096)),
                )
                .arg(Arg::new("amount").long("amount").required(true))
                .arg(
                    Arg::new("max_slippage")
                        .long("max-slippage")
                        .default_value("1")
                        .value_parser(clap::builder::ValueParser::new(slippage)),
                ),
        )
        .subcommand(generated::intent_command("root_register")?.name("register"));
    for name in ["list", "claim", "allocate", "move", "swap"] {
        command = command.mut_subcommand(name, |leaf| {
            with_globals(leaf, if name == "list" { Tier::Read } else { Tier::Tx })
        });
    }
    Ok(command)
}

/// Leaf-owned names that must not be overwritten by global options.
pub fn reserved(matches: &ArgMatches) -> Result<Vec<&'static str>, CliError> {
    match matches
        .subcommand()
        .ok_or_else(|| CliError::input("missing root command"))?
        .0
    {
        "list" => Ok(vec![
            "validator",
            "mine",
            "all_wallets",
            "coldkey_ss58",
            "dust",
        ]),
        "claim" => Ok(vec!["hotkey_ss58", "coldkey_ss58"]),
        "allocate" => Ok(vec!["hotkey_ss58", "amount", "all_amount"]),
        "move" => Ok(vec!["origin", "dest", "coldkey_ss58"]),
        "swap" => Ok(vec![
            "hotkey_ss58",
            "origin_netuid",
            "dest_netuid",
            "amount",
            "max_slippage",
        ]),
        "register" => generated::intent_reserved("root_register"),
        name => Err(CliError::input(format!("unknown root command {name:?}"))),
    }
}

/// Public inputs, resolved locally before chain reads where possible.
pub enum Request {
    /// Fund leaderboard, validator detail or public root positions.
    List {
        /// Address, root UID or validator name.
        validator: Option<String>,
        /// Public account, resolved before chain access.
        owner: Option<AccountId>,
        /// Wallet names and public coldkeys, for the all-wallets view.
        wallets: Vec<(String, AccountId)>,
        /// Whether to show positions instead of the leaderboard.
        mine: bool,
        /// Whether the all-wallets view was requested, even if empty.
        all_wallets: bool,
        /// Include dust in human output; JSON always contains it.
        dust: bool,
    },
    /// Redeem only accrued yield into the source's root stake.
    Claim {
        /// Explicit target; absent for the terminal position picker.
        hotkey: Option<AccountId>,
        /// Owner resolved before the picker reads.
        owner: AccountId,
    },
    /// Buy beta with TAO, preserving the all sentinel for SDK fee/ED planning.
    Allocate {
        /// Explicit target; absent for the terminal fund picker.
        hotkey: Option<AccountId>,
        /// Decimal money or all; absent for a prompt.
        amount: Option<String>,
    },
    /// Claim and move the complete root position without free-balance withdrawal.
    Move {
        /// Source selector, or a position picker.
        origin: Option<String>,
        /// Destination selector, or a fund picker.
        dest: Option<String>,
        /// Position owner resolved before chain access.
        owner: AccountId,
    },
    /// Basket swap inputs; output floor is filled from a best-effort quote.
    Swap {
        /// Validator basket.
        hotkey: AccountId,
        /// Origin subnet or root cash.
        origin: NetUid,
        /// Destination subnet or root cash.
        dest: NetUid,
        /// Exact origin money or all.
        amount: String,
        /// Finite percentage within 0..100.
        max_slippage: f64,
    },
    /// Ordinary registered SDK transaction.
    Transaction(TxRequest),
}

fn interactive(ctx: &AppContext) -> bool {
    ctx.prompt_context().interactive(&ctx.terminal)
}
fn missing_target(flag: &str) -> CliError {
    CliError::input(format!("missing required option: `{flag}`")).with_help(format!(
        "pass `{flag}`, or run on a terminal to pick a validator"
    ))
}
fn target(
    ctx: &mut AppContext,
    param: &str,
    value: Option<&str>,
    history: &mut PromptHistory,
) -> Result<AccountId, CliError> {
    ctx.resolve_target(param, value, history)?
        .ok_or_else(|| CliError::input(format!("missing public {param}")))
}
// Python's root CLI accepts Decimal text before constructing an intent.
// Keep that grammar at this boundary; Money/Amount constructors remain strict.
// Raw text and exponent magnitude are capped independently to bound all scratch
// work, including Unicode digit normalization and irrelevant zero exponents.
fn normalize_root_amount(raw: &str, netuid: NetUid) -> Result<String, CliError> {
    const TEXT_BUDGET: usize = 4096;
    const EXPONENT_BUDGET: u64 = 4096;
    if raw.len() > TEXT_BUDGET {
        return Err(CliError::input(
            "root amount exceeds its 4096-byte normalization budget",
        ));
    }
    let text = crate::prompt::python_strip(raw);
    if text.eq_ignore_ascii_case("all") {
        return Ok("all".into());
    }
    let invalid = || CliError::input("root amount must be a finite non-negative decimal or 'all'");
    // Decimal(string) ignores underscores and accepts Unicode decimal digits.
    let text = text
        .chars()
        .filter(|character| *character != '_')
        .collect::<String>();
    let (negative, text) = match text.strip_prefix('-') {
        Some(unsigned) => (true, unsigned),
        None => (false, text.strip_prefix('+').unwrap_or(&text)),
    };
    let decimal_digits = |value: &str| {
        value.chars().all(|character| {
            unicode_general_category::get_general_category(character)
                == unicode_general_category::GeneralCategory::DecimalNumber
        })
    };
    let (mantissa, exponent) = match text.find(['e', 'E']) {
        Some(index) => {
            let (mantissa, suffix) = text.split_at(index);
            let exponent = &suffix[1..];
            let unsigned = exponent.strip_prefix(['+', '-']).unwrap_or(exponent);
            if unsigned.is_empty() || !decimal_digits(unsigned) {
                return Err(invalid());
            }
            let exponent = rtao::primitives::pyfmt::parse_integer(exponent)
                .ok_or_else(invalid)?
                .to_string()
                .parse::<i64>()
                .map_err(|_| {
                    CliError::input("root amount exponent exceeds its normalization budget")
                })?;
            if exponent.unsigned_abs() > EXPONENT_BUDGET {
                return Err(CliError::input(
                    "root amount exponent exceeds its normalization budget",
                ));
            }
            (mantissa, exponent)
        }
        None => (text, 0),
    };
    let (integer, fraction) = mantissa.split_once('.').unwrap_or((mantissa, ""));
    if (integer.is_empty() && fraction.is_empty())
        || !decimal_digits(integer)
        || !decimal_digits(fraction)
    {
        return Err(invalid());
    }
    let coefficient = rtao::primitives::pyfmt::parse_integer(&format!("{integer}{fraction}"))
        .ok_or_else(invalid)?
        .to_string();
    if negative && coefficient != "0" {
        return Err(CliError::input("amount must be non-negative"));
    }
    // Remove redundant zero precision before the exact native constructor.
    // This admits e.g. 10e-10 as one rao without admitting 1e-10 or rounding.
    let significant = coefficient.trim_end_matches('0');
    let (significant, power) = if significant.is_empty() {
        ("0", 0)
    } else {
        let fraction_digits = i64::try_from(fraction.chars().count()).map_err(failure)?;
        let trailing_zeros =
            i64::try_from(coefficient.len() - significant.len()).map_err(failure)?;
        let power = exponent
            .checked_sub(fraction_digits)
            .and_then(|power| power.checked_add(trailing_zeros))
            .ok_or_else(|| {
                CliError::input("root amount exponent exceeds its normalization budget")
            })?;
        (significant, power)
    };
    let balance = DynBalance::from_amount(format!("{significant}e{power}"), netuid)
        .map_err(|error| CliError::input(error.to_string()))?;
    Ok(Money::from_rao(balance.rao).to_decimal_string())
}
fn position_wallet(ctx: &mut AppContext, owner: AccountId) -> Result<(), CliError> {
    if let Some((name, _)) = local_wallets(ctx)?
        .into_iter()
        .find(|(_, account)| *account == owner)
    {
        ctx.globals.wallet_name = name;
    }
    ctx.globals.wallet_given = true;
    Ok(())
}

impl Request {
    /// Parse and validate without starting any chain or signing work.
    pub fn from_matches(
        ctx: &mut AppContext,
        matches: &ArgMatches,
        history: &mut PromptHistory,
    ) -> Result<Self, CliError> {
        let (name, leaf) = matches
            .subcommand()
            .ok_or_else(|| CliError::input("missing root command"))?;
        match name {
            "list" => {
                let all_wallets = flag(leaf, "all_wallets");
                let mine = all_wallets || flag(leaf, "mine");
                let validator = optional(leaf, "validator").map(str::to_owned);
                if mine && validator.is_some() {
                    return Err(CliError::input(
                        "--mine lists your positions across validators; drop the validator argument, or drop --mine for that fund's detail view",
                    ));
                }
                let wallets = if all_wallets {
                    local_wallets(ctx)?
                } else {
                    vec![]
                };
                if all_wallets && wallets.is_empty() {
                    return Err(failure("no wallets found"));
                }
                let owner = if !all_wallets && (mine || validator.is_some()) {
                    Some(target(
                        ctx,
                        "coldkey_ss58",
                        optional(leaf, "coldkey_ss58"),
                        history,
                    )?)
                } else {
                    None
                };
                Ok(Self::List {
                    validator,
                    owner,
                    wallets,
                    mine,
                    all_wallets,
                    dust: flag(leaf, "dust"),
                })
            }
            "claim" => {
                let raw = optional(leaf, "hotkey_ss58");
                if raw.is_none() && !interactive(ctx) {
                    return Err(missing_target("--hotkey"));
                }
                let hotkey = raw
                    .map(|raw| target(ctx, "hotkey_ss58", Some(raw), history))
                    .transpose()?;
                let owner = target(ctx, "coldkey_ss58", optional(leaf, "coldkey_ss58"), history)?;
                if hotkey.is_none() {
                    position_wallet(ctx, owner)?;
                }
                Ok(Self::Claim { hotkey, owner })
            }
            "allocate" => {
                let raw = optional(leaf, "hotkey_ss58");
                if raw.is_none() && !interactive(ctx) {
                    return Err(missing_target("--hotkey"));
                }
                let hotkey = raw
                    .map(|raw| target(ctx, "hotkey_ss58", Some(raw), history))
                    .transpose()?;
                let mut amount = optional(leaf, "amount").map(str::to_owned);
                if flag(leaf, "all_amount") {
                    if amount.as_deref().is_some_and(|value| {
                        !crate::prompt::python_strip(value).is_empty()
                            && !crate::prompt::python_strip(value).eq_ignore_ascii_case("all")
                    }) {
                        return Err(CliError::input("pass --amount or --all, not both"));
                    }
                    amount = Some("all".into());
                }
                amount = amount
                    .as_deref()
                    .map(|raw| normalize_root_amount(raw, NetUid::ROOT))
                    .transpose()?;
                if amount.is_none() && !interactive(ctx) {
                    return Err(CliError::input("missing required option: `--amount`"));
                }
                Ok(Self::Allocate { hotkey, amount })
            }
            "move" => {
                let origin = optional(leaf, "origin").map(str::to_owned);
                let dest = optional(leaf, "dest").map(str::to_owned);
                if !interactive(ctx) && (origin.is_none() || dest.is_none()) {
                    let missing = match (origin.is_none(), dest.is_none()) {
                        (true, true) => "--from` and `--to",
                        (true, false) => "--from",
                        _ => "--to",
                    };
                    return Err(missing_target(missing));
                }
                let owner = target(ctx, "coldkey_ss58", optional(leaf, "coldkey_ss58"), history)?;
                if origin.is_none() || dest.is_none() {
                    position_wallet(ctx, owner)?;
                }
                Ok(Self::Move {
                    origin,
                    dest,
                    owner,
                })
            }
            "swap" => {
                // Resolve wallet-relative hotkeys only after the signing wallet
                // is selected, as the source command does before its quote.
                ctx.confirm_wallet(
                    &crate::prompt::WalletConfirmation {
                        help: "Wallet whose coldkey signs this transaction.".into(),
                        require_coldkey: true,
                        ..Default::default()
                    },
                    history,
                )?;
                let hotkey = target(ctx, "hotkey_ss58", optional(leaf, "hotkey_ss58"), history)?;
                let origin = NetUid(
                    *leaf
                        .get_one::<u16>("origin_netuid")
                        .ok_or_else(|| CliError::input("missing --from"))?,
                );
                let dest = NetUid(
                    *leaf
                        .get_one::<u16>("dest_netuid")
                        .ok_or_else(|| CliError::input("missing --to"))?,
                );
                let amount = normalize_root_amount(
                    optional(leaf, "amount").ok_or_else(|| CliError::input("missing --amount"))?,
                    origin,
                )?;
                let max_slippage = *leaf
                    .get_one::<f64>("max_slippage")
                    .ok_or_else(|| CliError::input("missing max slippage"))?;
                let request = swap_request(hotkey, origin, dest, &amount, 0)?;
                request.intent()?;
                Ok(Self::Swap {
                    hotkey,
                    origin,
                    dest,
                    amount,
                    max_slippage,
                })
            }
            "register" => {
                TxRequest::from_matches(ctx, "root_register", leaf, history).map(Self::Transaction)
            }
            _ => Err(CliError::input(format!("unknown root command {name:?}"))),
        }
    }
}

fn local_wallets(ctx: &AppContext) -> Result<Vec<(String, AccountId)>, CliError> {
    let lookup = FsAccountLookup::new(ctx.globals.wallet_path.clone()).map_err(failure)?;
    let mut wallets: Vec<_> = lookup
        .candidates()
        .into_iter()
        .filter(|candidate| candidate.source == "wallet")
        .filter_map(|candidate| {
            candidate
                .address
                .parse()
                .ok()
                .map(|account| (candidate.name, account))
        })
        .collect();
    wallets.sort_by(|left, right| super::wallet::natural_cmp(&left.0, &right.0));
    Ok(wallets)
}
fn local_names(ctx: &AppContext) -> Result<Vec<(AccountId, String)>, CliError> {
    let lookup = FsAccountLookup::new(ctx.globals.wallet_path.clone()).map_err(failure)?;
    let mut candidates: Vec<_> = lookup
        .candidates()
        .into_iter()
        .filter(|candidate| candidate.source == "hotkey")
        .collect();
    // Preserve wallet groups when distinct spellings have the same natural key.
    let mut wallet_rank = BTreeMap::new();
    for candidate in &candidates {
        let wallet = candidate
            .name
            .rsplit_once('/')
            .map_or("", |(wallet, _)| wallet);
        let next = wallet_rank.len();
        wallet_rank.entry(wallet.to_owned()).or_insert(next);
    }
    // Python sorts wallets stably, then sorts each wallet's hotkeys stably.
    // Compare the components separately: a slash is not part of a wallet name.
    candidates.sort_by(|left, right| {
        let (left_wallet, left_hotkey) = left.name.rsplit_once('/').unwrap_or(("", &left.name));
        let (right_wallet, right_hotkey) = right.name.rsplit_once('/').unwrap_or(("", &right.name));
        super::wallet::natural_cmp(left_wallet, right_wallet)
            .then_with(|| {
                wallet_rank
                    .get(left_wallet)
                    .cmp(&wallet_rank.get(right_wallet))
            })
            .then_with(|| super::wallet::natural_cmp(left_hotkey, right_hotkey))
    });
    let mut names = Vec::new();
    let mut seen = BTreeSet::new();
    for candidate in candidates {
        if let Ok(account) = candidate.address.parse::<AccountId>()
            && seen.insert(account)
        {
            names.push((
                account,
                candidate
                    .name
                    .rsplit('/')
                    .next()
                    .unwrap_or(&candidate.name)
                    .to_owned(),
            ));
        }
    }
    // Contacts follow local hotkeys in file order; the first label wins.
    for entry in &ctx.books.addresses {
        if let (Some(address), Some(name)) = (entry["address"].as_str(), entry["name"].as_str())
            && !name.is_empty()
            && let Ok(account) = address.parse::<AccountId>()
            && seen.insert(account)
        {
            names.push((account, name.to_owned()));
        }
    }
    Ok(names)
}
async fn names(
    ctx: &AppContext,
    client: &Client,
    hotkeys: &[AccountId],
) -> Result<BTreeMap<AccountId, String>, CliError> {
    let mut names: BTreeMap<_, _> = local_names(ctx)?.into_iter().collect();
    let missing: Vec<_> = hotkeys
        .iter()
        .filter(|key| !names.contains_key(key))
        .copied()
        .collect();
    for (hotkey, identity) in owner::hotkey_identities(client, missing)
        .await
        .map_err(failure)?
    {
        if let Some(name) = identity["name"].as_str().filter(|name| !name.is_empty()) {
            names.entry(hotkey).or_insert_with(|| name.to_owned());
        }
    }
    for (hotkey, name) in &names {
        ctx.output
            .name_address(&hotkey.to_string(), name)
            .map_err(failure)?;
    }
    Ok(names)
}
fn casefold(value: &str) -> String {
    value
        .chars()
        .flat_map(|character| {
            if unicode_general_category::get_general_category(character)
                == unicode_general_category::GeneralCategory::Unassigned
            {
                character.to_string()
            } else {
                caseless::default_case_fold_str(&character.to_string())
            }
            .chars()
            .collect::<Vec<_>>()
        })
        .collect()
}
async fn selector(ctx: &AppContext, client: &Client, value: &str) -> Result<AccountId, CliError> {
    if let Ok(account) = value.parse::<AccountId>() {
        return Ok(account);
    }
    if let Some(uid) = rtao::primitives::pyfmt::parse_integer(value)
        .filter(|_| !value.is_empty() && value.chars().all(char::is_numeric))
    {
        let uid = uid
            .to_string()
            .parse::<u16>()
            .map_err(|_| failure(format!("no validator at root UID {value}")))?;
        let result = client
            .storage(&st::KEYS, (NetUid::ROOT, uid))
            .await
            .map_err(failure)?;
        // The descriptor has a Default modifier. Python's raw query returns
        // its all-zero account on a missing entry, rather than Option::None.
        return Ok(*result.raw());
    }
    let wanted = casefold(value);
    for (hotkey, name) in local_names(ctx)? {
        if casefold(&name) == wanted {
            return Ok(hotkey);
        }
    }
    let funds = m6_root::root_baskets(client).await.map_err(failure)?;
    // Selection accepts the chain name even when a different local alias
    // wins display precedence. Do not reuse display-name filtering here.
    let identities =
        owner::hotkey_identities(client, funds.iter().map(|fund| fund.hotkey).collect())
            .await
            .map_err(failure)?;
    // The identity read returns a lookup map; selection must retain the funds'
    // descending NAV order, including stable ties, rather than account-byte order.
    for fund in &funds {
        if let Some(name) = identities
            .get(&fund.hotkey)
            .and_then(|identity| identity["name"].as_str())
            .filter(|name| !name.is_empty())
            && casefold(name) == wanted
        {
            ctx.output
                .name_address(&fund.hotkey.to_string(), name)
                .map_err(failure)?;
            return Ok(fund.hotkey);
        }
    }
    Err(
        failure(format!("no root validator named {value:?}")).with_help(
            "pass a hotkey ss58, a root UID, an address-book name, or an on-chain identity name",
        ),
    )
}
fn label(hotkey: AccountId, names: &BTreeMap<AccountId, String>) -> String {
    names
        .get(&hotkey)
        .cloned()
        .unwrap_or_else(|| hotkey.to_string())
}
fn record_answer(history: &mut PromptHistory, flag: &str, hotkey: AccountId) {
    history.record_answers(vec![flag.into(), hotkey.to_string()]);
}
fn choose(
    ctx: &AppContext,
    hotkeys: &[AccountId],
    names: &BTreeMap<AccountId, String>,
    details: &BTreeMap<AccountId, String>,
    flag: &str,
    page: bool,
) -> Result<Option<AccountId>, CliError> {
    if hotkeys.is_empty() {
        return Ok(None);
    }
    let mut terminal = ctx.terminal.clone();
    let default_first = !page;
    if !ctx.prompt_context().interactive(&terminal) {
        return Err(missing_target(flag));
    }
    let mut shown = 0;
    let print_page = |terminal: &mut dyn Terminal, shown: &mut usize| -> Result<(), CliError> {
        let end = if page {
            shown.saturating_add(20).min(hotkeys.len())
        } else {
            hotkeys.len()
        };
        for (index, hotkey) in hotkeys.iter().enumerate().take(end).skip(*shown) {
            terminal
                .message(&format!(
                    "{}. {}{}",
                    index + 1,
                    if page {
                        ctx.output.account_text(
                            &hotkey.to_string(),
                            names.get(hotkey).map(String::as_str),
                        )
                    } else {
                        label(*hotkey, names)
                    },
                    details.get(hotkey).map_or("", String::as_str)
                ))
                .map_err(prompt_error)?;
        }
        *shown = end;
        if end < hotkeys.len() {
            terminal
                .message(&format!(
                    "Enter for {} more ({end} of {} shown)",
                    20.min(hotkeys.len() - end),
                    hotkeys.len()
                ))
                .map_err(prompt_error)?;
        }
        Ok(())
    };
    print_page(&mut terminal, &mut shown)?;
    if hotkeys.len() == 1 {
        terminal
            .message("only validator — selected")
            .map_err(prompt_error)?;
        return Ok(Some(hotkeys[0]));
    }
    let mut matchable: BTreeMap<_, _> = hotkeys
        .iter()
        .enumerate()
        .map(|(index, hotkey)| ((index + 1).to_string(), *hotkey))
        .collect();
    for hotkey in hotkeys {
        matchable.insert(hotkey.to_string(), *hotkey);
    }
    for hotkey in hotkeys {
        matchable.entry(label(*hotkey, names)).or_insert(*hotkey);
    }
    loop {
        let raw = terminal
            .read_line(&format!(
                "{flag}{}: ",
                if default_first { " [1]" } else { "" }
            ))
            .map_err(prompt_error)?;
        let raw = crate::prompt::python_strip(&raw);
        if raw.is_empty() {
            if default_first {
                return Ok(Some(hotkeys[0]));
            }
            if shown < hotkeys.len() {
                print_page(&mut terminal, &mut shown)?;
            } else {
                terminal
                    .message("end of list — enter a number, name, or hotkey")
                    .map_err(prompt_error)?;
            }
            continue;
        }
        if let Some(key) = matchable.get(raw) {
            return Ok(Some(*key));
        }
        // Python counts matching keys, including a label that is itself an
        // address prefix. Two keys remain ambiguous even for the same validator.
        let mut matching = matchable
            .iter()
            .filter(|(key, hotkey)| key.starts_with(raw) && hotkey.to_string().starts_with(raw));
        if let (Some((_, key)), None) = (matching.next(), matching.next()) {
            return Ok(Some(*key));
        }
        terminal
            .message("enter a list number, name, or hotkey")
            .map_err(prompt_error)?;
    }
}
async fn pick_position(
    ctx: &AppContext,
    client: &Client,
    positions: &[RootPosition],
    flag: &str,
    history: &mut PromptHistory,
) -> Result<Option<AccountId>, CliError> {
    let mut held = Vec::new();
    for row in positions {
        let total = row.total()?.rao;
        if total >= root_helpers::DUST_POSITION_RAO {
            held.push((total, row));
        }
    }
    held.sort_by_key(|(total, _)| std::cmp::Reverse(*total));
    if held.is_empty() {
        ctx.output
            .message("no root position above dust (τ0.001) for this wallet")
            .map_err(failure)?;
        return Ok(None);
    }
    let hotkeys: Vec<_> = held.iter().map(|(_, row)| row.hotkey).collect();
    let names = names(ctx, client, &hotkeys).await?;
    let mut details = BTreeMap::new();
    for (_, row) in &held {
        details.insert(
            row.hotkey,
            format!(
                "  total {} (staked {}, accrued {})",
                row.total()?,
                row.staked,
                row.accrued
            ),
        );
    }
    let result = choose(ctx, &hotkeys, &names, &details, flag, false)?;
    if let Some(key) = result {
        record_answer(history, flag, key);
    }
    Ok(result)
}
struct PickedFund {
    hotkey: AccountId,
    name: Option<String>,
    beta_price_tao: f64,
}
async fn pick_fund(
    ctx: &AppContext,
    client: &Client,
    exclude: Option<AccountId>,
    flag: &str,
    history: &mut PromptHistory,
) -> Result<Option<PickedFund>, CliError> {
    let mut funds = m6_root::root_baskets(client).await.map_err(failure)?;
    let normalized = basket_index::normalize_positions(
        &funds
            .iter()
            .map(basket_index::Fund::from)
            .collect::<Vec<_>>(),
    )
    .1;
    let details: BTreeMap<_, _> = funds
        .iter()
        .zip(normalized)
        .map(|(fund, fields)| {
            (
                fund.hotkey,
                format!(
                    "  rate {:.4} τ/β  vs index {:+.2}%  NAV {}",
                    fields.display_price_tao,
                    fields.vs_index * 100.0,
                    fund.nav_tao
                ),
            )
        })
        .collect();
    funds.retain(|row| Some(row.hotkey) != exclude);
    funds.sort_by_key(|row| std::cmp::Reverse(row.nav_tao.rao));
    if funds.is_empty() {
        ctx.output
            .message("no validator baskets found")
            .map_err(failure)?;
        return Ok(None);
    }
    let hotkeys: Vec<_> = funds.iter().map(|row| row.hotkey).collect();
    let names = names(ctx, client, &hotkeys).await?;
    let Some(hotkey) = choose(ctx, &hotkeys, &names, &details, flag, true)? else {
        return Ok(None);
    };
    let fund = funds
        .iter()
        .find(|fund| fund.hotkey == hotkey)
        .ok_or_else(|| failure("selected root fund is not in the picker"))?;
    record_answer(history, flag, hotkey);
    Ok(Some(PickedFund {
        hotkey,
        name: names.get(&hotkey).cloned(),
        beta_price_tao: fund.beta_price_tao,
    }))
}

fn tx(op: &'static str, args: Map<String, Value>) -> Result<TxRequest, CliError> {
    let request = TxRequest {
        op,
        args,
        period: None,
    };
    request.intent()?;
    Ok(request)
}
fn swap_request(
    hotkey: AccountId,
    origin: NetUid,
    dest: NetUid,
    amount: &str,
    min: u64,
) -> Result<TxRequest, CliError> {
    tx(
        "swap_basket",
        Map::from_iter([
            ("hotkey_ss58".into(), json!(hotkey)),
            ("origin_netuid".into(), json!(origin)),
            ("dest_netuid".into(), json!(dest)),
            ("amount".into(), json!(amount)),
            ("min_amount_out".into(), json!(Money::from_rao(min))),
        ]),
    )
}

// A bounded decimal coefficient, used only for the pinned Python Decimal
// context (28 significant digits, ties to even). Inputs are a u64 and one
// finite binary64 percentage, so the scratch vectors stay below 350 digits.
fn decimal_round(digits: &mut Vec<u8>, exponent: &mut i32) -> Result<(), CliError> {
    let first = digits
        .iter()
        .position(|digit| *digit != 0)
        .unwrap_or(digits.len());
    digits.drain(..first);
    if digits.len() <= 28 {
        return Ok(());
    }
    let round_up = digits[28] > 5
        || (digits[28] == 5
            && (digits[29..].iter().any(|digit| *digit != 0) || digits[27] % 2 == 1));
    *exponent += i32::try_from(digits.len() - 28).map_err(failure)?;
    digits.truncate(28);
    if round_up {
        let mut carry = true;
        for digit in digits.iter_mut().rev() {
            if *digit == 9 {
                *digit = 0;
            } else {
                *digit += 1;
                carry = false;
                break;
            }
        }
        if carry {
            digits.insert(0, 1);
            digits.pop();
            *exponent += 1;
        }
    }
    Ok(())
}

/// Python's `int(Decimal(expected) * (1 - Decimal(str(percent)) / 100))`,
/// including its 28-significant-digit context and ties-to-even rounding.
pub fn minimum_out(expected: u64, percent: f64) -> Result<u64, CliError> {
    if !percent.is_finite() || !(0.0..=100.0).contains(&percent) {
        return Err(CliError::input(
            "max slippage must be finite and within 0..100 percent",
        ));
    }
    if expected == 0 || percent == 0.0 {
        return Ok(expected);
    }
    let text = rtao::primitives::pyfmt::float_repr(percent);
    let (mantissa, exponent) = match text.split_once(['e', 'E']) {
        Some((mantissa, exponent)) => (mantissa, exponent.parse::<i32>().map_err(failure)?),
        None => (text.as_str(), 0),
    };
    let (whole, fraction) = mantissa.split_once('.').unwrap_or((mantissa, ""));
    let numerator: Vec<u8> = format!("{whole}{fraction}")
        .bytes()
        .map(|byte| byte - b'0')
        .collect();
    let scale = 2 + i32::try_from(fraction.len()).map_err(failure)? - exponent;
    // Percent <= 100 implies a nonnegative scale with float_repr's shortest
    // representation. Subtract the aligned coefficient from 10^scale.
    let scale = usize::try_from(scale).map_err(failure)?;
    let mut factor = vec![0u8; scale + 1];
    factor[0] = 1;
    let mut borrow = 0i16;
    for index in 0..factor.len() {
        let at = factor.len() - 1 - index;
        let digit = numerator
            .len()
            .checked_sub(index + 1)
            .map_or(0, |at| numerator[at]);
        let value = i16::from(factor[at]) - i16::from(digit) - borrow;
        factor[at] = u8::try_from(value.rem_euclid(10)).map_err(failure)?;
        borrow = i16::from(value < 0);
    }
    let mut factor_exponent = -i32::try_from(scale).map_err(failure)?;
    decimal_round(&mut factor, &mut factor_exponent)?;
    if factor.is_empty() {
        return Ok(0);
    }
    let amount: Vec<u8> = expected
        .to_string()
        .bytes()
        .map(|byte| byte - b'0')
        .collect();
    let mut cells = vec![0u16; amount.len() + factor.len()];
    for (left, a) in amount.iter().enumerate() {
        for (right, b) in factor.iter().enumerate() {
            cells[left + right + 1] += u16::from(*a) * u16::from(*b);
        }
    }
    for index in (1..cells.len()).rev() {
        let carry = cells[index] / 10;
        cells[index] %= 10;
        cells[index - 1] += carry;
    }
    let mut product = cells
        .into_iter()
        .map(|digit| u8::try_from(digit).map_err(failure))
        .collect::<Result<Vec<_>, _>>()?;
    decimal_round(&mut product, &mut factor_exponent)?;
    let integral_digits = i32::try_from(product.len()).map_err(failure)? + factor_exponent;
    if integral_digits <= 0 {
        return Ok(0);
    }
    let integral_digits = usize::try_from(integral_digits).map_err(failure)?;
    product.resize(product.len().max(integral_digits), 0);
    product[..integral_digits]
        .iter()
        .map(|digit| char::from(b'0' + *digit))
        .collect::<String>()
        .parse()
        .map_err(failure)
}
type PendingReview = (String, Vec<ReviewSection>);

fn short_hotkey(hotkey: AccountId) -> String {
    format!(
        "{}…",
        hotkey.to_string().chars().take(8).collect::<String>()
    )
}
fn signing_wallet_row(ctx: &AppContext) -> Option<Value> {
    if ctx.globals.wallet_name.is_empty() {
        return None;
    }
    Some(
        ctx.resolve_address("coldkey_ss58", None)
            .ok()
            .flatten()
            .map_or_else(|| json!(ctx.globals.wallet_name), |owner| json!(owner)),
    )
}
fn review(
    ctx: &AppContext,
    pending: &mut Option<PendingReview>,
    summary: String,
    title: &str,
    rows: Vec<(&str, Value)>,
) -> Result<(), CliError> {
    let names = ctx.output.address_names().map_err(failure)?;
    *pending = Some((
        summary,
        vec![ReviewSection {
            title: title.into(),
            rows: rows
                .into_iter()
                .map(|(key, value)| {
                    let value = match value {
                        Value::String(address) if names.contains_key(&address) => {
                            Value::String(format!("{} ({address})", names[&address]))
                        }
                        value => value,
                    };
                    (key.into(), value)
                })
                .collect(),
        }],
    ));
    Ok(())
}

/// A typed transaction and command display facts bound to its semantic intent.
pub struct PreparedSubmission {
    /// Actual SDK request; display context never enters its encoded arguments.
    pub request: TxRequest,
    /// Optional richer command summary and stages for preview and every review.
    pub review: Option<CommandReview>,
}

/// Prepare command facts without printing an early review card. The caller
/// forwards the bound context through SubmissionOptions at actual submission.
pub async fn prepare_submission(
    ctx: &mut AppContext,
    client: &Client,
    request: &Request,
    history: &mut PromptHistory,
) -> Result<Option<PreparedSubmission>, CliError> {
    let mut pending = None;
    let Some(request) =
        prepare_transaction_inner(ctx, client, request, history, &mut pending).await?
    else {
        return Ok(None);
    };
    let review = match pending {
        Some((summary, sections)) => Some(CommandReview::new(
            request.intent()?.as_dyn(),
            summary,
            sections,
        )),
        None => None,
    };
    Ok(Some(PreparedSubmission { request, review }))
}

/// Prepare an SDK transaction and render its command-only diagnostic card.
/// The actual entrypoint uses prepare_submission to retain these facts through
/// the shared preview, confirmation and saved-approval paths.
pub async fn prepare_transaction(
    ctx: &mut AppContext,
    client: &Client,
    request: &Request,
    history: &mut PromptHistory,
) -> Result<Option<TxRequest>, CliError> {
    let Some(prepared) = prepare_submission(ctx, client, request, history).await? else {
        return Ok(None);
    };
    if let Some(review) = &prepared.review {
        ctx.output
            .transaction_card(&review.sections)
            .map_err(failure)?;
    }
    Ok(Some(prepared.request))
}

async fn prepare_transaction_inner(
    ctx: &mut AppContext,
    client: &Client,
    request: &Request,
    history: &mut PromptHistory,
    pending: &mut Option<PendingReview>,
) -> Result<Option<TxRequest>, CliError> {
    match request {
        Request::Transaction(request) => Ok(Some(TxRequest {
            op: request.op,
            args: request.args.clone(),
            period: request.period,
        })),
        Request::Claim {
            hotkey,
            owner: account,
        } => {
            let positions = root_helpers::fetch_root_positions(client, *account).await?;
            let hotkey = match hotkey {
                Some(key) => *key,
                None => match pick_position(ctx, client, &positions, "--hotkey", history).await? {
                    Some(key) => key,
                    None => return Ok(None),
                },
            };
            let accrued = positions
                .iter()
                .find(|row| row.hotkey == hotkey)
                .map_or_else(
                    || DynBalance::from_rao(0, NetUid::ROOT),
                    |row| row.accrued.clone(),
                );
            if accrued.rao == 0 {
                return Err(failure(format!("no accrued yield to claim on {hotkey}")));
            }
            let fund = owner::validator_basket_summary(client, hotkey)
                .await
                .map_err(failure)?;
            let names = names(ctx, client, &[hotkey]).await?;
            let mut rows = vec![
                ("wallet", json!(account)),
                ("validator", json!(hotkey)),
                ("accrued", json!(accrued.to_string())),
            ];
            if fund.beta_price_tao > 0.0 {
                rows.push(("rate", json!(format!("{:.4} τ/β", fund.beta_price_tao))));
                rows.push((
                    "redeem",
                    json!(format!("~{:.4} β", accrued.amount() / fund.beta_price_tao)),
                ));
            }
            rows.push((
                "destination",
                json!(format!("root stake · {}", label(hotkey, &names))),
            ));
            rows.push((
                "note",
                json!("principal stays staked — withdraw TAO with rtao stake remove --netuid 0"),
            ));
            let label = names
                .get(&hotkey)
                .cloned()
                .unwrap_or_else(|| short_hotkey(hotkey));
            review(
                ctx,
                pending,
                format!("claim {accrued} into {label}'s root stake"),
                "Claim",
                rows,
            )?;
            tx(
                "claim_root_with_hotkey",
                Map::from_iter([("hotkey_ss58".into(), json!(hotkey))]),
            )
            .map(Some)
        }
        Request::Allocate { hotkey, amount } => {
            let (hotkey, picked) = match hotkey {
                Some(key) => (*key, None),
                None => match pick_fund(ctx, client, None, "--hotkey", history).await? {
                    Some(fund) => (fund.hotkey, Some(fund)),
                    None => return Ok(None),
                },
            };
            let amount = match amount {
                Some(amount) => amount.clone(),
                None => {
                    let mut terminal = ctx.terminal.clone();
                    let (normalized, raw) = ask_with(
                        &ctx.prompt_context(),
                        &mut terminal,
                        &PromptSpec::new("amount", "--amount"),
                        |raw| {
                            normalize_root_amount(raw, NetUid::ROOT)
                                .map_err(|error| PromptValidation::Message(error.to_string()))
                        },
                    )
                    .map_err(prompt_error)?;
                    // A leading minus is accepted through attached CLI syntax;
                    // preserve that spelling in replay without widening Clap.
                    history.record_answers(if raw.starts_with('-') {
                        vec![format!("--amount={raw}")]
                    } else {
                        vec!["--amount".into(), raw]
                    });
                    normalized
                }
            };
            let resolved_amount = Amount::parse(&amount)
                .and_then(|amount| amount.resolve(NetUid::ROOT, true))
                .map_err(|error| CliError::input(error.to_string()))?;
            let amount_display = match &resolved_amount {
                ResolvedAmount::All => "ALL free TAO".into(),
                ResolvedAmount::Exact(value) => value.to_string(),
            };
            let mut label = short_hotkey(hotkey);
            let mut summary_rate = None;
            let mut summary_receive = None;
            let mut rows = vec![
                ("validator", json!(hotkey)),
                ("amount", json!(amount_display)),
                (
                    "note",
                    json!(
                        "deploys this TAO into the fund's holdings and credits β at the value actually added — you bear entry slippage and swap fees"
                    ),
                ),
            ];
            // The interactive picker already holds Python's selected record.
            // Keep its name and raw mint rate without another display-only RPC.
            // Explicit targets retain their best-effort individual quote.
            let context = if let Some(fund) = picked {
                Some((fund.name, fund.beta_price_tao))
            } else if let Ok(summary) = owner::validator_basket_summary(client, hotkey).await {
                let name = names(ctx, client, &[hotkey])
                    .await
                    .ok()
                    .and_then(|names| names.get(&hotkey).cloned());
                Some((name, summary.beta_price_tao))
            } else {
                None
            };
            if let Some((name, beta_price_tao)) = context {
                if let Some(name) = name {
                    label = name.clone();
                    rows.insert(0, ("name", json!(name)));
                }
                if beta_price_tao > 0.0 {
                    summary_rate = Some(beta_price_tao);
                    rows.push(("rate", json!(format!("{beta_price_tao:.4} τ/β"))));
                    if let Ok(ResolvedAmount::Exact(amount)) =
                        Amount::parse(&amount).and_then(|amount| amount.resolve(NetUid::ROOT, true))
                    {
                        let receive = comma_fixed(amount.amount() / beta_price_tao, 4);
                        rows.push(("receive", json!(format!("~{receive} β"))));
                        summary_receive = Some(receive);
                    }
                }
            }
            ctx.confirm_wallet(
                &crate::prompt::WalletConfirmation {
                    help: "Wallet whose coldkey signs this transaction.".into(),
                    require_coldkey: true,
                    ..Default::default()
                },
                history,
            )?;
            if let Some(wallet) = signing_wallet_row(ctx) {
                rows.insert(0, ("wallet", wallet));
            }
            let mut summary = format!("allocate {amount_display} into {label}'s fund");
            if let Some(rate) = summary_rate {
                summary.push_str(&format!(" at {rate:.4} τ/β"));
                if let Some(receive) = summary_receive {
                    summary.push_str(&format!(" for ~{receive} β"));
                }
            }
            review(ctx, pending, summary, "Allocate", rows)?;
            tx(
                "stake_into_basket",
                Map::from_iter([
                    ("hotkey_ss58".into(), json!(hotkey)),
                    ("amount_tao".into(), json!(amount)),
                ]),
            )
            .map(Some)
        }
        Request::Move {
            origin,
            dest,
            owner: account,
        } => {
            let positions = root_helpers::fetch_root_positions(client, *account).await?;
            let origin = match origin {
                Some(value) => selector(ctx, client, value).await?,
                None => match pick_position(ctx, client, &positions, "--from", history).await? {
                    Some(key) => key,
                    None => return Ok(None),
                },
            };
            let (dest, picked_dest_name) = match dest {
                Some(value) => (selector(ctx, client, value).await?, None),
                None => match pick_fund(ctx, client, Some(origin), "--to", history).await? {
                    Some(fund) => (fund.hotkey, fund.name),
                    None => return Ok(None),
                },
            };
            if origin == dest {
                return Err(CliError::input(
                    "source and destination are the same validator",
                ));
            }
            let position = positions
                .iter()
                .find(|row| row.hotkey == origin)
                .ok_or_else(|| failure(format!("no root position to move on {origin}")))?;
            let total = position.total()?;
            if total.rao == 0 {
                return Err(failure(format!("no root position to move on {origin}")));
            }
            let labels = if let Some(name) = picked_dest_name {
                // The picker already resolved the destination label. Python
                // reads identities again only when an endpoint still needs one.
                let mut labels: BTreeMap<_, _> = local_names(ctx)?.into_iter().collect();
                if !labels.contains_key(&origin) {
                    labels = names(ctx, client, &[origin, dest]).await?;
                }
                // A required origin lookup must not replace the selected label.
                labels.insert(dest, name);
                labels
            } else {
                names(ctx, client, &[origin, dest]).await?
            };
            ctx.confirm_wallet(
                &crate::prompt::WalletConfirmation {
                    help: "Wallet whose coldkey signs this transaction.".into(),
                    require_coldkey: true,
                    ..Default::default()
                },
                history,
            )?;
            let origin_label = labels
                .get(&origin)
                .cloned()
                .unwrap_or_else(|| short_hotkey(origin));
            let dest_label = labels
                .get(&dest)
                .cloned()
                .unwrap_or_else(|| short_hotkey(dest));
            let mut summary = format!("move {total} from {origin_label} to {dest_label} on root");
            if position.accrued.rao > 0 {
                summary = format!("claim {} then {summary}", position.accrued);
            }
            review(
                ctx,
                pending,
                summary,
                "Move",
                vec![
                    ("wallet", json!(account)),
                    ("from", json!(origin)),
                    ("to", json!(dest)),
                    ("staked", json!(position.staked.to_string())),
                    ("accrued", json!(position.accrued.to_string())),
                    ("total", json!(total.to_string())),
                    (
                        "note",
                        json!(
                            "claims yield into root stake on the source, then moves the whole position to the destination — future yield accrues there"
                        ),
                    ),
                ],
            )?;
            tx(
                "move_stake",
                Map::from_iter([
                    ("origin_hotkey_ss58".into(), json!(origin)),
                    ("origin_netuid".into(), json!(0)),
                    ("dest_hotkey_ss58".into(), json!(dest)),
                    ("dest_netuid".into(), json!(0)),
                    ("amount_alpha".into(), json!("all")),
                    ("claim".into(), json!(true)),
                ]),
            )
            .map(Some)
        }
        Request::Swap {
            hotkey,
            origin,
            dest,
            amount,
            max_slippage,
        } => {
            let context = async {
                let holdings = m6_root::validator_basket(client, *hotkey)
                    .await
                    .map_err(failure)?;
                let status = m6_root::basket_trading_status(client, *hotkey)
                    .await
                    .map_err(failure)?;
                let sell = match Amount::parse(amount)
                    .and_then(|amount| amount.resolve(*origin, true))
                    .map_err(failure)?
                {
                    ResolvedAmount::Exact(amount) => Some(amount),
                    ResolvedAmount::All => holdings
                        .iter()
                        .find(|row| row.netuid == *origin)
                        .map(|row| row.alpha.clone()),
                };
                let expected = if let Some(sell) = sell.filter(|amount| amount.rao > 0) {
                    let quote = async {
                        let tao = if *origin == NetUid::ROOT {
                            sell
                        } else {
                            m6_staking::quote_unstake(client, *origin, sell.amount())
                                .await
                                .map_err(failure)?
                                .tao
                        };
                        if *dest == NetUid::ROOT {
                            Ok::<_, CliError>(tao)
                        } else {
                            Ok(m6_staking::quote_stake(client, *dest, tao.amount())
                                .await
                                .map_err(failure)?
                                .alpha)
                        }
                    }
                    .await
                    .ok();
                    quote.filter(|amount| amount.rao > 0)
                } else {
                    None
                };
                Ok::<_, CliError>((holdings, status, expected))
            }
            .await;
            let (holdings, status, expected) = match context {
                Ok((holdings, status, expected)) => (holdings, Some(status), expected),
                Err(_) => (Vec::new(), None, None),
            };
            let floor = expected
                .as_ref()
                .map(|expected| minimum_out(expected.rao, *max_slippage))
                .transpose()?
                .unwrap_or(0);
            let amount_display = match Amount::parse(amount)
                .and_then(|amount| amount.resolve(*origin, true))
                .map_err(|error| CliError::input(error.to_string()))?
            {
                ResolvedAmount::All => "the whole holding".into(),
                ResolvedAmount::Exact(value) => value.to_string(),
            };
            let mut rows = vec![
                ("validator", json!(hotkey)),
                ("sell", json!(format!("{amount_display} (netuid {origin})"))),
                ("buy", json!(format!("netuid {dest}"))),
            ];
            for (key, netuid) in [("origin holding", origin), ("dest holding", dest)] {
                if let Some(row) = holdings.iter().find(|row| row.netuid == *netuid) {
                    rows.push((key, json!(format!("{} worth {}", row.alpha, row.value_tao))));
                }
            }
            if let Some(expected) = expected {
                rows.push((
                    "expected out",
                    json!(format!("~{expected} (quoted now, after fees)")),
                ));
                rows.push((
                    "minimum out",
                    json!(format!(
                        "{} (expected less {}% max slippage; the trade rolls back below this)",
                        DynBalance::from_rao(floor, *dest),
                        rtao::primitives::pyfmt::fmt_g(*max_slippage)
                    )),
                ));
            } else {
                rows.push(("warning",json!("no quote available: minimum out is 0, so only the 2% protocol band bounds this trade")));
            }
            if let Some(status) = status {
                rows.push((
                    "turnover budget",
                    json!(format!(
                        "{} of {} available (refills {} per block)",
                        status.remaining_tao, status.budget_tao, status.refill_per_block_tao
                    )),
                ));
                if !status.enabled {
                    rows.push(("warning", json!("basket trading is disabled network-wide")));
                }
                if status.frozen {
                    rows.push(("warning", json!("trading is frozen for this validator")));
                }
            }
            rows.push(("note",json!("each leg must fill within 2% of both moving and spot price; TAO through the middle counts against the daily turnover budget")));
            if let Some(wallet) = signing_wallet_row(ctx) {
                rows.insert(0, ("wallet", wallet));
            }
            let summary = format!(
                "sell {amount_display} on netuid {origin} to buy netuid {dest} in {}'s fund",
                short_hotkey(*hotkey)
            );
            review(ctx, pending, summary, "Swap", rows)?;
            swap_request(*hotkey, *origin, *dest, amount, floor).map(Some)
        }
        Request::List { .. } => Err(CliError::input("root list does not prepare a transaction")),
    }
}

fn merge(record: &mut Value, fields: &impl serde::Serialize) -> Result<(), CliError> {
    let fields = serde_json::to_value(fields).map_err(failure)?;
    record
        .as_object_mut()
        .ok_or_else(|| failure("basket record is not an object"))?
        .extend(
            fields
                .as_object()
                .ok_or_else(|| failure("basket fields are not an object"))?
                .clone(),
        );
    Ok(())
}
fn table(
    output: &Output,
    title: &str,
    columns: &[&str],
    rows: Vec<Vec<Value>>,
    records: Value,
    footer: Option<String>,
) -> Result<(), CliError> {
    let mut spec = TableSpec::new(
        title,
        columns.iter().map(|column| (*column).into()).collect(),
        rows,
    );
    spec.legend = match title {
        "validator baskets vs basket index" => vec![
            ("stake (τ)", "Total TAO staked to this validator on root."),
            ("β (τ)", "What one TAO of root stake is worth, principal plus accrued yield, spliced onto the stake index at the fund's first sighting."),
            ("yield", "Root staker dividends since first sighting, valued at today's spot rate; allocating beta does not earn this dividend pipe."),
            ("yield /yr", "That yield annualized over the tracked period."),
            ("vs index", "Stake price against the average fund's total return."),
            ("nav (τ)", "Slippage-aware proceeds of fully redeeming the fund."),
            ("slippage", "Haircut between the spot NAV and full-redemption proceeds."),
            ("age (days)", "Days since value first entered the fund."),
        ],
        _ if title.starts_with("root positions") => vec![
            ("staked (τ)", "Your principal on root (netuid 0)."),
            ("accrued (τ)", "Unclaimed fund yield, quoted as realizable TAO."),
            ("return", "Accrued yield divided by principal since your last claim."),
            ("total (τ)", "Staked principal plus accrued yield."),
            ("β (τ)", "The index-spliced total value of one TAO of root stake."),
            ("vs index", "The stake price against the average fund's total return."),
        ],
        _ if title.starts_with("fund holdings") => vec![
            ("holding", "The fund's alpha balance on that subnet."),
            ("realizable (τ)", "TAO a full redemption would fetch, including slippage."),
            ("spot (τ)", "Rate times the holding, ignoring slippage."),
            ("lifetime", "NAV plus redeemed TAO, divided by all deposited TAO."),
            ("staker yield", "Beta entitlement minted per TAO of root stake since first sighting, valued at today's spot rate."),
        ],
        _ => vec![],
    }.into_iter().map(|(key,value)|(key.into(),value.into())).collect();
    spec.records = Some(records);
    spec.footer = footer;
    output.table(&spec).map_err(failure)
}

fn comma_fixed(value: f64, precision: usize) -> String {
    let text = format!("{value:.precision$}");
    let (whole, fraction) = text.split_once('.').unwrap_or((&text, ""));
    let mut grouped = String::new();
    for (index, character) in whole.chars().enumerate() {
        if index != 0 && (whole.len() - index) % 3 == 0 {
            grouped.push(',');
        }
        grouped.push(character);
    }
    if precision != 0 {
        grouped.push('.');
        grouped.push_str(fraction);
    }
    grouped
}
fn decimal_cell(record: &Value, key: &str, precision: usize, percent: bool, signed: bool) -> Value {
    json!(record[key].as_f64().map_or_else(
        || "—".into(),
        |value| {
            let value = if percent { value * 100.0 } else { value };
            let text = if signed {
                format!("{value:+.precision$}")
            } else {
                format!("{value:.precision$}")
            };
            format!("{text}{}", if percent { "%" } else { "" })
        }
    ))
}
fn dust_note(positions: &[RootPosition], show_dust: bool) -> Result<Option<String>, CliError> {
    if show_dust {
        return Ok(None);
    }
    let mut total = 0.0;
    let mut count = 0;
    for position in positions {
        if position.is_dust()? {
            count += 1;
            total += position.total()?.amount();
        }
    }
    Ok((count!=0).then(||format!("{count} dust position{} hidden (each < τ0.001, ≈ τ{total:.6} total) — pass --dust to show",if count==1 {""}else{"s"})))
}

async fn execute_list(
    ctx: &AppContext,
    client: &Client,
    request: &Request,
) -> Result<(), CliError> {
    let Request::List {
        validator,
        owner: account,
        wallets,
        mine,
        all_wallets,
        dust,
    } = request
    else {
        return Err(CliError::input("expected root list"));
    };
    if *mine {
        let positions = if *all_wallets {
            root_helpers::fetch_all_root_positions(client, wallets).await?
        } else {
            root_helpers::fetch_root_positions(
                client,
                account.ok_or_else(|| CliError::input("missing position owner"))?,
            )
            .await?
        };
        let hotkeys: Vec<_> = positions.iter().map(|row| row.hotkey).collect();
        let labels = names(ctx, client, &hotkeys).await?;
        let summaries = if positions.is_empty() {
            vec![]
        } else {
            m6_root::root_baskets(client).await.map_err(failure)?
        };
        let normalized = basket_index::normalize_positions(
            &summaries
                .iter()
                .map(basket_index::Fund::from)
                .collect::<Vec<_>>(),
        )
        .1;
        let funds: BTreeMap<_, _> = summaries
            .iter()
            .zip(normalized)
            .map(|(fund, fields)| (fund.hotkey, fields))
            .collect();
        let mut records = Vec::new();
        let mut shown = Vec::new();
        let mut total = 0u64;
        for position in &positions {
            let mut record = position.as_record()?;
            let fund = funds.get(&position.hotkey);
            record["name"] = labels
                .get(&position.hotkey)
                .map_or(Value::Null, |name| json!(name));
            record["price_tao"] = fund.map_or(Value::Null, |fund| json!(fund.display_price_tao));
            record["vs_index"] = fund.map_or(Value::Null, |fund| json!(fund.vs_index));
            record["index_provisional"] =
                fund.map_or(Value::Null, |fund| json!(fund.index_provisional));
            record["stake_value"] = fund.map_or(Value::Null, |fund| json!(fund.stake_value));
            record["stake_vs_index"] = fund.map_or(Value::Null, |fund| json!(fund.stake_vs_index));
            if !ctx.output.options().json_mode && (*dust || !position.is_dust()?) {
                total = total
                    .checked_add(position.total()?.rao)
                    .ok_or_else(|| failure("root position total exceeds native u64 rao"))?;
                let mut row = vec![json!(ctx.output.account_text(
                    &position.hotkey.to_string(),
                    labels.get(&position.hotkey).map(String::as_str)
                ))];
                if *all_wallets {
                    row.push(json!(position.wallet.as_deref().unwrap_or("—")));
                }
                row.extend([
                    json!(position.staked.to_string()),
                    json!(position.accrued.to_string()),
                    json!(
                        position
                            .return_pct()
                            .map_or_else(|| "—".into(), |value| format!("{:+.3}%", value * 100.0))
                    ),
                    json!(position.total()?.to_string()),
                    json!(fund.and_then(|fund| fund.stake_value).map_or_else(
                        || "—".into(),
                        |value| format!(
                            "{value:.4}{}",
                            if fund.is_some_and(|fund| fund.index_provisional) {
                                "*"
                            } else {
                                ""
                            }
                        )
                    )),
                    json!(
                        fund.and_then(|fund| fund.stake_vs_index)
                            .map_or_else(|| "—".into(), |value| format!("{:+.2}%", value * 100.0))
                    ),
                ]);
                shown.push(row);
            }
            records.push(record);
        }
        let title = if *all_wallets {
            "root positions (all wallets)".to_owned()
        } else {
            format!(
                "root positions of {}",
                account.ok_or_else(|| CliError::input("missing position owner"))?
            )
        };
        if ctx.output.options().json_mode {
            return ctx.output.value(&json!(records)).map_err(failure);
        }
        if positions.is_empty() {
            return ctx
                .output
                .message(&format!("{title}: none"))
                .map_err(failure);
        }
        let dust_note = dust_note(&positions, *dust)?;
        if shown.is_empty() {
            ctx.output
                .message(&format!("{title}: none above dust (τ0.001)"))
                .map_err(failure)?;
            if let Some(note) = dust_note {
                ctx.output.message(&note).map_err(failure)?;
            }
            return Ok(());
        }
        let mut columns = vec!["validator"];
        if *all_wallets {
            columns.push("wallet");
        }
        columns.extend([
            "staked (τ)",
            "accrued (τ)",
            "return",
            "total (τ)",
            "β (τ)",
            "vs index",
        ]);
        let index = funds
            .values()
            .next()
            .map_or_else(|| basket_index::index_level(None), |fund| fund.basket_index);
        table(
            &ctx.output,
            &title,
            &columns,
            shown,
            json!(records),
            Some(format!(
                "total {} · basket index {index:.4} (spot)",
                DynBalance::from_rao(total, NetUid::ROOT),
            )),
        )?;
        if let Some(note) = dust_note {
            ctx.output.message(&note).map_err(failure)?;
        }
        return Ok(());
    }
    if let Some(validator) = validator {
        let hotkey = selector(ctx, client, validator).await?;
        let positions = root_helpers::fetch_root_positions(
            client,
            account.ok_or_else(|| CliError::input("missing position owner"))?,
        )
        .await?;
        let summary = owner::validator_basket_summary(client, hotkey)
            .await
            .map_err(failure)?;
        if ctx.output.options().json_mode {
            return ctx.output.value(&json!(summary)).map_err(failure);
        }
        if let Some(position) = positions.iter().find(|row| row.hotkey == hotkey)
            && position.total()?.rao > 0
        {
            ctx.output
                .message(&format!(
                    "your position on {hotkey}: {} (staked {}, accrued {})",
                    position.total()?,
                    position.staked,
                    position.accrued
                ))
                .map_err(failure)?;
        }
        let rows = summary
            .holdings
            .iter()
            .map(|row| {
                vec![
                    json!(ctx.output.subnet_text(row.netuid, true)),
                    json!(row.alpha.to_string()),
                    json!(row.realizable_tao.to_string()),
                    json!(row.spot_tao.to_string()),
                ]
            })
            .collect();
        let mut footer = format!(
            "nav {} (spot {}) · deposited {}, redeemed {}",
            summary.nav_tao, summary.spot_nav_tao, summary.deposited_tao, summary.redeemed_tao
        );
        if let Some(value) = summary.lifetime_return {
            footer.push_str(&format!(" · lifetime {value:.4}x"));
        }
        if let Some(value) = basket_index::staker_yield(&basket_index::Fund::from(&summary)) {
            footer.push_str(&format!(" · staker yield {:.2}%", value * 100.0));
        }
        if summary.holdings.is_empty() {
            ctx.output.message(&footer).map_err(failure)?;
        } else {
            table(
                &ctx.output,
                &format!("fund holdings of {hotkey}"),
                &["subnet", "holding", "realizable (τ)", "spot (τ)"],
                rows,
                json!(summary.holdings),
                Some(footer),
            )?;
        }
        if validator != &hotkey.to_string() {
            // Upstream uses a separate stderr console for this canonical hint,
            // so it survives --quiet. JSON has already returned above. The
            // interpolated AccountId is canonical, with no raw selector text.
            ctx.output
                .sink()
                .write(
                    crate::output::Stream::Stderr,
                    format!("hint: rtao root list {hotkey}\n").as_bytes(),
                )
                .map_err(failure)?;
        }
        return Ok(());
    }
    let snapshot = client.at(BlockRef::Best).await.map_err(failure)?;
    let summaries = m6_root::root_baskets(&snapshot).await.map_err(failure)?;
    let mut plan = ReadPlan::new();
    for row in &summaries {
        plan.push(
            st::TOTAL_HOTKEY_ALPHA.surface(),
            vec![DynValue::Str(row.hotkey.to_string()), DynValue::Uint(0)],
        );
    }
    let stakes = snapshot.fetch(plan).await.map_err(failure)?;
    let (level, fields) = basket_index::normalize_positions(
        &summaries
            .iter()
            .map(basket_index::Fund::from)
            .collect::<Vec<_>>(),
    );
    let mut records = Vec::new();
    let mut stake_amounts = BTreeMap::new();
    for ((summary, fields), stake) in summaries.iter().zip(fields).zip(stakes) {
        let mut record = json!(summary);
        merge(&mut record, &fields)?;
        let raw = stake.to_json();
        let raw = raw
            .as_u64()
            .ok_or_else(|| failure("root stake is outside native u64 rao"))?;
        stake_amounts.insert(
            summary.hotkey,
            DynBalance::from_rao(raw, NetUid::ROOT).amount(),
        );
        record["stake_tao"] = json!(DynBalance::from_rao(raw, NetUid::ROOT));
        let age = basket_index::age_days(fields.index_first_block, snapshot.block().number);
        record["age_days"] = json!(age);
        let spot = summary.spot_nav_tao.amount();
        record["redemption_slippage"] =
            json!((spot > 0.0).then(|| 1.0 - summary.nav_tao.amount() / spot));
        record["yield_per_year"] = json!(
            fields
                .staker_yield
                .zip(age.filter(|age| *age != 0.0))
                .map(|(yield_, age)| yield_ / age * 365.0)
        );
        records.push((summary.nav_tao.rao, summary.hotkey, record));
    }
    records.sort_by_key(|(nav, _, _)| std::cmp::Reverse(*nav));
    let shown: Vec<_> = records
        .iter()
        .filter(|(nav, _, _)| *dust || *nav >= root_helpers::DUST_POSITION_RAO)
        .map(|(_, hotkey, _)| *hotkey)
        .collect();
    let names = names(ctx, client, &shown).await?;
    for (nav, hotkey, record) in &mut records {
        if *dust || *nav >= root_helpers::DUST_POSITION_RAO {
            record["name"] = names.get(hotkey).map_or(Value::Null, |name| json!(name));
        }
    }
    let payload = json!({"basket_index":level,"funds":records.iter().map(|(_,_,record)|record).collect::<Vec<_>>()});
    if ctx.output.options().json_mode {
        return ctx.output.value(&payload).map_err(failure);
    }
    if shown.is_empty() {
        return ctx
            .output
            .message("no funds above dust (τ0.001); pass --dust to show all")
            .map_err(failure);
    }
    let summary_by_key: BTreeMap<_, _> = summaries
        .iter()
        .map(|summary| (summary.hotkey, summary))
        .collect();
    let rows = records
        .iter()
        .filter(|(nav, _, _)| *dust || *nav >= root_helpers::DUST_POSITION_RAO)
        .map(|(_, hotkey, record)| {
            let summary = summary_by_key.get(hotkey);
            let stake_value = stake_amounts
                .get(hotkey)
                .map_or_else(|| "—".into(), |amount| comma_fixed(*amount, 4));
            let rate = decimal_cell(record, "stake_value", 4, false, false);
            let rate = if record["index_provisional"] == true && record["stake_value"].is_number() {
                json!(format!("{}*", rate.as_str().unwrap_or("—")))
            } else {
                rate
            };
            vec![
                json!(
                    ctx.output
                        .account_text(&hotkey.to_string(), names.get(hotkey).map(String::as_str))
                ),
                json!(stake_value),
                rate,
                decimal_cell(record, "staker_yield", 2, true, false),
                decimal_cell(record, "yield_per_year", 1, true, false),
                decimal_cell(record, "stake_vs_index", 2, true, true),
                json!(summary.map_or_else(
                    || "—".into(),
                    |summary| comma_fixed(summary.nav_tao.amount(), 4)
                )),
                decimal_cell(record, "redemption_slippage", 2, true, false),
                decimal_cell(record, "age_days", 1, false, false),
            ]
        })
        .collect();
    let stake_index = records
        .iter()
        .filter(|(nav, _, _)| *dust || *nav >= root_helpers::DUST_POSITION_RAO)
        .find_map(|(_, _, record)| record["stake_index"].as_f64().filter(|value| *value != 0.0));
    let mut footer =
        stake_index.map_or_else(|| "β index —".into(), |value| format!("β index {value:.4}"));
    footer.push_str(&format!(
        " · basket index {level:.4} (spot) · above index = out-earning the average fund"
    ));
    let hidden = records.len() - shown.len();
    if hidden != 0 {
        footer.push_str(&format!(" · {hidden} dust funds hidden (--dust to show)"));
    }
    footer.push_str(" · your positions: rtao root list --mine");
    table(
        &ctx.output,
        "validator baskets vs basket index",
        &[
            "validator",
            "stake (τ)",
            "β (τ)",
            "yield",
            "yield /yr",
            "vs index",
            "nav (τ)",
            "slippage",
            "age (days)",
        ],
        rows,
        payload,
        Some(footer),
    )?;
    if records.iter().any(|(nav, _, record)| {
        (*dust || *nav >= root_helpers::DUST_POSITION_RAO) && record["index_provisional"] == true
    }) {
        ctx.output.message("* fund not yet in the frozen baseline table; shown at the index level until the table is rebuilt").map_err(failure)?;
    }
    Ok(())
}

/// Execute reads through the connected client, or submit the real SDK intent.
pub async fn execute(
    ctx: &mut AppContext,
    client: &Client,
    request: &Request,
    options: SubmissionOptions<'_>,
    external: Option<&dyn ExternalSignerFactory>,
    history: &mut PromptHistory,
) -> Result<Option<ExtrinsicResult>, CliError> {
    if matches!(request, Request::List { .. }) {
        execute_list(ctx, client, request).await?;
        return Ok(None);
    }
    match prepare_submission(ctx, client, request, history).await? {
        Some(prepared) => {
            generated::execute_tx(
                ctx,
                client,
                &prepared.request,
                SubmissionOptions {
                    review: prepared.review.as_ref().or(options.review),
                    ..options
                },
                external,
                history,
            )
            .await
        }
        None => Ok(None),
    }
}
