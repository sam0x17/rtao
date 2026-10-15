// Derived from RaoFoundation/subtensor sdk/python/bittensor/cli/commands/evm/__init__.py @ 850f2755bb05a3851f948489f0ab27b0645cebce (Apache-2.0); modified by rtao.
// Additional sources at the same commit/license: sdk/python/bittensor/cli/commands/evm/_shared.py, address.py, money.py, stake.py.
//! EVM keys, native funding, JSON-RPC transactions, precompiles and configuration.

mod association;
/// Contract artifact deployment commands.
pub mod contracts;
mod keys;
/// Precompile discovery and generic read/write calls.
pub mod precompiles;
/// Public endpoint diagnosis and existing-node native setup commands.
pub mod setup;

use super::generated::{self, TxRequest};
use crate::{
    context::{AppContext, ExternalSignerFactory, SubmissionOptions},
    error::CliError,
    globals::{Tier, with_globals},
    output::ReviewSection,
    prompt::{PromptHistory, PromptSpec, PromptValidation, ask_with},
};
use clap::{Arg, ArgAction, ArgMatches, Command};
use rtao::{
    client::Client,
    evm::{self, Address, EvmKeyInfo, EvmNetwork, EvmRpc, EvmTxPreview, U256},
    primitives::{DynBalance, NetUid},
};
use serde_json::{Map, Value, json};
use std::{future::Future, pin::Pin, str::FromStr, time::Duration};

fn failure(error: impl std::fmt::Display) -> CliError {
    CliError::failure(error.to_string())
}
fn optional<'a>(matches: &'a ArgMatches, field: &str) -> Option<&'a str> {
    matches
        .try_get_one::<String>(field)
        .ok()
        .flatten()
        .map(String::as_str)
}
fn required<'a>(matches: &'a ArgMatches, field: &str) -> Result<&'a str, CliError> {
    optional(matches, field)
        .ok_or_else(|| CliError::input(format!("missing --{}", field.replace('_', "-"))))
}
fn evm_key(command: Command) -> Command {
    command.arg(
        Arg::new("evm_key")
            .long("evm-key")
            .help("Stored EVM key NAME or WALLET/NAME; defaults to default."),
    )
}
fn rpc_url(command: Command) -> Command {
    command.arg(
        Arg::new("rpc_url")
            .long("rpc-url")
            .help("EVM HTTP(S) JSON-RPC endpoint."),
    )
}
fn netuid(command: Command) -> Command {
    command
        .arg(
            Arg::new("netuid")
                .long("netuid")
                .required(true)
                .value_parser(clap::value_parser!(u16).range(0..4096)),
        )
        .arg(
            Arg::new("hotkey_ss58")
                .long("hotkey")
                .help("Validator hotkey, book name or local hotkey."),
        )
}

/// Construct all twenty-five pinned Python EVM command routes.
pub fn command() -> Result<Command, CliError> {
    let mut group = Command::new("evm")
        .about("Bittensor EVM: keys, funding, transfers, association, and precompiles.")
        .subcommand_required(true)
        .arg_required_else_help(true)
        .subcommand(keys::command())
        .subcommand(
            Command::new("stake")
                .subcommand_required(true)
                .arg_required_else_help(true)
                .subcommand(with_globals(
                    rpc_url(evm_key(netuid(Command::new("add"))))
                        .arg(Arg::new("amount_tao").long("amount-tao").required(true)),
                    Tier::Tx,
                ))
                .subcommand(with_globals(
                    rpc_url(evm_key(netuid(Command::new("remove"))))
                        .arg(Arg::new("amount_alpha").long("amount-alpha").required(true)),
                    Tier::Tx,
                ))
                .subcommand(with_globals(
                    rpc_url(evm_key(netuid(Command::new("show")))),
                    Tier::Read,
                )),
        )
        .subcommand(with_globals(
            Command::new("mirror").arg(Arg::new("address")),
            Tier::Read,
        ))
        .subcommand(with_globals(
            Command::new("pubkey").arg(Arg::new("ss58").required(true)),
            Tier::Read,
        ))
        .subcommand(with_globals(Command::new("deposit-address"), Tier::Read))
        .subcommand(with_globals(
            rpc_url(Command::new("balance").arg(Arg::new("address"))),
            Tier::Read,
        ))
        .subcommand(with_globals(
            evm_key(Command::new("fund"))
                .arg(Arg::new("amount_tao").long("amount-tao"))
                .arg(
                    Arg::new("all_amount")
                        .long("all")
                        .action(ArgAction::SetTrue),
                ),
            Tier::Tx,
        ))
        .subcommand(with_globals(
            rpc_url(evm_key(Command::new("send")))
                .arg(Arg::new("to").long("to").required(true))
                .arg(Arg::new("amount_tao").long("amount-tao").required(true)),
            Tier::Tx,
        ))
        .subcommand(with_globals(
            rpc_url(evm_key(Command::new("send-to-ss58")))
                .arg(Arg::new("to").long("to"))
                .arg(Arg::new("amount_tao").long("amount-tao").required(true)),
            Tier::Tx,
        ))
        .subcommand(generated::intent_command("evm_withdraw")?.name("claim-deposit"))
        .subcommand(association::command());
    for command in std::iter::once(contracts::command())
        .chain(precompiles::commands())
        .chain(setup::commands())
    {
        group = group.subcommand(command);
    }
    Ok(group)
}

/// Leaf-owned IDs excluded from global flag resolution.
pub fn reserved(group: &ArgMatches) -> Result<Vec<&'static str>, CliError> {
    let (name, child) = group
        .subcommand()
        .ok_or_else(|| CliError::input("missing evm command"))?;
    Ok(match name {
        "key" => keys::reserved(child)?,
        "stake" => vec![
            "netuid",
            "hotkey_ss58",
            "amount_tao",
            "amount_alpha",
            "evm_key",
            "rpc_url",
        ],
        "mirror" | "balance" => vec!["address", "rpc_url"],
        "pubkey" => vec!["ss58"],
        "fund" => vec!["evm_key", "amount_tao", "all_amount"],
        "send" | "send-to-ss58" => vec!["to", "amount_tao", "evm_key", "rpc_url"],
        "claim-deposit" => return generated::intent_reserved("evm_withdraw"),
        "associate" => association::reserved(),
        "precompiles" | "abi" | "call" => precompiles::reserved(name),
        "deploy" => contracts::reserved(),
        "networks" | "config" | "doctor" | "setup-localnet" => setup::reserved(name),
        "deposit-address" => vec![],
        _ => return Err(CliError::input(format!("unknown evm command {name:?}"))),
    })
}

/// Whether a route needs a native Subtensor client; EVM HTTP/key operations do not.
pub fn needs_native(group: &ArgMatches) -> bool {
    matches!(
        group.subcommand_name(),
        Some("fund" | "claim-deposit" | "associate" | "setup-localnet")
    )
}

/// Reject invalid native-route public flags before opening a Subtensor connection.
pub fn validate_before_connect(ctx: &AppContext, group: &ArgMatches) -> Result<(), CliError> {
    let (name, leaf) = group
        .subcommand()
        .ok_or_else(|| CliError::input("missing evm command"))?;
    if name == "setup-localnet" {
        setup::refuse_public_network(ctx)?;
    }
    if name == "fund" {
        let amount = optional(leaf, "amount_tao");
        let all = leaf.get_flag("all_amount");
        if all
            && amount.is_some_and(|amount| {
                rtao::primitives::Amount::parse(amount) != Ok(rtao::primitives::Amount::All)
            })
        {
            return Err(CliError::input("pass --amount-tao or --all, not both"));
        }
        if let Some(amount) = amount {
            rtao::primitives::Amount::parse(amount)
                .and_then(|amount| amount.resolve(NetUid::ROOT, true))
                .map_err(|error| CliError::input(error.to_string()))?;
        }
    }
    Ok(())
}

async fn resolve_account(
    ctx: &AppContext,
    param: &'static str,
    value: Option<&str>,
) -> Result<rtao::primitives::AccountId, CliError> {
    let ctx = ctx.clone();
    let value = value.map(str::to_owned);
    rtao::core::offload(move || ctx.resolve_address(param, value.as_deref()))
        .await
        .map_err(failure)??
        .ok_or_else(|| CliError::input(format!("cannot resolve {param}")))
}

fn key_ref(ctx: &AppContext, value: Option<&str>) -> (String, String) {
    match value.filter(|value| !value.is_empty()) {
        Some(value) => match value.rsplit_once('/') {
            Some((wallet, name)) => (
                if wallet.is_empty() {
                    ctx.globals.wallet_name.clone()
                } else {
                    wallet.into()
                },
                name.into(),
            ),
            None => (ctx.globals.wallet_name.clone(), value.into()),
        },
        None => (ctx.globals.wallet_name.clone(), "default".into()),
    }
}
async fn key_info(ctx: &AppContext, value: Option<&str>) -> Result<EvmKeyInfo, CliError> {
    let (wallet, name) = key_ref(ctx, value);
    let root = ctx.globals.wallet_path.clone();
    rtao::core::offload(move || evm::get_evm_key_info(&name, &wallet, &root))
        .await
        .map_err(failure)?
        .map_err(|error| failure(error).with_help("create one with `rtao evm key new`"))
}
async fn address_of(ctx: &AppContext, value: Option<&str>) -> Result<String, CliError> {
    if let Some(value) = value
        && (value.starts_with("0x") || evm::is_h160(&format!("0x{value}")))
    {
        return evm::normalize_h160(value).map_err(|error| CliError::input(error.to_string()));
    }
    Ok(key_info(ctx, value).await?.address)
}
fn tao_to_wei(raw: &str) -> Result<U256, CliError> {
    let amount = DynBalance::from_tao(raw)
        .map_err(|error| CliError::input(format!("invalid TAO amount {raw:?}: {error}")))?;
    evm::balance_to_wei(&amount).map_err(failure)
}
async fn rpc(ctx: &AppContext, endpoint: Option<&str>) -> Result<(EvmNetwork, EvmRpc), CliError> {
    let network = evm::evm_network(endpoint.unwrap_or(&ctx.globals.network)).map_err(failure)?;
    let url = network.rpc_url.clone();
    let rpc = rtao::core::offload(move || EvmRpc::new(url))
        .await
        .map_err(failure)?
        .map_err(failure)?;
    Ok((network, rpc))
}
fn bytes(raw: &str) -> Result<Vec<u8>, CliError> {
    hex::decode(raw.strip_prefix("0x").unwrap_or(raw))
        .map_err(|_| CliError::input("invalid EVM calldata hex"))
}

/// Injected preparation or submission future, with no implicit sockets or key access.
pub type EvmFuture<'a, T> = Pin<Box<dyn Future<Output = Result<T, CliError>> + Send + 'a>>;

/// Side-effect boundary for testing the shared EVM preview/confirm/submit discipline.
pub trait EvmTxBackend: Send + Sync {
    /// Inspect public keystore metadata without decrypting it.
    fn key_info<'a>(
        &'a self,
        ctx: &'a AppContext,
        key: Option<&'a str>,
    ) -> EvmFuture<'a, EvmKeyInfo>;
    /// Resolve gas, nonce and chain ID before any private material is acquired.
    fn prepare<'a>(
        &'a self,
        ctx: &'a AppContext,
        endpoint: Option<&'a str>,
        sender: Address,
        to: Option<Address>,
        value: U256,
        data: Vec<u8>,
    ) -> EvmFuture<'a, EvmTxPreview>;
    /// Unlock, sign and submit once, only after the caller has confirmed the preview.
    fn submit<'a>(
        &'a self,
        ctx: &'a AppContext,
        key: Option<&'a str>,
        endpoint: Option<&'a str>,
        preview: EvmTxPreview,
    ) -> EvmFuture<'a, Value>;
}
struct NativeEvmTx;
impl EvmTxBackend for NativeEvmTx {
    fn key_info<'a>(
        &'a self,
        ctx: &'a AppContext,
        key: Option<&'a str>,
    ) -> EvmFuture<'a, EvmKeyInfo> {
        Box::pin(key_info(ctx, key))
    }
    fn prepare<'a>(
        &'a self,
        ctx: &'a AppContext,
        endpoint: Option<&'a str>,
        sender: Address,
        to: Option<Address>,
        value: U256,
        data: Vec<u8>,
    ) -> EvmFuture<'a, EvmTxPreview> {
        Box::pin(async move {
            let (_, rpc) = rpc(ctx, endpoint).await?;
            match evm::prepare_transaction(&rpc, sender, to, value, &data).await {
                Ok(preview) => Ok(preview),
                Err(error) => {
                    let mut findings = Vec::new();
                    if let Ok(balance) = rpc.get_balance_wei(sender).await {
                        if balance.is_zero() {
                            findings.push(format!(
                                "sender {sender} has zero balance; fund it with `rtao evm fund`"
                            ));
                        } else if balance < value {
                            findings.push("sender balance is less than the attached value".into());
                        }
                    }
                    if rpc.chain_id().await.is_err() {
                        findings.push("the node has no EVM chain ID; use `rtao evm setup-localnet` for an existing local endpoint".into());
                    }
                    if to.is_none() && findings.is_empty() {
                        findings.push(
                            "contract deployment may be blocked by the deployment whitelist".into(),
                        );
                    }
                    let note = if findings.is_empty() {
                        "eth_estimateGas also reports insufficient balance, invalid calls, unset chain IDs and deployment whitelist failures".into()
                    } else {
                        findings.join("; ")
                    };
                    Err(failure(error).with_help(note))
                }
            }
        })
    }
    fn submit<'a>(
        &'a self,
        ctx: &'a AppContext,
        key: Option<&'a str>,
        endpoint: Option<&'a str>,
        preview: EvmTxPreview,
    ) -> EvmFuture<'a, Value> {
        Box::pin(async move {
            let account = keys::unlock_key(ctx, key).await?;
            let signed = rtao::core::offload(move || {
                evm::transactions::sign_transaction(&account, &preview)
            })
            .await
            .map_err(failure)?
            .map_err(failure)?;
            let (_, rpc) = rpc(ctx, endpoint).await?;
            let hash = signed.hash;
            rpc.send_raw_transaction(&signed.raw)
                .await
                .map_err(|error| unresolved(hash, error))?;
            let receipt = rpc
                .wait_for_receipt(hash, Duration::from_secs(120))
                .await
                .map_err(|error| unresolved(hash, error))?;
            Ok(evm::transactions::EvmTxResult {
                tx_hash: hash,
                receipt: Some(receipt),
            }
            .to_json())
        })
    }
}
fn unresolved(hash: evm::B256, error: evm::EvmError) -> CliError {
    failure(evm::EvmError::UnknownFate { hash,cause:Box::new(error) })
        .with_help(format!("transaction fate is unknown; inspect receipt for {hash:#x} before any new submission; do not resend automatically"))
}

#[allow(clippy::too_many_arguments)]
async fn submit_evm_tx(
    ctx: &AppContext,
    key: Option<&str>,
    to: Option<&str>,
    value_wei: U256,
    data: Vec<u8>,
    summary: String,
    endpoint: Option<&str>,
    preview_fields: Map<String, Value>,
) -> Result<Option<Value>, CliError> {
    submit_evm_tx_with_backend(
        ctx,
        key,
        to,
        value_wei,
        data,
        summary,
        endpoint,
        preview_fields,
        &NativeEvmTx,
    )
    .await
}

/// Prepare and review exactly one EVM transaction, then authorize its injected secret boundary.
#[allow(clippy::too_many_arguments)]
pub async fn submit_evm_tx_with_backend(
    ctx: &AppContext,
    key: Option<&str>,
    to: Option<&str>,
    value_wei: U256,
    data: Vec<u8>,
    summary: String,
    endpoint: Option<&str>,
    preview_fields: Map<String, Value>,
    backend: &dyn EvmTxBackend,
) -> Result<Option<Value>, CliError> {
    evm::wei_to_balance(value_wei).map_err(|error| CliError::input(error.to_string()))?;
    if data.len() > 8 * 1024 * 1024 {
        return Err(CliError::input("EVM transaction data exceeds 8 MiB"));
    }
    let info = backend.key_info(ctx, key).await?;
    let sender = Address::from_str(&info.address).map_err(failure)?;
    let recipient = to.map(Address::from_str).transpose().map_err(failure)?;
    let requested_data = data.clone();
    let preview = backend
        .prepare(ctx, endpoint, sender, recipient, value_wei, data)
        .await?;
    // An injectable transport cannot silently change the reviewed signing domain.
    if preview.sender != sender
        || preview.to != recipient
        || preview.value_wei != value_wei
        || preview.data != requested_data
    {
        return Err(failure(
            "EVM preparation changed the requested sender, recipient, value or calldata",
        ));
    }
    let cloned = preview.clone();
    let public = rtao::core::offload(move || cloned.to_json())
        .await
        .map_err(failure)?
        .map_err(failure)?;
    let data = public["data"].as_str().unwrap_or("0x");
    let shown_data = if data.len() > 80 {
        format!("{}… (+{} chars)", &data[..80], data.len() - 80)
    } else {
        data.into()
    };
    let mut context = vec![
        ("summary".into(), json!(summary)),
        ("from".into(), json!(info.address)),
        ("to".into(), public["to"].clone()),
    ];
    if !value_wei.is_zero() {
        context.push((
            "value".into(),
            json!(preview.value().map_err(failure)?.to_string()),
        ));
    }
    context.extend(
        preview_fields
            .into_iter()
            .map(|(name, value)| (name.replace('_', " "), value)),
    );
    let sections = [
        ReviewSection {
            title: "Evm".into(),
            rows: context,
        },
        ReviewSection {
            title: "Fees".into(),
            rows: vec![
                (
                    "max fee".into(),
                    json!(preview.max_fee().map_err(failure)?.to_string()),
                ),
                ("gas".into(), json!(preview.gas.to_string())),
            ],
        },
        ReviewSection {
            title: "Signer".into(),
            rows: vec![("signer".into(), json!(info.address))],
        },
        ReviewSection {
            title: "Transaction".into(),
            rows: vec![
                ("operation".into(), json!(summary)),
                ("nonce".into(), json!(preview.nonce)),
                ("chain id".into(), json!(preview.chain_id)),
                ("data".into(), json!(shown_data)),
            ],
        },
    ];
    ctx.output.transaction_card(&sections).map_err(failure)?;
    if ctx.globals.dry_run {
        let fields = public
            .as_object()
            .map(|map| {
                map.iter()
                    .map(|(name, value)| (name.clone(), value.clone()))
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        ctx.output
            .detail(Some("evm transaction preview"), &fields, Some(&public))
            .map_err(failure)?;
        return Ok(None);
    }
    ctx.confirm(&format!(
        "{} (max fee {})",
        crate::output::terminal_text(&summary),
        preview.max_fee().map_err(failure)?
    ))?;
    let mut result = backend.submit(ctx, key, endpoint, preview).await?;
    result["from"] = json!(info.address);
    result["to"] = json!(to.unwrap_or("(contract creation)"));
    if result.get("success") == Some(&Value::Bool(false)) {
        return Err(failure("transaction reverted")
            .with_help(serde_json::to_string(&result).map_err(failure)?));
    }
    let fields = result
        .as_object()
        .map(|map| {
            map.iter()
                .map(|(name, value)| (name.clone(), value.clone()))
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    ctx.output
        .detail(Some(&summary), &fields, Some(&result))
        .map_err(|error| {
            failure(error).with_help(format!(
                "transaction already submitted; retain this result before retrying: {result}"
            ))
        })?;
    Ok(Some(result))
}

/// Dispatch an EVM route; the caller supplies one native client only for native operations.
pub async fn run(
    ctx: &mut AppContext,
    client: Option<&Client>,
    group: &ArgMatches,
    options: SubmissionOptions<'_>,
    external: Option<&dyn ExternalSignerFactory>,
    history: &mut PromptHistory,
) -> Result<(), CliError> {
    validate_before_connect(ctx, group)?;
    let (name, leaf) = group
        .subcommand()
        .ok_or_else(|| CliError::input("missing evm command"))?;
    match name {
        "key" => keys::run(ctx, leaf).await,
        "precompiles" | "abi" | "call" => precompiles::run(ctx, name, leaf).await,
        "deploy" => contracts::run(ctx, leaf).await,
        "networks" | "config" | "doctor" | "setup-localnet" => {
            setup::run(ctx, name, leaf, client, options, external, history).await
        }
        "associate" => {
            association::run(
                ctx,
                client.ok_or_else(|| failure("native client is required"))?,
                leaf,
                options,
                external,
                history,
            )
            .await
        }
        "claim-deposit" => {
            let client = client.ok_or_else(|| failure("native client is required"))?;
            let request =
                TxRequest::from_matches_with_client(ctx, client, "evm_withdraw", leaf, history)
                    .await?;
            generated::execute_tx(ctx, client, &request, options, external, history).await?;
            Ok(())
        }
        "fund" => {
            let address = address_of(ctx, optional(leaf, "evm_key")).await?;
            let all = leaf.get_flag("all_amount");
            let amount = optional(leaf, "amount_tao");
            if all
                && amount.is_some_and(|amount| {
                    rtao::primitives::Amount::parse(amount) != Ok(rtao::primitives::Amount::All)
                })
            {
                return Err(CliError::input("pass --amount-tao or --all, not both"));
            }
            let amount = if all {
                "all".to_owned()
            } else if let Some(amount) = amount {
                amount.to_owned()
            } else {
                let mut terminal = ctx.terminal.clone();
                let (_, raw) = ask_with(
                    &ctx.prompt_context(),
                    &mut terminal,
                    &PromptSpec::new("amount_tao", "--amount-tao"),
                    |raw| {
                        rtao::primitives::Amount::parse(raw)
                            .and_then(|amount| amount.resolve(NetUid::ROOT, true))
                            .map_err(|error| PromptValidation::Message(error.to_string()))
                    },
                )
                .map_err(|error| CliError {
                    code: error.exit_code(),
                    message: error.to_string(),
                    help: None,
                })?;
                history.record_answers(vec!["--amount-tao".into(), raw.clone()]);
                raw
            };
            let request = TxRequest {
                op: "fund_evm_key",
                args: Map::from_iter([
                    ("evm_address".into(), json!(address)),
                    ("amount_tao".into(), json!(amount)),
                ]),
                period: None,
            };
            generated::execute_tx(
                ctx,
                client.ok_or_else(|| failure("native client is required"))?,
                &request,
                options,
                external,
                history,
            )
            .await?;
            Ok(())
        }
        "stake" => stake(ctx, leaf).await,
        "mirror" | "pubkey" | "deposit-address" | "balance" => address(ctx, name, leaf).await,
        "send" | "send-to-ss58" => send(ctx, name, leaf).await,
        _ => Err(CliError::input(format!("unknown evm command {name:?}"))),
    }
}

async fn address(ctx: &AppContext, name: &str, leaf: &ArgMatches) -> Result<(), CliError> {
    let (title, human, payload) = match name {
        "pubkey" => {
            let ss58 = required(leaf, "ss58")?;
            let pubkey = evm::ss58_to_pubkey(ss58).map_err(|error| {
                CliError::input(format!("invalid ss58 address {ss58:?}: {error}"))
            })?;
            let payload = json!({"ss58":ss58,"pubkey":pubkey});
            (
                None,
                vec![
                    ("ss58".into(), json!(ss58)),
                    ("pubkey".into(), json!(pubkey)),
                ],
                payload,
            )
        }
        "deposit-address" => {
            let coldkey = resolve_account(ctx, "coldkey_ss58", None)
                .await?
                .to_string();
            let truncated = evm::ss58_to_h160_truncated(&coldkey).map_err(failure)?;
            let mirror = evm::h160_to_ss58(&truncated).map_err(failure)?;
            (
                Some(format!(
                    "EVM deposit address for {}",
                    ctx.globals.wallet_name
                )),
                vec![
                    ("coldkey".into(), json!(coldkey)),
                    ("evm deposit address".into(), json!(truncated)),
                    ("its ss58 mirror".into(), json!(mirror)),
                ],
                json!({"coldkey":coldkey,"evm_deposit_address":truncated,"mirror_ss58":mirror}),
            )
        }
        name => {
            let address = address_of(ctx, optional(leaf, "address")).await?;
            let mirror = evm::h160_to_ss58(&address).map_err(failure)?;
            if name == "balance" {
                let (_, rpc) = rpc(ctx, optional(leaf, "rpc_url")).await?;
                let wei = rpc
                    .get_balance_wei(Address::from_str(&address).map_err(failure)?)
                    .await
                    .map_err(failure)?;
                let balance = evm::wei_to_balance(wei).map_err(failure)?;
                let wei: Value = serde_json::from_str(&wei.to_string()).map_err(failure)?;
                (
                    None,
                    vec![
                        ("address".into(), json!(address)),
                        ("balance".into(), json!(balance.to_string())),
                        ("wei".into(), wei.clone()),
                        ("ss58 mirror".into(), json!(mirror)),
                    ],
                    json!({"address":address,"balance_tao":balance.to_decimal_string().trim_end_matches('0').trim_end_matches('.'),"balance_wei":wei,"ss58_mirror":mirror}),
                )
            } else {
                (
                    None,
                    vec![
                        ("address".into(), json!(address)),
                        ("ss58 mirror".into(), json!(mirror)),
                    ],
                    json!({"address":address,"ss58_mirror":mirror}),
                )
            }
        }
    };
    ctx.output
        .detail(title.as_deref(), &human, Some(&payload))
        .map_err(failure)?;
    if name == "deposit-address" {
        ctx.output.message("send TAO from the EVM side to the deposit address, then run `rtao evm claim-deposit --amount-tao <n>` to pull it into the coldkey").map_err(failure)?;
    }
    Ok(())
}
async fn send(ctx: &AppContext, name: &str, leaf: &ArgMatches) -> Result<(), CliError> {
    let amount = required(leaf, "amount_tao")?;
    let value = tao_to_wei(amount)?;
    let balance = DynBalance::from_tao(amount).map_err(failure)?;
    let (to, data, summary) = if name == "send" {
        let dest = address_of(ctx, Some(required(leaf, "to")?)).await?;
        (dest.clone(), vec![], format!("send {balance} to {dest}"))
    } else {
        let dest = resolve_account(ctx, "coldkey_ss58", optional(leaf, "to"))
            .await?
            .to_string();
        let precompile = evm::get_precompile("balance-transfer").map_err(failure)?;
        let data = precompile
            .encode_call(
                "transfer",
                &[json!(evm::ss58_to_pubkey(&dest).map_err(failure)?)],
            )
            .map_err(failure)?;
        (
            precompile.address(),
            bytes(&data)?,
            format!("send {balance} from the EVM key to {dest}"),
        )
    };
    submit_evm_tx(
        ctx,
        optional(leaf, "evm_key"),
        Some(&to),
        value,
        data,
        summary,
        optional(leaf, "rpc_url"),
        Map::new(),
    )
    .await?;
    Ok(())
}
async fn stake(ctx: &AppContext, group: &ArgMatches) -> Result<(), CliError> {
    let (name, leaf) = group
        .subcommand()
        .ok_or_else(|| CliError::input("missing evm stake command"))?;
    let netuid = *leaf
        .get_one::<u16>("netuid")
        .ok_or_else(|| CliError::input("missing --netuid"))?;
    let hotkey = resolve_account(ctx, "hotkey_ss58", optional(leaf, "hotkey_ss58"))
        .await?
        .to_string();
    let precompile = evm::get_precompile("staking-v2").map_err(failure)?;
    if name == "show" {
        let info = key_info(ctx, optional(leaf, "evm_key")).await?;
        let data = precompile
            .encode_call(
                "getStake",
                &[json!(hotkey), json!(info.ss58_mirror), json!(netuid)],
            )
            .map_err(failure)?;
        let (_, rpc) = rpc(ctx, optional(leaf, "rpc_url")).await?;
        let raw = rpc
            .eth_call(&json!({"to":precompile.address(),"data":data}))
            .await
            .map_err(failure)?;
        let function = precompile.function("getStake").map_err(failure)?;
        let decoded = evm::precompiles::decode_result_bytes(&function, &raw).map_err(failure)?;
        let rao = decoded
            .first()
            .and_then(|value| {
                value
                    .as_u64()
                    .or_else(|| value.as_str().and_then(|value| value.parse().ok()))
            })
            .ok_or_else(|| failure("staking result exceeds native u64 rao"))?;
        let amount = DynBalance::from_rao(rao, NetUid(netuid));
        let payload = json!({"evm_key":info.name,"address":info.address,"coldkey_mirror":info.ss58_mirror,"hotkey":hotkey,"netuid":netuid,"stake_rao":rao});
        return ctx
            .output
            .detail(
                Some(&format!("EVM stake on netuid {netuid}")),
                &[
                    (
                        "evm key".into(),
                        json!(format!("{} ({})", info.name, info.address)),
                    ),
                    ("coldkey (mirror)".into(), json!(info.ss58_mirror)),
                    ("hotkey".into(), json!(hotkey)),
                    ("stake".into(), json!(amount.to_string())),
                ],
                Some(&payload),
            )
            .map_err(failure);
    }
    let (function, amount, summary) = match name {
        "add" => {
            let amount = DynBalance::from_tao(required(leaf, "amount_tao")?)
                .map_err(|error| CliError::input(error.to_string()))?;
            let summary = format!("stake {amount} from the EVM key to {hotkey} on netuid {netuid}");
            ("addStake", amount, summary)
        }
        "remove" => {
            let amount = DynBalance::from_amount(required(leaf, "amount_alpha")?, NetUid(netuid))
                .map_err(|error| CliError::input(error.to_string()))?;
            let summary =
                format!("unstake {amount} from {hotkey} on netuid {netuid} to the EVM key");
            ("removeStake", amount, summary)
        }
        _ => {
            return Err(CliError::input(format!(
                "unknown evm stake command {name:?}"
            )));
        }
    };
    let data = precompile
        .encode_call(function, &[json!(hotkey), json!(amount.rao), json!(netuid)])
        .map_err(failure)?;
    submit_evm_tx(
        ctx,
        optional(leaf, "evm_key"),
        Some(&precompile.address()),
        U256::ZERO,
        bytes(&data)?,
        summary,
        optional(leaf, "rpc_url"),
        Map::new(),
    )
    .await?;
    Ok(())
}
