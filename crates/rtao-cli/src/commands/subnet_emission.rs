// Derived from RaoFoundation/subtensor sdk/python/bittensor/cli/commands/subnet_emission.py @ 6a972693ae9580541225bd37696ac2fee26b8ae9 (Apache-2.0); modified by rtao.
//! `rtao subnets emission`: view and change how a subnet splits its alpha emission.
//!
//! The vector is `[burn, recycle, owner, validator, miner]`; owners move the
//! last three. `show` prints the vector, what root keeps, the allowed range of
//! every slot and when the owner may change it next. `set` writes the three
//! shares in one go; `set-value`, `increase` and `decrease` move one slot and
//! rescale the other two so the vector still sums to 100%. Every write is
//! checked against the chain's rules locally first, and a rejected change
//! names the rule and the allowed range, so nothing the chain would refuse is
//! signed.
//!
//! Exit codes: 0 success; 1 the subnet does not exist, the change is blocked
//! right now (owner rate limit) or the chain rejected it; 2 invalid input or a
//! vector the chain's rules would reject.

use crate::{
    context::{AppContext, CommandReview, ExternalSignerFactory, SubmissionOptions},
    error::CliError,
    globals::{Tier, with_globals},
    output::{DiagnosticContext, KvSection, ReviewSection},
    prompt::PromptHistory,
};
use clap::{Arg, ArgMatches, Command};
use rtao::{
    client::{BlockRef, Client},
    primitives::{NetUid, emission_weights as ew, pyfmt::fmt_grouped},
    reads::reads::subnets::{SubnetEmissionWeights, subnet_emission_weights},
    tx::ExtrinsicResult,
};
use rtao_intents::SetSubnetEmissionWeights;
use serde_json::{Value, json};

const EXIT_CODES: &str = "Exit codes: 0 done; 1 subnet missing, change blocked right now, or rejected by the chain; 2 invalid input or a vector the chain's rules would reject.";
const NETUID_HELP: &str = "Numeric identifier of the subnet.";
const SLOT_HELP: &str = "Slot to change: owner, validator or miner.";
const SHARE_HELP: &str = "A percentage (41%), a 0..1 fraction (0.41), or raw u64 parts.";

fn show_hint(netuid: NetUid) -> String {
    format!(
        "`rtao subnets emission show --netuid {netuid}` prints the vector and every allowed range"
    )
}

/// Which slot each chain error is about; `None` for whole-vector rules.
fn error_slot(error: &str) -> Option<&'static str> {
    match error {
        "BurnEmissionMustBeZero" => Some("burn"),
        "RecycleEmissionMustBeZero" => Some("recycle"),
        "OwnerEmissionAboveMaximum" | "LeasedSubnetOwnerCutBelowMinimum" => Some("owner"),
        "ValidatorEmissionAboveMaximum" | "ValidatorEmissionTooLowToPreserveRoot" => {
            Some("validator")
        }
        "MinerEmissionBelowMinimum" => Some("miner"),
        _ => None,
    }
}

fn netuid_arg() -> Arg {
    Arg::new("netuid")
        .long("netuid")
        .required(true)
        .value_parser(clap::value_parser!(u16))
        .help(NETUID_HELP)
}

fn text_arg(id: &'static str, help: String) -> Arg {
    Arg::new(id).long(id).required(true).help(help)
}

// py: sdk/python/bittensor/cli/commands/subnet_emission.py:36-50
/// The `subnets emission` command group.
pub fn command() -> Command {
    Command::new("emission")
        .about(
            "Subnet emission weights: how a subnet splits its alpha emission between the owner, validators and miners.",
        )
        .long_about(format!(
            "Subnet emission weights: how a subnet splits its alpha emission between the owner, validators and miners.\n\nOnly the subnet owner coldkey can change the vector, once every 50,400 blocks (about 7 days) per subnet. Root is paid its baseline share of emission off the top of the validator slot every block, whatever the subnet sets. Add --dry-run to any write to preview the before/after vector and what root and validators keep; --json for machine-readable output.\n\n{EXIT_CODES}\n\nGuide: {}/guides/subnets",
            rtao_catalog::py::DOCS_URL
        ))
        .subcommand_required(true)
        .arg_required_else_help(true)
        .subcommand(with_globals(
            Command::new("show")
                .about("Show the emission split, what root keeps, the allowed ranges and the next change window.")
                .after_help("Example: rtao subnets emission show --netuid 4\n\nJSON fields: netuid, block, weights (u64 parts), shares (0..1), global_tao_weight, root_share, baseline_root_share, validator_share_after_root, root_capped, root_tao, alpha_issuance (rao), min_validator (parts), created_via_lease, limits{slot: min, max, min_parts, max_parts, why}, rate_limit{limit_blocks, last_change_block, next_allowed_block, blocks_until_allowed, owner_can_change_now}.")
                .arg(netuid_arg()),
            Tier::Read,
        ))
        .subcommand(with_globals(
            Command::new("set")
                .about("Set the owner / validator / miner split in one go (burn and recycle stay 0).")
                .long_about("Set the owner / validator / miner split in one go (burn and recycle stay 0).\n\nThe three shares must add up to 100%; rounding dust of a few parts lands in the miner slot. Root stakers always keep their baseline share: it is paid off the top of the validator slot every block, so the validator share must stay above it (a share at or below the baseline is rejected) and alpha validators share what is left.")
                .after_help(format!("Example: rtao subnets emission set --netuid 4 --owner 18% --validator 20% --miner 62% --dry-run\n\n{EXIT_CODES}"))
                .arg(netuid_arg())
                .arg(text_arg("owner", format!("Owner share. {SHARE_HELP}")))
                .arg(text_arg("validator", format!("Validator share. {SHARE_HELP}")))
                .arg(text_arg("miner", format!("Miner share. {SHARE_HELP}"))),
            Tier::Tx,
        ))
        .subcommand(with_globals(
            Command::new("set-value")
                .about("Set one slot; the other two are rescaled so the vector still sums to 100%.")
                .long_about("Set one slot; the other two are rescaled so the vector still sums to 100%.\n\nThe untouched slots keep their ratio: lowering owner from 18% to 10% hands the 8% to validators and miners in their current 41:41 proportion.")
                .after_help(format!("Example: rtao subnets emission set-value --netuid 4 --slot validator --value 20% --dry-run\n\n{EXIT_CODES}"))
                .arg(netuid_arg())
                .arg(text_arg("slot", SLOT_HELP.into()))
                .arg(text_arg("value", format!("New share for the slot. {SHARE_HELP}"))),
            Tier::Tx,
        ))
        .subcommand(with_globals(
            Command::new("increase")
                .about("Raise one slot by an amount; the other two are rescaled to fill 100%.")
                .after_help(format!("Example: rtao subnets emission increase --netuid 4 --slot miner --by 5%\n\n{EXIT_CODES}"))
                .arg(netuid_arg())
                .arg(text_arg("slot", SLOT_HELP.into()))
                .arg(text_arg("by", format!("Amount to add to the slot. {SHARE_HELP}"))),
            Tier::Tx,
        ))
        .subcommand(with_globals(
            Command::new("decrease")
                .about("Lower one slot by an amount; the other two are rescaled to fill 100%.")
                .after_help(format!("Example: rtao subnets emission decrease --netuid 4 --slot validator --by 10%\n\n{EXIT_CODES}"))
                .arg(netuid_arg())
                .arg(text_arg("slot", SLOT_HELP.into()))
                .arg(text_arg("by", format!("Amount to take from the slot. {SHARE_HELP}"))),
            Tier::Tx,
        ))
}

/// Command-owned identifiers to reserve from leaf-global overrides.
pub fn reserved(matches: &ArgMatches) -> Result<Vec<&'static str>, CliError> {
    match matches.subcommand_name() {
        Some("show") => Ok(vec!["netuid"]),
        Some("set") => Ok(vec!["netuid", "owner", "validator", "miner"]),
        Some("set-value") => Ok(vec!["netuid", "slot", "value"]),
        Some("increase" | "decrease") => Ok(vec!["netuid", "slot", "by"]),
        _ => Err(CliError::input("missing subnets emission command")),
    }
}

/// One parsed `subnets emission` invocation; shares are read after the vector.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Request {
    /// Print the vector, root's shares, the allowed ranges and the window.
    Show {
        /// Subnet to inspect.
        netuid: NetUid,
    },
    /// Write the owner/validator/miner split.
    Set {
        /// Subnet to configure.
        netuid: NetUid,
        /// Owner share text.
        owner: String,
        /// Validator share text.
        validator: String,
        /// Miner share text.
        miner: String,
    },
    /// Set one slot and rescale the others.
    SetValue {
        /// Subnet to configure.
        netuid: NetUid,
        /// Slot name or uid.
        slot: String,
        /// New share text.
        value: String,
    },
    /// Move one slot by an amount and rescale the others.
    Adjust {
        /// Subnet to configure.
        netuid: NetUid,
        /// Slot name or uid.
        slot: String,
        /// Amount text.
        by: String,
        /// Whether the amount is added rather than taken.
        increase: bool,
    },
}

impl Request {
    /// Parse one leaf; share and slot text are validated after the chain read,
    /// in Python's order.
    pub fn from_matches(matches: &ArgMatches) -> Result<Self, CliError> {
        let (name, leaf) = matches
            .subcommand()
            .ok_or_else(|| CliError::input("missing subnets emission command"))?;
        let netuid = NetUid(
            *leaf
                .get_one::<u16>("netuid")
                .ok_or_else(|| CliError::input("missing --netuid"))?,
        );
        let text = |id: &str| {
            leaf.get_one::<String>(id)
                .cloned()
                .ok_or_else(|| CliError::input(format!("missing --{id}")))
        };
        Ok(match name {
            "show" => Self::Show { netuid },
            "set" => Self::Set {
                netuid,
                owner: text("owner")?,
                validator: text("validator")?,
                miner: text("miner")?,
            },
            "set-value" => Self::SetValue {
                netuid,
                slot: text("slot")?,
                value: text("value")?,
            },
            "increase" | "decrease" => Self::Adjust {
                netuid,
                slot: text("slot")?,
                by: text("by")?,
                increase: name == "increase",
            },
            _ => {
                return Err(CliError::input(format!(
                    "unknown subnets emission command {name:?}"
                )));
            }
        })
    }

    /// Whether this invocation signs a transaction.
    pub fn is_write(&self) -> bool {
        !matches!(self, Self::Show { .. })
    }
}

/// Print a diagnostic with Python's note and help, then exit with `code`.
fn report(
    ctx: &AppContext,
    code: u8,
    text: &str,
    note: Option<&str>,
    help: Option<&str>,
) -> CliError {
    if let Err(error) = ctx.output.error(
        text,
        DiagnosticContext {
            note,
            help,
            see: None,
        },
    ) {
        return CliError::failure(error.to_string());
    }
    // The diagnostic is already rendered; only the status remains.
    CliError {
        code,
        ..CliError::failure(String::new())
    }
}

// py: sdk/python/bittensor/cli/commands/subnet_emission.py:76-84
async fn fetch(
    ctx: &AppContext,
    client: &Client,
    netuid: NetUid,
) -> Result<SubnetEmissionWeights, CliError> {
    let view = client.at(BlockRef::Best).await.map_err(CliError::from)?;
    subnet_emission_weights(&view, netuid)
        .await
        .map_err(CliError::from)?
        .ok_or_else(|| {
            report(
                ctx,
                1,
                &format!("subnet {netuid} does not exist or has no emission vector"),
                None,
                Some("the root subnet (netuid 0) has no alpha emission to divide"),
            )
        })
}

// py: sdk/python/bittensor/cli/commands/subnet_emission.py:87-105
/// The stable machine-readable shape of `show`.
pub fn record(info: &SubnetEmissionWeights) -> Value {
    json!({
        "netuid": info.netuid,
        "block": info.block,
        "weights": info.weights,
        "shares": info.shares,
        "global_tao_weight": info.global_tao_weight,
        "root_share": info.root_share,
        "baseline_root_share": info.baseline_root_share,
        "validator_share_after_root": info.validator_share_after_root,
        "root_capped": info.root_capped,
        "root_tao": info.root_tao,
        "alpha_issuance": info.alpha_issuance,
        "min_validator": info.min_validator,
        "created_via_lease": info.created_via_lease,
        "limits": info.rules().limits(),
        "rate_limit": info.rate_limit(),
    })
}

fn pct4(value: f64) -> String {
    format!("{:.4}%", value * 100.0)
}

// Python's `f"{x:,.0f}"`: round the binary value half to even, then group.
fn grouped_whole(value: f64) -> String {
    let text = format!("{value:.0}");
    match text.strip_prefix('-') {
        Some(digits) => format!("-{}", group_digits(digits)),
        None => group_digits(&text),
    }
}

fn group_digits(digits: &str) -> String {
    digits
        .parse::<u128>()
        .map_or_else(|_| digits.to_owned(), fmt_grouped)
}

// py: sdk/python/bittensor/cli/commands/subnet_emission.py:108-110
fn hours(blocks: u64) -> String {
    let hours = blocks as f64 * ew::BLOCK_SECONDS as f64 / 3600.0;
    if hours >= 48.0 {
        format!("{:.1} d", hours / 24.0)
    } else {
        format!("{hours:.1} h")
    }
}

// py: sdk/python/bittensor/cli/commands/subnet_emission.py:113-122
fn next_change(rate: &ew::RateLimitStatus) -> (String, String) {
    if rate.owner_can_change_now {
        let why = if rate.last_change_block == 0 {
            "first change"
        } else {
            "the weekly window has passed"
        };
        return (
            "now".into(),
            format!(
                "{why}; owners may change once per {} blocks",
                fmt_grouped(u128::from(rate.limit_blocks))
            ),
        );
    }
    (
        format!("block {}", fmt_grouped(u128::from(rate.next_allowed_block))),
        format!(
            "in {} blocks (~{}); last changed at block {}; root is exempt",
            fmt_grouped(u128::from(rate.blocks_until_allowed)),
            hours(rate.blocks_until_allowed),
            fmt_grouped(u128::from(rate.last_change_block))
        ),
    )
}

fn ranges(rules: &ew::Rules) -> Vec<(&'static str, String, String)> {
    rules.table()
}

fn range(rules: &ew::Rules, slot: &str) -> Option<(String, String)> {
    ranges(rules)
        .into_iter()
        .find(|(name, _, _)| *name == slot)
        .map(|(_, rule, why)| (rule, why))
}

// py: sdk/python/bittensor/cli/commands/subnet_emission.py:130-135
fn parse_share(ctx: &AppContext, raw: &str, flag: &str) -> Result<u64, CliError> {
    ew::parse_share(raw).map_err(|error| {
        report(
            ctx,
            2,
            &format!("invalid value for `{flag}`: {error}"),
            None,
            None,
        )
    })
}

// py: sdk/python/bittensor/cli/commands/subnet_emission.py:138-150
fn parse_slot(ctx: &AppContext, raw: &str) -> Result<usize, CliError> {
    let slot = ew::slot_index(raw)
        .map_err(|error| report(ctx, 2, &error.to_string(), None, Some(SLOT_HELP)))?;
    if !ew::ADJUSTABLE_SLOTS.contains(&slot) {
        return Err(report(
            ctx,
            2,
            &format!(
                "{} (slot {slot}) is reserved and pinned to 0 in this runtime",
                ew::SLOT_NAMES[slot]
            ),
            None,
            Some(SLOT_HELP),
        ));
    }
    Ok(slot)
}

// py: sdk/python/bittensor/cli/commands/subnet_emission.py:153-175
fn reject(
    ctx: &AppContext,
    info: &SubnetEmissionWeights,
    proposed: &ew::Weights,
) -> Result<(), CliError> {
    let rules = info.rules();
    let violations = ew::validate(proposed, &rules);
    if violations.is_empty() {
        return Ok(());
    }
    let mut allowed: Vec<(&'static str, String, String)> = Vec::new();
    for violation in &violations {
        if let Some(slot) = error_slot(violation.error)
            && !allowed.iter().any(|(known, _, _)| *known == slot)
            && let Some((rule, why)) = range(&rules, slot)
        {
            allowed.push((slot, rule, why));
        }
    }
    let text = if let [violation] = violations.as_slice() {
        format!("the chain would reject this change: {violation}")
    } else {
        format!(
            "the chain would reject this change:\n{}",
            violations
                .iter()
                .map(|violation| format!("  - {violation}"))
                .collect::<Vec<_>>()
                .join("\n")
        )
    };
    let note = (!allowed.is_empty()).then(|| {
        format!(
            "allowed on subnet {} right now: {}",
            info.netuid,
            allowed
                .iter()
                .map(|(slot, rule, why)| format!("{slot} {rule} ({why})"))
                .collect::<Vec<_>>()
                .join("; ")
        )
    });
    Err(report(
        ctx,
        2,
        &text,
        note.as_deref(),
        Some(&show_hint(info.netuid)),
    ))
}

// py: sdk/python/bittensor/cli/commands/subnet_emission.py:178-225
/// Review-card rows: each moving slot, what root keeps, what validators are left.
pub fn card(info: &SubnetEmissionWeights, preview: &ew::Preview) -> Vec<(String, Value)> {
    let (before, after) = (&preview.before.weights, &preview.after.weights);
    let mut rows = Vec::new();
    for slot in ew::ADJUSTABLE_SLOTS {
        let mut value = ew::format_share(after[slot], 2);
        if after[slot] != before[slot] {
            value = format!("{} -> {value}", ew::format_share(before[slot], 2));
        }
        rows.push((ew::SLOT_NAMES[slot].to_owned(), json!(value)));
    }
    let root = &preview.root;
    rows.push((
        "root share of emission".into(),
        json!(format!(
            "{} -> {} (baseline {}, paid off the top)",
            pct4(root.share_before),
            root.share_after.map_or_else(|| "rejected".into(), pct4),
            pct4(root.baseline_share)
        )),
    ));
    rows.push((
        "validators after root".into(),
        json!(format!(
            "{} -> {}",
            pct4(root.validators_before),
            root.validators_after.map_or_else(
                || "rejected".into(),
                |after| format!(
                    "{}{}",
                    pct4(after),
                    if root.capped_after {
                        " (capped: root takes the whole slot)"
                    } else {
                        ""
                    }
                )
            )
        )),
    ));
    let rate = info.rate_limit();
    rows.push((
        "next owner change after this".into(),
        json!(format!(
            "about block {} ({} blocks after the one that includes this call)",
            fmt_grouped(u128::from(info.block) + u128::from(rate.limit_blocks)),
            fmt_grouped(u128::from(rate.limit_blocks))
        )),
    ));
    rows
}

// py: sdk/python/bittensor/cli/commands/subnet_emission.py:228-251
#[allow(clippy::too_many_arguments)]
async fn submit(
    ctx: &mut AppContext,
    client: &Client,
    info: &SubnetEmissionWeights,
    proposed: &ew::Weights,
    slots: &[usize],
    renormalize: bool,
    options: SubmissionOptions<'_>,
    external: Option<&dyn ExternalSignerFactory>,
    history: &mut PromptHistory,
) -> Result<Option<ExtrinsicResult>, CliError> {
    reject(ctx, info, proposed)?;
    let intent = SetSubnetEmissionWeights {
        netuid: info.netuid,
        slots: slots.iter().map(|slot| json!(slot)).collect(),
        values: slots.iter().map(|slot| json!(proposed[*slot])).collect(),
        renormalize,
    };
    rtao::tx::Intent::validate(&intent).map_err(|error| CliError::input(error.to_string()))?;
    let preview = ew::preview(&info.weights, proposed, &info.root_inputs());
    let review = CommandReview::new(
        &intent,
        rtao::tx::Intent::summary(&intent),
        vec![ReviewSection {
            title: "Emission weights".into(),
            rows: card(info, &preview),
        }],
    );
    ctx.submit_with_options(
        client,
        &intent,
        SubmissionOptions {
            review: Some(&review),
            ..options
        },
        external,
        history,
    )
    .await
}

// py: sdk/python/bittensor/cli/commands/subnet_emission.py:254-262
fn renormalized(
    ctx: &AppContext,
    info: &SubnetEmissionWeights,
    slot: usize,
    parts: u64,
) -> Result<ew::Weights, CliError> {
    ew::renormalize(&info.weights, &[(slot, parts)]).map_err(|error| {
        report(
            ctx,
            2,
            &error.to_string(),
            None,
            Some(&show_hint(info.netuid)),
        )
    })
}

// py: sdk/python/bittensor/cli/commands/subnet_emission.py:265-350
fn show(ctx: &AppContext, info: &SubnetEmissionWeights) -> Result<(), CliError> {
    let netuid = info.netuid;
    let mut vector = Vec::new();
    for slot in ew::ADJUSTABLE_SLOTS {
        vector.push((
            ew::SLOT_NAMES[slot].to_owned(),
            ew::format_share(info.weights[slot], 2),
            Some(format!("{} parts", info.weights[slot])),
        ));
    }
    for slot in [ew::BURN, ew::RECYCLE] {
        vector.push((
            ew::SLOT_NAMES[slot].to_owned(),
            ew::format_share(info.weights[slot], 2),
            Some("reserved, pinned to 0".to_owned()),
        ));
    }
    let (next_value, next_note) = next_change(&info.rate_limit());
    let root = vec![
        (
            "root share of emission".to_owned(),
            pct4(info.root_share),
            Some(if info.root_capped {
                "the baseline, capped at the validator slot: root takes the whole slot".to_owned()
            } else {
                "what root TAO stakers earn from this subnet now, paid off the top of the validator slot".to_owned()
            }),
        ),
        (
            "root baseline share".to_owned(),
            pct4(info.baseline_root_share),
            Some(format!(
                "with the default validator share and the global TaoWeight ({:.2}%); recomputed every block, so it moves with root stake and alpha issuance, not with this vector",
                info.global_tao_weight * 100.0
            )),
        ),
        (
            "validators after root".to_owned(),
            pct4(info.validator_share_after_root),
            Some(if info.root_capped {
                "nothing: raise the validator share above the baseline to pay validators".to_owned()
            } else {
                "what alpha validators share once root is paid".to_owned()
            }),
        ),
        (
            "validator floor".to_owned(),
            format!("> {}", pct4(info.baseline_root_share)),
            Some("a change must leave the validator share above the baseline".to_owned()),
        ),
        (
            "root tao".to_owned(),
            format!("{} TAO", grouped_whole(info.root_tao as f64 / 1e9)),
            None,
        ),
        (
            "alpha issuance".to_owned(),
            format!("{} alpha", grouped_whole(info.alpha_issuance as f64 / 1e9)),
            None,
        ),
        ("next owner change".to_owned(), next_value, Some(next_note)),
        (
            "created via lease".to_owned(),
            if info.created_via_lease { "yes" } else { "no" }.to_owned(),
            None,
        ),
    ];
    let table = ranges(&info.rules());
    let allowed = ew::ADJUSTABLE_SLOTS
        .into_iter()
        .map(|slot| ew::SLOT_NAMES[slot])
        .chain(["burn", "recycle", "sum"])
        .filter_map(|name| {
            table
                .iter()
                .find(|(known, _, _)| *known == name)
                .map(|(name, rule, why)| ((*name).to_owned(), rule.clone(), Some(why.clone())))
        })
        .collect();
    ctx.output
        .kv_sections(
            Some(&format!("subnet {netuid} emission weights")),
            &[
                KvSection {
                    header: Some("Vector".into()),
                    rows: vector,
                },
                KvSection {
                    header: Some("Root".into()),
                    rows: root,
                },
                KvSection {
                    header: Some("Allowed".into()),
                    rows: allowed,
                },
            ],
            &record(info),
            Some(&format!(
                "change one slot with `rtao subnets emission set-value --netuid {netuid} --slot owner --value 10%` (the other two rescale to fill 100%); add --dry-run to preview"
            )),
        )
        .map_err(|error| CliError::failure(error.to_string()))
}

/// Read the vector, then show it or check and submit a change.
pub async fn execute(
    ctx: &mut AppContext,
    client: &Client,
    request: &Request,
    options: SubmissionOptions<'_>,
    external: Option<&dyn ExternalSignerFactory>,
    history: &mut PromptHistory,
) -> Result<Option<ExtrinsicResult>, CliError> {
    match request {
        Request::Show { netuid } => {
            let info = fetch(ctx, client, *netuid).await?;
            show(ctx, &info)?;
            Ok(None)
        }
        // py: sdk/python/bittensor/cli/commands/subnet_emission.py:353-391
        Request::Set {
            netuid,
            owner,
            validator,
            miner,
        } => {
            let info = fetch(ctx, client, *netuid).await?;
            let owner = parse_share(ctx, owner, "--owner")?;
            let validator = parse_share(ctx, validator, "--validator")?;
            let miner = parse_share(ctx, miner, "--miner")?;
            let total = u128::from(owner) + u128::from(validator) + u128::from(miner);
            let gap = i128::from(ew::ONE) - i128::try_from(total).unwrap_or(i128::MAX);
            let miner = if gap != 0 && gap.abs() <= 1_000 {
                // Decimal shares rarely hit u64::MAX exactly; the miner slot
                // soaks up the dust.
                u64::try_from(i128::from(miner) + gap).map_err(|_| {
                    report(
                        ctx,
                        2,
                        "the miner share cannot absorb the rounding dust",
                        None,
                        None,
                    )
                })?
            } else if gap != 0 {
                return Err(report(
                    ctx,
                    2,
                    &format!(
                        "owner + validator + miner must add up to 100%, not {}",
                        ew::format_share(total, 4)
                    ),
                    None,
                    Some(
                        "give all three shares, or use set-value to move one slot and rescale the rest",
                    ),
                ));
            } else {
                miner
            };
            let proposed = [0, 0, owner, validator, miner];
            let slots: Vec<usize> = (0..ew::SLOT_NAMES.len()).collect();
            submit(
                ctx, client, &info, &proposed, &slots, false, options, external, history,
            )
            .await
        }
        // py: sdk/python/bittensor/cli/commands/subnet_emission.py:394-418
        Request::SetValue {
            netuid,
            slot,
            value,
        } => {
            let info = fetch(ctx, client, *netuid).await?;
            let slot = parse_slot(ctx, slot)?;
            let parts = parse_share(ctx, value, "--value")?;
            let proposed = renormalized(ctx, &info, slot, parts)?;
            submit(
                ctx,
                client,
                &info,
                &proposed,
                &[slot],
                true,
                options,
                external,
                history,
            )
            .await
        }
        // py: sdk/python/bittensor/cli/commands/subnet_emission.py:421-448
        Request::Adjust {
            netuid,
            slot,
            by,
            increase,
        } => {
            let info = fetch(ctx, client, *netuid).await?;
            let slot = parse_slot(ctx, slot)?;
            let delta = parse_share(ctx, by, "--by")?;
            let current = info.weights[slot];
            let parts = if *increase {
                current.saturating_add(delta)
            } else if delta > current {
                return Err(report(
                    ctx,
                    2,
                    &format!(
                        "{} is only {}; cannot take {} from it",
                        ew::SLOT_NAMES[slot],
                        ew::format_share(current, 2),
                        ew::format_share(delta, 2)
                    ),
                    None,
                    Some(&show_hint(info.netuid)),
                ));
            } else {
                current - delta
            };
            let proposed = renormalized(ctx, &info, slot, parts)?;
            submit(
                ctx,
                client,
                &info,
                &proposed,
                &[slot],
                true,
                options,
                external,
                history,
            )
            .await
        }
    }
}
