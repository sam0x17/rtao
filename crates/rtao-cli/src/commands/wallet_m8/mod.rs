// Derived from RaoFoundation/subtensor sdk/python/bittensor/cli/commands/wallet.py @ 850f2755bb05a3851f948489f0ab27b0645cebce (Apache-2.0); modified by rtao.
//! Remaining wallet operations share the ordinary SDK acquisition and submit path.

use super::*;
use crate::{
    commands::generated::{self, TxRequest},
    context::{CliWalletKeys, SubmissionOptions},
};
use rtao::{
    client::{BlockRef, View},
    signer::{DynKeySource, Signer},
};

pub(super) mod history;

const IDENTITY_FIELDS: [&str; 7] = [
    "name",
    "url",
    "description",
    "github_repo",
    "image",
    "discord",
    "additional",
];

fn period(command: Command) -> Command {
    command.arg(
        option("__period", "period", "Mortal transaction period in blocks.")
            .value_parser(clap::value_parser!(u32)),
    )
}

pub(super) fn commands() -> Vec<Command> {
    let message = || option("message", "message", "Exact UTF-8 message text.").required(true);
    let use_hotkey = || {
        flag(
            "use_hotkey",
            "use-hotkey",
            "Use the hotkey instead of the coldkey.",
        )
    };
    let coldkey = || {
        option(
            "coldkey_ss58",
            "coldkey",
            "Coldkey address, wallet, or saved account.",
        )
        .alias("coldkey-ss58")
    };
    let netuid = || {
        option("netuid", "netuid", "Filter to one subnet.").value_parser(clap::value_parser!(u16))
    };
    let all = || flag("all", "all", "Include every local wallet.").short('a');
    let mut identity = Command::new("set-identity")
        .about("Set public coldkey identity; omitted fields keep their current values.");
    for field in IDENTITY_FIELDS {
        identity = identity.arg(
            Arg::new(field)
                .long(field.replace('_', "-"))
                .help(format!("Public identity {field}.")),
        );
    }
    vec![
        with_globals(
            Command::new("sign")
                .about("Sign a UTF-8 message with the wallet key.")
                .arg(message())
                .arg(use_hotkey()),
            Tier::Unlock,
        ),
        with_globals(
            Command::new("verify")
                .about("Verify a message signature against an address.")
                .arg(message())
                .arg(option("signature", "signature", "Bare or 0x-prefixed signature hex.").required(true))
                .arg(option("ss58", "ss58", "Signer address.").required(true)),
            Tier::Read,
        ),
        with_globals(
            Command::new("encrypt")
                .about("Encrypt UTF-8 text for an ed25519 recipient.")
                .arg(message())
                .arg(option("recipient", "recipient", "Recipient ed25519 SS58 address.").required(true)),
            Tier::Read,
        ),
        with_globals(
            Command::new("decrypt")
                .about("Decrypt a sealed message with the wallet key.")
                .arg(option("ciphertext", "ciphertext", "Bare or 0x-prefixed sealed-box hex.").required(true))
                .arg(use_hotkey())
                .arg(flag("copy", "copy", "Copy plaintext to the clipboard instead of printing it.")),
            Tier::Unlock,
        ),
        with_globals(Command::new("unlock").about("Unlock the configured wallet coldkey."), Tier::Unlock),
        with_globals(
            Command::new("overview")
                .about("Show free TAO, per-subnet stake, locks and root yield.")
                .arg(all())
                .arg(netuid())
                .arg(flag("dust", "dust", "Include dust positions in human output; JSON includes them.")),
            Tier::Read,
        ),
        with_globals(
            Command::new("registrations")
                .about("List local and chain-owned hotkeys, including zero-stake registrations.")
                .arg(all())
                .arg(netuid()),
            Tier::Read,
        ),
        with_globals(
            Command::new("history")
                .about("Show recent mainnet transfers from TaoMarketCap.")
                .long_about("Show recent mainnet transfers from TaoMarketCap. The queried coldkey address is sent to the third-party api.taomarketcap.com indexer.")
                .arg(coldkey())
                .arg(option("limit", "limit", "Maximum transfers, newest first.")
                    .value_parser(clap::value_parser!(i64)).allow_negative_numbers(true).default_value("50")),
            Tier::Read,
        ),
        with_globals(
            period(Command::new("transfer").about("Transfer TAO to another coldkey.")
                .arg(option("dest_ss58", "dest", "Destination address, wallet or saved account.").alias("dest-ss58"))
                .arg(option("amount_tao", "amount-tao", "Amount in TAO, or all.").alias("amount"))
                .arg(flag("__all_amount", "all", "Send the entire transferable balance."))
                // Retain every catalog field identifier for the shared parser.
                // Python's hand-written transfer keeps keep_alive at its default.
                .arg(flag("keep_alive", "keep-alive", "Keep the sender alive.").hide(true))
                .arg(flag("__no_keep_alive", "no-keep-alive", "Permit reaping the sender.").hide(true))),
            Tier::Tx,
        ),
        with_globals(period(identity), Tier::Tx),
        with_globals(Command::new("get-identity").about("Show public coldkey identity.").arg(coldkey()), Tier::Read),
        with_globals(
            period(Command::new("associate-hotkey").about("Associate a hotkey with the wallet coldkey on chain.")
                .arg(option("hotkey_ss58", "hotkey", "Hotkey address or local hotkey name.").alias("hotkey-ss58"))),
            Tier::Tx,
        ),
        with_globals(
            period(Command::new("swap-hotkey").about("Rotate the registered hotkey using its owning coldkey.")
                .arg(option("new_hotkey_ss58", "new-hotkey", "Replacement hotkey.").alias("new-hotkey-ss58").required(true))
                .arg(option("netuid", "netuid", "Only rotate this subnet; omit for all subnets."))
                .arg(option("hotkey_ss58", "hotkey", "Old hotkey; defaults to the wallet hotkey.").hide(true))),
            Tier::Tx,
        ),
        with_globals(
            period(Command::new("swap-coldkey").about("Execute an announced coldkey migration after its waiting period.")
                .arg(option("new_coldkey_ss58", "new-coldkey", "Previously announced replacement coldkey.").alias("new-coldkey-ss58").required(true))),
            Tier::Tx,
        ),
        with_globals(Command::new("swap-check").about("Check a pending coldkey swap announcement.").arg(coldkey()), Tier::Read),
        with_globals(
            period(Command::new("announce-coldkey-swap").about("Announce a replacement coldkey and start the waiting period.")
                .arg(option("new_coldkey_ss58", "new-coldkey", "Replacement coldkey.").alias("new-coldkey-ss58").required(true))),
            Tier::Tx,
        ),
    ]
}

pub(super) fn is_chain(name: &str) -> bool {
    matches!(
        name,
        "overview" | "registrations" | "get-identity" | "swap-check"
    ) || transaction_op(name).is_some()
}
pub(super) fn is_local(name: &str) -> bool {
    matches!(name, "sign" | "verify" | "encrypt" | "decrypt" | "unlock")
}
pub(super) fn transaction_op(name: &str) -> Option<&'static str> {
    Some(match name {
        "transfer" => "transfer",
        "set-identity" => "set_identity",
        "associate-hotkey" => "associate_hotkey",
        "swap-hotkey" => "swap_hotkey",
        "swap-coldkey" => "swap_coldkey_announced",
        "announce-coldkey-swap" => "announce_coldkey_swap",
        _ => return None,
    })
}

pub(super) async fn transaction(
    ctx: &mut AppContext,
    client: &Client,
    name: &str,
    matches: &ArgMatches,
    history: &mut PromptHistory,
) -> Result<(), CliError> {
    let op = transaction_op(name).ok_or_else(|| input("unknown wallet transaction"))?;
    let request = if op == "set_identity" {
        identity_request(ctx, client, matches, history).await?
    } else if op == "associate_hotkey" {
        let hotkey = ctx
            .resolve_target("hotkey_ss58", text(matches, "hotkey_ss58"), history)?
            .ok_or_else(|| input("missing hotkey"))?;
        let request = TxRequest {
            op,
            args: serde_json::Map::from_iter([("hotkey_ss58".into(), json!(hotkey))]),
            period: matches.get_one::<u32>("__period").copied(),
        };
        request.intent()?;
        request
    } else {
        TxRequest::from_matches_with_client(ctx, client, op, matches, history).await?
    };
    generated::execute_tx(
        ctx,
        client,
        &request,
        SubmissionOptions::default(),
        None,
        history,
    )
    .await?;
    Ok(())
}

async fn identity_request(
    ctx: &mut AppContext,
    client: &Client,
    matches: &ArgMatches,
    history: &mut PromptHistory,
) -> Result<TxRequest, CliError> {
    ctx.confirm_wallet(
        &WalletConfirmation {
            help: "Wallet whose coldkey identity to set.".into(),
            ..Default::default()
        },
        history,
    )?;
    // Saved multisig selection has the same precedence as the mutation route.
    let lookup = FsAccountLookup::new(ctx.globals.wallet_path.clone()).map_err(failure)?;
    let resolver = NameResolver::new(
        &ctx.books,
        &lookup,
        &ctx.globals.wallet_name,
        &ctx.globals.hotkey_name,
    );
    let owner = match resolver
        .saved_multisig_address(&ctx.globals.wallet_name)
        .map_err(failure)?
    {
        Some(address) => address.parse().map_err(input)?,
        None => ctx
            .resolve_address("coldkey_ss58", None)?
            .ok_or_else(|| input("missing coldkey"))?,
    };
    let view = client.pin().await.map_err(failure)?;
    let current = rtao::reads::reads::owner::identity(&view, owner)
        .await
        .map_err(failure)?
        .unwrap_or_else(|| json!({}));
    if !ctx.globals.json {
        if current.as_object().is_some_and(|fields| !fields.is_empty()) {
            ctx.output
                .detail(
                    Some("current identity"),
                    &current
                        .as_object()
                        .into_iter()
                        .flatten()
                        .map(|(k, v)| (k.clone(), v.clone()))
                        .collect::<Vec<_>>(),
                    None,
                )
                .map_err(failure)?;
        } else {
            ctx.output
                .message("no on-chain identity yet")
                .map_err(failure)?;
        }
    }
    let mut args = serde_json::Map::new();
    let context = ctx.prompt_context();
    let mut terminal = ctx.terminal.clone();
    for field in IDENTITY_FIELDS {
        let current_value = current.get(field).and_then(Value::as_str).unwrap_or("");
        let value = if let Some(value) = text(matches, field) {
            value.to_owned()
        } else if context.interactive(&terminal) {
            let mut spec =
                crate::prompt::PromptSpec::new(field, format!("--{}", field.replace('_', "-")));
            spec.help = Some(format!("Public identity {field}."));
            spec.default =
                (!current_value.is_empty() || field != "name").then(|| current_value.to_owned());
            crate::prompt::ask_with(&context, &mut terminal, &spec, |raw| Ok(raw.to_owned()))
                .map_err(prompt_error)?
                .0
        } else {
            current_value.to_owned()
        };
        args.insert(field.into(), json!(value));
    }
    let request = TxRequest {
        op: "set_identity",
        args,
        period: matches.get_one::<u32>("__period").copied(),
    };
    request.intent()?;
    Ok(request)
}

pub(super) async fn get_identity(
    ctx: &mut AppContext,
    client: &Client,
    matches: &ArgMatches,
    history: &mut PromptHistory,
) -> Result<(), CliError> {
    let owner = ctx
        .resolve_target("coldkey_ss58", text(matches, "coldkey_ss58"), history)?
        .ok_or_else(|| input("missing coldkey"))?;
    let view = match matches.try_get_one::<u64>("__block").ok().flatten() {
        Some(block) => client.at(BlockRef::Number(*block)).await,
        None => client.pin().await,
    }
    .map_err(failure)?;
    let identity = rtao::reads::reads::owner::identity(&view, owner)
        .await
        .map_err(failure)?;
    if ctx.globals.json {
        return ctx.output.value(&identity).map_err(failure);
    }
    let fields = identity
        .and_then(|v| v.as_object().cloned())
        .unwrap_or_default();
    ctx.output
        .detail(
            Some("identity"),
            &fields.into_iter().collect::<Vec<_>>(),
            None,
        )
        .map_err(failure)
}

pub(super) async fn swap_check(
    ctx: &mut AppContext,
    client: &Client,
    matches: &ArgMatches,
    history: &mut PromptHistory,
) -> Result<(), CliError> {
    let owner = ctx
        .resolve_target("coldkey_ss58", text(matches, "coldkey_ss58"), history)?
        .ok_or_else(|| input("missing coldkey"))?;
    let view = client.pin().await.map_err(failure)?;
    let Some(announcement) =
        rtao::reads::reads::m6_accounts::coldkey_swap_announcement(&view, owner)
            .await
            .map_err(failure)?
    else {
        return ctx
            .output
            .detail(
                Some("coldkey swap status"),
                &[("status".into(), json!("none"))],
                None,
            )
            .map_err(failure);
    };
    let current_block = view.block().number;
    let remaining = u64::from(announcement.execute_block).saturating_sub(current_block);
    let executable = swap_executable(remaining, jiff::Zoned::now())?;
    ctx.output
        .detail(
            Some("coldkey swap status"),
            &[
                ("execute_block".into(), json!(announcement.execute_block)),
                ("current_block".into(), json!(current_block)),
                ("blocks_remaining".into(), json!(remaining)),
                ("executable".into(), json!(executable)),
                (
                    "new_coldkey_hash".into(),
                    json!(announcement.new_coldkey_hash),
                ),
                ("disputed".into(), json!(announcement.disputed)),
                ("dispute_block".into(), json!(announcement.dispute_block)),
            ],
            None,
        )
        .map_err(failure)
}

fn swap_executable(remaining: u64, now: jiff::Zoned) -> Result<String, CliError> {
    if remaining == 0 {
        return Ok("now (waiting period elapsed)".into());
    }
    let seconds = remaining.saturating_mul(12);
    let rounded_seconds = if seconds >= 60 {
        let minutes = seconds / 60;
        let remainder = seconds % 60;
        (minutes + u64::from(remainder > 30 || (remainder == 30 && !minutes.is_multiple_of(2))))
            .saturating_mul(60)
    } else {
        seconds
    };
    let days = rounded_seconds / 86400;
    let hours = rounded_seconds % 86400 / 3600;
    let minutes = rounded_seconds % 3600 / 60;
    let secs = rounded_seconds % 60;
    let mut parts = Vec::new();
    if days > 0 {
        parts.push(format!("{days}d"));
    }
    if hours > 0 {
        parts.push(format!("{hours}h"));
    }
    if minutes > 0 {
        parts.push(format!("{minutes}m"));
    }
    if secs > 0 || parts.is_empty() {
        parts.push(format!("{secs}s"));
    }
    // Python astimezone() captures the current offset and abbreviation. A
    // future ETA keeps that fixed offset even if a DST transition intervenes.
    let info = now.time_zone().to_offset_info(now.timestamp());
    let eta = now
        .timestamp()
        .checked_add(jiff::SignedDuration::from_secs(
            i64::try_from(seconds).map_err(input)?,
        ))
        .map_err(input)?
        .to_zoned(jiff::tz::TimeZone::fixed(now.offset()));
    Ok(format!(
        "in ~{} ({} {})",
        parts.join(" "),
        eta.strftime("%Y-%m-%d %H:%M"),
        info.abbreviation(),
    ))
}

pub(super) async fn run_local(
    ctx: &mut AppContext,
    name: &str,
    matches: &ArgMatches,
    history: &mut PromptHistory,
) -> Result<(), CliError> {
    if name == "verify" {
        let message = text(matches, "message").ok_or_else(|| input("missing --message"))?;
        let signature = text(matches, "signature").ok_or_else(|| input("missing --signature"))?;
        let ss58 = text(matches, "ss58").ok_or_else(|| input("missing --ss58"))?;
        // Python constructs a public-only Keypair with its default sr25519 scheme.
        // Its verifier accepts exact bytes and the historical <Bytes> wrapper.
        let valid = rtao::wallet::verify_message(message, signature, ss58);
        ctx.output
            .detail(
                None,
                &[("valid".into(), json!(valid)), ("ss58".into(), json!(ss58))],
                None,
            )
            .map_err(failure)?;
        return if valid {
            Ok(())
        } else {
            Err(CliError {
                code: 1,
                message: String::new(),
                help: None,
            })
        };
    }
    if name == "encrypt" {
        let message = text(matches, "message").ok_or_else(|| input("missing --message"))?;
        let recipient = text(matches, "recipient").ok_or_else(|| input("missing --recipient"))?;
        let result = rtao::wallet::encrypt_message(message, recipient)
            .await
            .map_err(failure)?;
        return ctx
            .output
            .detail(
                Some("encrypted"),
                &[
                    ("ciphertext".into(), json!(result.ciphertext)),
                    ("recipient".into(), json!(result.recipient)),
                ],
                None,
            )
            .map_err(failure);
    }
    if name == "decrypt" && enabled(matches, "copy") && ctx.globals.json {
        return Err(input("`--copy` does not apply in --json mode")
            .with_help("drop --copy; JSON output prints the decrypted message"));
    }
    let hotkey = enabled(matches, "use_hotkey");
    ctx.confirm_wallet(
        &WalletConfirmation {
            help: format!("Wallet to {name}."),
            require_coldkey: !hotkey,
            allow_multisig: false,
            ..Default::default()
        },
        history,
    )?;
    let wallet = ctx
        .wallet()?
        .with_hotkey_password_sources(rtao::wallet::PasswordSources {
            prompt: ctx.passwords.prompt,
            prompt_callback: ctx.passwords.prompt_callback.clone(),
            ..Default::default()
        });
    if name == "unlock" {
        let file = wallet.reference().coldkey_path();
        let data = rtao::core::offload(move || rtao::wallet::read_keyfile_data(&file))
            .await
            .map_err(|_| failure("keyfile inspection task failed"))?
            .map_err(failure)?;
        let encrypted = rtao::core::keyfile_json::keyfile_data_is_encrypted(&data);
        let ss58 = if encrypted {
            local_signer(ctx, wallet, false).await?.account()
        } else {
            wallet
                .public_info(SignerRole::Coldkey)
                .map_err(failure)?
                .account
        };
        return ctx
            .output
            .detail(
                Some(if encrypted { "unlocked" } else { "unlock" }),
                &[
                    ("wallet".into(), json!(ctx.globals.wallet_name)),
                    ("encrypted".into(), json!(encrypted)),
                    ("ss58".into(), json!(ss58)),
                ],
                None,
            )
            .map_err(failure);
    }
    if name == "sign" {
        let message = text(matches, "message").ok_or_else(|| input("missing --message"))?;
        let signer = local_signer(ctx, wallet, hotkey).await?;
        let signature = signer.sign_raw(message.as_bytes()).await.map_err(failure)?;
        return ctx
            .output
            .detail(
                Some("signed"),
                &[
                    ("signed_message".into(), json!(hex::encode(signature))),
                    ("signer_address".into(), json!(signer.account())),
                ],
                None,
            )
            .map_err(failure);
    }
    if name == "decrypt" {
        let ciphertext =
            text(matches, "ciphertext").ok_or_else(|| input("missing --ciphertext"))?;
        // SDK retains secret ownership through decryption and UTF-8 validation.
        let plaintext = decrypt_with_retry(ctx, &wallet, ciphertext, hotkey).await?;
        if enabled(matches, "copy") {
            let mut runner = crate::process::SystemProcessRunner::default();
            if crate::secrets::copy_to_clipboard(&mut runner, &plaintext).await {
                return ctx
                    .output
                    .message("decrypted message copied to clipboard (not printed)")
                    .map_err(failure);
            }
            ctx.output.message("warning: no clipboard tool found (pbcopy, wl-copy, xclip, or xsel) — printing instead").map_err(failure)?;
        }
        return if ctx.output.options.json_mode {
            ctx.output
                .secret_detail(
                    Some("decrypted"),
                    &[],
                    &[("message", plaintext.expose_secret())],
                )
                .map_err(failure)
        } else {
            ctx.output.message("decrypted").map_err(failure)?;
            ctx.output
                .secret_line(&["  message: ", plaintext.expose_secret()])
                .map_err(failure)
        };
    }
    Err(input("unknown local wallet operation"))
}

async fn local_signer(
    ctx: &AppContext,
    wallet: Wallet,
    hotkey: bool,
) -> Result<Arc<dyn Signer>, CliError> {
    CliWalletKeys::new(wallet, ctx.output.clone())
        .signer_for(if hotkey {
            SignerRole::Hotkey
        } else {
            SignerRole::Coldkey
        })
        .await
        .map_err(failure)
}

async fn decrypt_with_retry(
    ctx: &AppContext,
    wallet: &Wallet,
    ciphertext: &str,
    hotkey: bool,
) -> Result<SecretString, CliError> {
    for attempts_left in [2, 1, 0] {
        match wallet.decrypt_message(ciphertext, hotkey).await {
            Err(rtao::wallet::WalletError::WrongPassword {
                source: rtao::wallet::PasswordSource::Prompt,
            }) if !hotkey && attempts_left > 0 => {
                ctx.output
                    .error(
                        "wrong password",
                        crate::output::DiagnosticContext::default(),
                    )
                    .map_err(failure)?;
            }
            result => return result.map_err(failure),
        }
    }
    Err(failure("decryption failed"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn swap_wait_rounding_and_elapsed_state_match_python() -> Result<(), Box<dyn std::error::Error>>
    {
        let now: jiff::Zoned = "2026-10-15T12:00:00Z[UTC]".parse()?;
        assert_eq!(
            swap_executable(0, now.clone())?,
            "now (waiting period elapsed)"
        );
        assert_eq!(
            swap_executable(1, now.clone())?,
            "in ~12s (2026-10-15 12:00 UTC)"
        );
        assert_eq!(
            swap_executable(8, now.clone())?,
            "in ~2m (2026-10-15 12:01 UTC)"
        );
        assert_eq!(swap_executable(300, now)?, "in ~1h (2026-10-15 13:00 UTC)");
        let dst: jiff::Zoned = "2026-11-01T01:59:00-04:00[America/New_York]".parse()?;
        assert_eq!(swap_executable(10, dst)?, "in ~2m (2026-11-01 02:01 EDT)");
        Ok(())
    }
}
