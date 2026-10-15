// Derived from RaoFoundation/subtensor sdk/python/bittensor/cli/stake_exit.py @ 850f2755bb05a3851f948489f0ab27b0645cebce (Apache-2.0); modified by rtao.
//! Single-hotkey exits and atomic sweeps of discovered live positions.

use super::{failure, generated, picker};
use crate::{
    context::{AppContext, ExternalSignerFactory, SubmissionOptions},
    error::CliError,
    globals::{Tier, with_globals},
    prompt::PromptHistory,
};
use clap::{Arg, ArgAction, ArgMatches, Command};
use rtao::{
    client::Client,
    primitives::AccountId,
    reads::reads::{m6_staking, owner::StakePosition},
    tx::ExtrinsicResult,
};
use serde_json::{Map, json};
use std::collections::BTreeSet;

/// Build one high-level exit command with shared transaction globals.
pub fn command(name: &'static str, include_root: bool) -> Command {
    with_globals(Command::new(name)
        .about(if include_root { "Unstake every position on one hotkey, or sweep all hotkeys atomically." } else { "Sell alpha positions to root stake on one hotkey, or sweep all hotkeys atomically." })
        .after_help("Use --hotkey for one hotkey, or --all-hotkeys for every live non-zero position. Multiple hotkeys execute in Utility.batch_all, which the Staking proxy type cannot wrap; sign directly or use a broader proxy type such as NonTransfer.")
        .arg(Arg::new("hotkey_ss58").long("hotkey").conflicts_with("all_hotkeys").help(crate::books::ss58_param_help("hotkey_ss58")))
        .arg(Arg::new("all_hotkeys").long("all-hotkeys").action(ArgAction::SetTrue))
        .arg(Arg::new("__period").long("period").value_parser(clap::value_parser!(u32))), Tier::Tx)
}

/// Exit target selection retained without opening a signing session.
pub struct Request {
    /// Root positions participate in a free-balance exit; alpha-only exits ignore them.
    pub include_root: bool,
    /// One explicit or local-default hotkey, absent for a picker or sweep.
    pub hotkey: Option<AccountId>,
    /// Discover all live non-zero hotkeys for the effective stake owner.
    pub all_hotkeys: bool,
    /// Explicit mortal era override.
    pub period: Option<u32>,
}
impl Request {
    /// Resolve explicit public targets and conflicting selectors before chain access.
    pub fn from_matches(
        ctx: &mut AppContext,
        include_root: bool,
        leaf: &ArgMatches,
        history: &mut PromptHistory,
    ) -> Result<Self, CliError> {
        let raw = leaf.get_one::<String>("hotkey_ss58").map(String::as_str);
        let all_hotkeys = leaf.get_flag("all_hotkeys");
        if all_hotkeys && raw.is_some() {
            return Err(CliError::input(
                "choose only one of --hotkey or --all-hotkeys",
            ));
        }
        let interactive = ctx.prompt_context().interactive(&ctx.terminal)
            && !ctx.globals.assume_yes
            && !ctx.globals.uses_external_signer();
        let hotkey = if all_hotkeys || (raw.is_none() && interactive) {
            None
        } else {
            Some(
                ctx.resolve_target("hotkey_ss58", raw, history)?
                    .ok_or_else(|| {
                        CliError::input("missing hotkey")
                            .with_help("pass --hotkey <HOTKEY> or --all-hotkeys")
                    })?,
            )
        };
        Ok(Self {
            include_root,
            hotkey,
            all_hotkeys,
            period: leaf.get_one::<u32>("__period").copied(),
        })
    }
}

/// Deduplicate eligible targets in runtime position order, preserving exact amounts.
pub fn live_hotkeys(positions: &[StakePosition], include_root: bool) -> Vec<AccountId> {
    let mut seen = BTreeSet::new();
    positions
        .iter()
        .filter(|position| position.stake.rao > 0 && (include_root || position.netuid.0 != 0))
        .filter_map(|position| seen.insert(position.hotkey).then_some(position.hotkey))
        .collect()
}

/// Use a single semantic intent for one target and Utility.batch_all for two or more.
/// The caller gives the batch the child intents' shield default at submission.
pub fn transaction(
    include_root: bool,
    hotkeys: &[AccountId],
    period: Option<u32>,
) -> Result<generated::TxRequest, CliError> {
    if hotkeys.is_empty() {
        return Err(CliError::failure("no live stake positions"));
    }
    let op = if include_root {
        "unstake_all"
    } else {
        "unstake_all_alpha"
    };
    let request = if hotkeys.len() == 1 {
        generated::TxRequest {
            op,
            args: Map::from_iter([("hotkey_ss58".into(), json!(hotkeys[0]))]),
            period,
        }
    } else {
        generated::TxRequest {
            op: "batch",
            args: Map::from_iter([(
                "intents".into(),
                json!(
                    hotkeys
                        .iter()
                        .map(|hotkey| json!({"op":op,"hotkey_ss58":hotkey}))
                        .collect::<Vec<_>>()
                ),
            )]),
            period,
        }
    };
    request.intent()?;
    Ok(request)
}

/// Resolve live targets, then submit once through the ordinary semantic executor.
pub async fn execute(
    ctx: &mut AppContext,
    client: &Client,
    request: &Request,
    options: SubmissionOptions<'_>,
    external: Option<&dyn ExternalSignerFactory>,
    history: &mut PromptHistory,
) -> Result<Option<ExtrinsicResult>, CliError> {
    let previous = history.replay_command(&[]);
    let hotkeys = if request.all_hotkeys {
        let owner = picker::owner(ctx).await?;
        let positions = m6_staking::stake_for_coldkey(client, owner)
            .await
            .map_err(failure)?;
        let hotkeys = live_hotkeys(&positions, request.include_root);
        if hotkeys.is_empty() {
            let kind = if request.include_root {
                "stake"
            } else {
                "alpha stake outside root"
            };
            return Err(CliError::failure(format!("coldkey {owner} has no {kind}"))
                .with_help("`rtao stake list` shows every position"));
        }
        hotkeys
    } else if let Some(hotkey) = request.hotkey {
        vec![hotkey]
    } else {
        vec![{
            let mut answers = Map::new();
            picker::source(ctx, client, "hotkey_ss58", None, &mut answers, history).await?;
            answers["hotkey_ss58"]
                .as_str()
                .ok_or_else(|| failure("picker omitted hotkey"))?
                .parse()
                .map_err(failure)?
        }]
    };
    let tx = transaction(request.include_root, &hotkeys, request.period)?;
    let replay = super::updated_replay(options.replay, &previous, history);
    if hotkeys.len() > 1
        && ctx.globals.force_proxy_type.as_deref() == Some("Staking")
        && ctx.dispatch_proxy(None)?.is_some()
    {
        return Err(CliError::input(
            "Staking proxy cannot wrap a multi-hotkey Utility.batch_all exit",
        )
        .with_help("sign directly or use a broader proxy type such as NonTransfer"));
    }
    generated::execute_tx(
        ctx,
        client,
        &tx,
        SubmissionOptions {
            replay: replay.as_deref(),
            mev_shield_default: (hotkeys.len() > 1)
                .then_some(true)
                .or(options.mev_shield_default),
            ..options
        },
        external,
        history,
    )
    .await
}
