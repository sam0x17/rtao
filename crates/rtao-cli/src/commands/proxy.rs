// Derived from RaoFoundation/subtensor sdk/python/bittensor/cli/commands/proxy.py @ 850f2755bb05a3851f948489f0ab27b0645cebce (Apache-2.0); modified by rtao.
// Additional source at the same commit/license: sdk/python/bittensor/config.py.
//! Proxy delegation commands and pure-account creation/retirement bookkeeping.

use super::{
    generated::{self, TxRequest},
    schema::{self, Field},
};
use crate::{
    books::{AccountLookup, FsAccountLookup, ProxyBook, public_json_truthy},
    context::{AppContext, ExternalSignerFactory, SubmissionOptions},
    error::CliError,
    globals::{Tier, with_globals},
    output::TableSpec,
    prompt::{
        PromptError, PromptFlow, PromptHistory, PromptResolver, PromptSpec, PromptValidation,
        Terminal, ask_with, fill_missing, python_strip,
    },
};
use clap::{Arg, ArgAction, ArgMatches, Command};
use rtao::{
    client::{Client, config::Paths},
    primitives::{AccountId, pyfmt},
    tx::ExtrinsicResult,
};
use serde_json::{Map, Value, json};
use std::collections::BTreeSet;

fn failure(error: impl std::fmt::Display) -> CliError {
    CliError::failure(error.to_string())
}

fn prompt_error(error: PromptError) -> CliError {
    CliError {
        code: error.exit_code(),
        ..crate::error::CliError::failure(error.to_string())
    }
}

fn fields(op: &str) -> Result<Vec<Field>, CliError> {
    schema::intent_fields(
        rtao_catalog::INTENTS
            .iter()
            .find(|spec| spec.name == op)
            .ok_or_else(|| CliError::input(format!("unknown proxy operation {op:?}")))?,
    )
}

fn proxy_choices(command: Command, field: &str) -> Command {
    command.mut_arg(field, |arg| {
        arg.value_parser(rtao_intents::proxy::PROXY_TYPES)
    })
}

/// Build create/add/remove/kill/list/execute with the pinned Python defaults.
pub fn command() -> Result<Command, CliError> {
    let create = proxy_choices(
        generated::intent_command("create_pure_proxy")?,
        "proxy_type",
    )
    .name("create")
    .arg(
        Arg::new("name")
            .long("name")
            .help("Save the spawned address and creation coordinates in the local proxy book."),
    );
    let add = proxy_choices(generated::intent_command("add_proxy")?, "proxy_type")
        .name("add")
        .mut_arg("delegate_ss58", |arg| arg.required(true));
    let remove = proxy_choices(generated::intent_command("remove_proxy")?, "proxy_type")
        .name("remove")
        .arg(
            Arg::new("all_proxies")
                .long("all")
                .action(ArgAction::SetTrue)
                .help("Remove every proxy at once; pure accounts may become inaccessible."),
        );
    let kill = proxy_choices(generated::intent_command("kill_pure_proxy")?, "proxy_type")
        .name("kill")
        .about("Kill a pure proxy; empty it first, because remaining funds become inaccessible.")
        .arg(Arg::new("name").index(1).help(
            "Saved proxy entry supplying its spawner, type, index, height and ext-index; flags override it."));
    let list = with_globals(
        Command::new("list")
            .about("Show an account's on-chain proxy delegations and reserved deposit.")
            .arg(
                Arg::new("coldkey_ss58")
                    .long("coldkey")
                    .help(crate::books::ss58_param_help("coldkey_ss58")),
            ),
        Tier::Read,
    );
    let execute = proxy_choices(
        generated::intent_command("execute_proxy_announced")?,
        "force_proxy_type",
    )
    .name("execute")
    .mut_arg("delegate_ss58", |arg| arg.required(true))
    .mut_arg("real_ss58", |arg| arg.required(true))
    .mut_arg("inner_op", |arg| arg.required(true))
    .mut_arg("inner_args", |arg| arg.default_value("{}"));
    Ok(Command::new("proxy")
        .about("On-chain proxy management and the local proxy book.")
        .subcommand_required(true)
        .arg_required_else_help(true)
        .subcommands([create, add, remove, kill, list, execute, book_command()]))
}

/// Reserve command fields so execute's inner force type stays an intent input.
pub fn reserved(matches: &ArgMatches) -> Result<Vec<&'static str>, CliError> {
    let (name, _) = matches
        .subcommand()
        .ok_or_else(|| CliError::input("missing proxy command"))?;
    match name {
        "create" => {
            let mut names = generated::intent_reserved("create_pure_proxy")?;
            names.push("name");
            Ok(names)
        }
        "add" => generated::intent_reserved("add_proxy"),
        "remove" => {
            let mut names = generated::intent_reserved("remove_proxy")?;
            names.push("all_proxies");
            Ok(names)
        }
        "kill" => {
            let mut names = generated::intent_reserved("kill_pure_proxy")?;
            names.push("name");
            Ok(names)
        }
        "list" => Ok(vec!["coldkey_ss58"]),
        "execute" => generated::intent_reserved("execute_proxy_announced"),
        "book" => Ok(Vec::new()),
        _ => Err(CliError::input(format!("unknown proxy command {name:?}"))),
    }
}

enum AfterSubmit {
    None,
    Create { name: Option<String> },
    Kill { name: Option<String> },
}

enum RequestKind {
    Transaction {
        tx: TxRequest,
        after: AfterSubmit,
        proxy_override: Option<String>,
    },
    List(AccountId),
}

/// Prepared command, including the pure-account lifecycle metadata.
pub struct Request {
    kind: RequestKind,
}

impl Request {
    /// Resolve public arguments and safety prompts before connecting/unlocking.
    pub fn from_matches(
        ctx: &mut AppContext,
        matches: &ArgMatches,
        history: &mut PromptHistory,
    ) -> Result<Self, CliError> {
        let (name, leaf) = matches
            .subcommand()
            .ok_or_else(|| CliError::input("missing proxy command"))?;
        if name == "list" {
            return Ok(Self {
                kind: RequestKind::List(
                    ctx.resolve_address("coldkey_ss58", text(leaf, "coldkey_ss58"))?
                        .ok_or_else(|| CliError::input("proxy list requires a public coldkey"))?,
                ),
            });
        }
        if name == "kill" {
            return kill_request(ctx, leaf, history);
        }
        let (op, after) = match name {
            "create" => (
                "create_pure_proxy",
                AfterSubmit::Create {
                    name: text(leaf, "name").map(str::to_owned),
                },
            ),
            "add" => ("add_proxy", AfterSubmit::None),
            "remove" if leaf.get_flag("all_proxies") => {
                confirm_remove_all(ctx)?;
                let tx = TxRequest {
                    op: "remove_proxies",
                    args: Map::new(),
                    period: period(leaf),
                };
                tx.intent()?;
                return Ok(Self {
                    kind: RequestKind::Transaction {
                        tx,
                        after: AfterSubmit::None,
                        proxy_override: None,
                    },
                });
            }
            "remove" => ("remove_proxy", AfterSubmit::None),
            "execute" => ("execute_proxy_announced", AfterSubmit::None),
            _ => return Err(CliError::input(format!("unknown proxy command {name:?}"))),
        };
        let tx = TxRequest::from_matches(ctx, op, leaf, history)?;
        Ok(Self {
            kind: RequestKind::Transaction {
                tx,
                after,
                proxy_override: None,
            },
        })
    }
}

fn text<'a>(matches: &'a ArgMatches, field: &str) -> Option<&'a str> {
    matches.get_one::<String>(field).map(String::as_str)
}

fn period(matches: &ArgMatches) -> Option<u32> {
    matches.get_one::<u32>("__period").copied()
}

fn confirm_remove_all(ctx: &AppContext) -> Result<(), CliError> {
    let mut terminal = ctx.terminal.clone();
    if ctx.globals.assume_yes || !ctx.prompt_context().interactive(&terminal) {
        return Ok(());
    }
    ctx.output.message("removing ALL proxies: any pure proxy spawned by this account becomes permanently inaccessible, along with its funds").map_err(failure)?;
    let reply = terminal
        .read_line("type \"remove all\" to continue: ")
        .map_err(prompt_error)?;
    if python_strip(&reply).to_lowercase() != "remove all" {
        return Err(CliError::failure("aborted."));
    }
    Ok(())
}

struct KillResolver<'a> {
    ctx: &'a AppContext,
    fields: &'a [Field],
}
impl PromptResolver for KillResolver<'_> {
    fn parse(&mut self, spec: &PromptSpec, raw: &str) -> Result<Value, PromptValidation> {
        if spec.field == "spawner_ss58" {
            return self
                .ctx
                .resolve_address("coldkey_ss58", Some(raw))
                .map(|account| json!(account))
                .map_err(|error| PromptValidation::Message(error.message));
        }
        schema::coerce(
            self.fields
                .iter()
                .find(|field| field.name == spec.field)
                .ok_or_else(|| {
                    PromptValidation::Message("unknown pure-proxy creation field".into())
                })?,
            raw,
        )
        .map_err(|error| PromptValidation::Message(error.message))
    }
    fn custom(
        &mut self,
        spec: &PromptSpec,
        terminal: &mut dyn Terminal,
        answers: &mut Map<String, Value>,
    ) -> Result<Vec<String>, PromptError> {
        let lookup = FsAccountLookup::new(self.ctx.globals.wallet_path.clone())
            .map_err(|error| PromptError::InvalidValue(error.to_string()))?;
        let mut seen = BTreeSet::new();
        let mut choices = Vec::new();
        for entry in &self.ctx.books.addresses {
            if let (Some(name), Some(address)) = (entry["name"].as_str(), entry["address"].as_str())
                && seen.insert(address.to_owned())
            {
                choices.push((name.to_owned(), address.to_owned()));
            }
        }
        for entry in lookup
            .candidates()
            .into_iter()
            .filter(|entry| entry.source == "wallet" && entry.name != self.ctx.globals.wallet_name)
        {
            if seen.insert(entry.address.clone()) {
                choices.push((entry.name, entry.address));
            }
        }
        for (index, (name, account)) in choices.iter().enumerate() {
            terminal.message(&format!("{}  {name}  {account}", index + 1))?;
        }
        let (value, _) = ask_with(&self.ctx.prompt_context(), terminal, spec, |raw| {
            let raw = if let Ok(index) = raw.parse::<usize>()
                && !choices.is_empty()
            {
                choices
                    .get(index.saturating_sub(1))
                    .filter(|_| index > 0)
                    .map(|(_, address)| address.as_str())
                    .ok_or_else(|| {
                        PromptValidation::Message(format!(
                            "enter a number between 1 and {}",
                            choices.len()
                        ))
                    })?
            } else {
                raw
            };
            self.parse(spec, raw)
        })?;
        let token = value
            .as_str()
            .ok_or_else(|| PromptError::InvalidValue("selected account is not text".into()))?
            .to_owned();
        answers.insert(spec.field.clone(), value);
        Ok(vec![spec.flag.clone(), token])
    }
}

fn kill_request(
    ctx: &mut AppContext,
    matches: &ArgMatches,
    history: &mut PromptHistory,
) -> Result<Request, CliError> {
    let name = text(matches, "name").map(str::to_owned);
    let entry = name
        .as_deref()
        .map(|name| {
            ctx.books.proxy(name).cloned().ok_or_else(|| {
                CliError::failure(format!(
                    "proxy {} not found in the proxy book",
                    pyfmt::string_repr(name)
                ))
                .with_help("`rtao proxy book list` shows saved entries")
            })
        })
        .transpose()?;
    let entry_value = |key: &str| {
        entry
            .as_ref()
            .and_then(|entry| entry.get(key))
            .filter(|value| !value.is_null())
            .cloned()
    };
    let fields = fields("kill_pure_proxy")?;
    let mut answers = Map::new();
    for field in &fields {
        let value = text(matches, field.name)
            .filter(|raw| field.name != "spawner_ss58" || !raw.is_empty())
            .map(|raw| schema::coerce(field, raw))
            .transpose()?
            .or_else(|| {
                entry_value(if field.name == "spawner_ss58" {
                    "spawner"
                } else {
                    field.name
                })
            });
        let value = if matches!(field.name, "index" | "height" | "ext_index") {
            value
                .map(|value| match value.as_str() {
                    Some(raw) => schema::coerce(field, raw),
                    None => Ok(value),
                })
                .transpose()?
        } else {
            value
        };
        if let Some(value) = value {
            answers.insert(field.name.into(), value);
        }
    }
    // CLI requires these creation coordinates even though the lower-level
    // intent's constructor has zero defaults. Explicit zero overrides the book.
    let mut missing = Vec::new();
    for field in ["spawner_ss58", "height", "ext_index"] {
        if !answers.get(field).is_some_and(|value| !value.is_null()) {
            let mut spec = PromptSpec::new(
                field,
                format!("--{}", field.trim_end_matches("_ss58").replace('_', "-")),
            );
            if field == "spawner_ss58" {
                spec.flow = PromptFlow::DestinationAccount;
            } else {
                spec.placeholder = Some("integer".into());
            }
            missing.push(spec);
        }
    }
    let mut terminal = ctx.terminal.clone();
    fill_missing(
        &ctx.prompt_context(),
        &mut terminal,
        &missing,
        &mut answers,
        &mut KillResolver {
            ctx,
            fields: &fields,
        },
        history,
    )
    .map_err(prompt_error)?;
    let raw = answers
        .get("spawner_ss58")
        .and_then(Value::as_str)
        .ok_or_else(|| CliError::input("--spawner requires an account reference"))?;
    let spawner = ctx
        .resolve_address("coldkey_ss58", Some(raw))?
        .ok_or_else(|| CliError::input("--spawner requires an account reference"))?;
    answers.insert("spawner_ss58".into(), json!(spawner));
    if answers
        .get("proxy_type")
        .is_none_or(|value| value.as_str() == Some(""))
    {
        answers.insert("proxy_type".into(), json!("Staking"));
    }
    if !answers.contains_key("index") {
        answers.insert("index".into(), json!(0));
    }
    let proxy_override = match ctx.globals.proxy_for.as_deref() {
        Some("self") => Some("self".into()),
        Some(raw) if !raw.is_empty() => Some(
            ctx.resolve_address("proxy_for", Some(raw))?
                .ok_or_else(|| CliError::input("--proxy-for requires an account reference"))?
                .to_string(),
        ),
        _ => entry_value("address")
            .and_then(|value| {
                value
                    .as_str()
                    .filter(|value| !value.is_empty())
                    .map(str::to_owned)
            })
            .map(|raw| {
                ctx.resolve_address("proxy_for", Some(&raw))?
                    .ok_or_else(|| CliError::input("saved proxy has no account"))
                    .map(|account| account.to_string())
            })
            .transpose()?,
    };
    if proxy_override
        .as_deref()
        .is_none_or(|target| target == "self")
    {
        ctx.output.message("note: kill_pure must be dispatched by the pure account itself; without `--proxy-for` the call is signed directly and will likely fail with NoPermission").map_err(failure)?;
    }
    let tx = TxRequest {
        op: "kill_pure_proxy",
        args: answers,
        period: period(matches),
    };
    tx.intent()?;
    Ok(Request {
        kind: RequestKind::Transaction {
            tx,
            after: AfterSubmit::Kill { name },
            proxy_override,
        },
    })
}

/// Execute through the ordinary SDK review, proxy/multisig and receipt boundary.
/// Pure-account book changes happen only after a successful actual receipt.
pub async fn execute(
    ctx: &mut AppContext,
    client: &Client,
    request: &Request,
    options: SubmissionOptions<'_>,
    external: Option<&dyn ExternalSignerFactory>,
    history: &mut PromptHistory,
) -> Result<Option<ExtrinsicResult>, CliError> {
    match &request.kind {
        RequestKind::List(owner) => {
            list(ctx, client, *owner).await?;
            Ok(None)
        }
        RequestKind::Transaction {
            tx,
            after,
            proxy_override,
        } => {
            let previous = ctx.globals.proxy_for.clone();
            if let Some(target) = proxy_override {
                ctx.globals.proxy_for = Some(target.clone());
            }
            let result = generated::execute_tx(ctx, client, tx, options, external, history).await;
            ctx.globals.proxy_for = previous;
            let result = result?;
            if let Some(receipt) = result.as_ref().filter(|receipt| receipt.success) {
                // Approval success alone does not mean the pure account died.
                // Retain its recovery coordinates until as_multi executed it.
                if !matches!(after, AfterSubmit::Kill { .. }) || kill_dispatched(ctx, receipt) {
                    after_submit(ctx, tx, after, receipt).await?;
                }
            }
            Ok(result)
        }
    }
}

fn kill_dispatched(ctx: &AppContext, receipt: &ExtrinsicResult) -> bool {
    let Some(entry) = ctx.books.multisig(&ctx.globals.wallet_name) else {
        return true;
    };
    // as_multi_threshold_1 dispatches immediately and emits no MultisigExecuted.
    entry.get("threshold").and_then(Value::as_u64) == Some(1)
        || crate::call::multisig_executed(receipt)
}

async fn list(ctx: &AppContext, client: &Client, owner: AccountId) -> Result<(), CliError> {
    let lookup = FsAccountLookup::new(ctx.globals.wallet_path.clone()).map_err(failure)?;
    for entry in lookup
        .candidates()
        .into_iter()
        .filter(|entry| entry.source == "wallet")
    {
        ctx.output
            .name_address(&entry.address, &entry.name)
            .map_err(failure)?;
    }
    for entry in ctx.books.addresses.iter().chain(&ctx.books.proxies) {
        if let (Some(name), Some(address)) = (entry["name"].as_str(), entry["address"].as_str()) {
            ctx.output.name_address(address, name).map_err(failure)?;
        }
    }
    let data = rtao::reads::generated::interfaces::proxies(client, owner)
        .await
        .map_err(CliError::from)?;
    let records = serde_json::to_value(&data.proxies).map_err(failure)?;
    let payload = json!({"account":owner,"proxies":records,"deposit":data.deposit.to_string()});
    if ctx.output.options.json_mode {
        return ctx.output.value(&payload).map_err(failure);
    }
    let names = ctx.output.address_names().map_err(failure)?;
    let label = |account: &str| {
        names
            .get(account)
            .map(|name| format!("{name} ({account})"))
            .unwrap_or_else(|| account.to_owned())
    };
    let rows = records
        .as_array()
        .ok_or_else(|| failure("proxy delegations are not a list"))?
        .iter()
        .map(|row| {
            vec![
                json!(label(row["delegate"].as_str().unwrap_or(""))),
                row["proxy_type"].clone(),
                row["delay"].clone(),
            ]
        })
        .collect();
    ctx.output
        .columns(&TableSpec {
            title: format!("on-chain proxies — {}", label(&owner.to_string())),
            columns: vec!["delegate".into(), "type".into(), "delay".into()],
            rows,
            records: Some(records),
            right_align: BTreeSet::new(),
            legend: Vec::new(),
            footer: (!data.proxies.is_empty()).then(|| format!("deposit {}", data.deposit)),
        })
        .map_err(failure)
}

fn context_book(ctx: &AppContext) -> Result<ProxyBook, CliError> {
    match &ctx.config_store {
        Some(store) => Ok(ProxyBook::new(store.paths())),
        None => Ok(ProxyBook::new(&Paths::from_process().map_err(failure)?)),
    }
}

fn book_command() -> Command {
    let add = Command::new("add")
        .about("Add an entry to the local proxy address book.")
        .arg(Arg::new("name").long("name").required(true))
        .arg(
            Arg::new("address")
                .long("address")
                .required(true)
                .help("Pure or delegate proxy SS58."),
        )
        .arg(Arg::new("spawner").long("spawner").default_value(""))
        .arg(
            Arg::new("proxy_type")
                .long("proxy-type")
                .default_value("Staking")
                .value_parser(rtao_intents::proxy::PROXY_TYPES),
        )
        .arg(
            Arg::new("delay")
                .long("delay")
                .default_value("0")
                .allow_hyphen_values(true),
        )
        .arg(
            Arg::new("index")
                .long("index")
                .default_value("0")
                .allow_hyphen_values(true),
        )
        .arg(Arg::new("height").long("height").allow_hyphen_values(true))
        .arg(
            Arg::new("ext_index")
                .long("ext-index")
                .allow_hyphen_values(true),
        )
        .arg(Arg::new("note").long("note").default_value(""));
    Command::new("book")
        .about("Local proxy address book (named pure/delegate proxies).")
        .subcommand_required(true)
        .arg_required_else_help(true)
        .subcommands([
            with_globals(add, Tier::Read),
            with_globals(
                Command::new("list").about("List saved proxies.").arg(
                    Arg::new("verify")
                        .long("verify")
                        .action(ArgAction::SetTrue)
                        .help("Check each entry against on-chain proxy state."),
                ),
                Tier::Read,
            ),
            with_globals(
                Command::new("remove")
                    .about("Remove a proxy book entry.")
                    .arg(Arg::new("name").required(true)),
                Tier::Read,
            ),
            with_globals(
                Command::new("update")
                    .about("Update a proxy book entry.")
                    .arg(Arg::new("name").required(true))
                    .arg(Arg::new("address").long("address"))
                    .arg(Arg::new("note").long("note")),
                Tier::Read,
            ),
            with_globals(
                Command::new("clear").about("Clear the local proxy address book."),
                Tier::Read,
            ),
        ])
}

/// Execute a `proxy book` group. Only a nonempty `list --verify` connects.
pub async fn book_run(ctx: &AppContext, paths: &Paths, group: &ArgMatches) -> Result<(), CliError> {
    let (name, leaf) = group
        .subcommand()
        .ok_or_else(|| CliError::input("missing proxy book command"))?;
    if name == "list" && leaf.get_flag("verify") {
        let entries = ProxyBook::new(paths).load().await.map_err(failure)?;
        if !entries.is_empty() {
            return ctx
                .run(|client| async move { book_list(ctx, paths, entries, Some(&client)).await })
                .await;
        }
        return book_list(ctx, paths, entries, None).await;
    }
    book_execute(ctx, paths, group, None).await
}

/// Execute book mutations or verification with an injected, already-open client.
/// Mutations never request a signer or submit an extrinsic.
pub async fn book_execute(
    ctx: &AppContext,
    paths: &Paths,
    group: &ArgMatches,
    client: Option<&Client>,
) -> Result<(), CliError> {
    let (name, leaf) = group
        .subcommand()
        .ok_or_else(|| CliError::input("missing proxy book command"))?;
    let book = ProxyBook::new(paths);
    let name_arg = || text(leaf, "name").ok_or_else(|| CliError::input("missing proxy book name"));
    match name {
        "add" => {
            let mut entry = json!({"name":name_arg()?,"address":text(leaf,"address").ok_or_else(|| CliError::input("missing proxy address"))?,"spawner":text(leaf,"spawner").unwrap_or_default(),"proxy_type":text(leaf,"proxy_type").unwrap_or("Staking"),"note":text(leaf,"note").unwrap_or_default()});
            for field in ["delay", "index", "height", "ext_index"] {
                if let Some(raw) = text(leaf, field) {
                    let value = rtao::primitives::pyfmt::parse_integer(raw).ok_or_else(|| {
                        CliError::input(format!("expected an integer, got {raw:?}"))
                    })?;
                    entry[field] =
                        rtao::primitives::json::from_str(&value.to_string()).map_err(failure)?;
                }
            }
            let stored = book.add(entry).await.map_err(failure)?;
            book_detail(
                ctx,
                "added proxy",
                json!({"entry":stored,"path":book.path()}),
            )
        }
        "list" => {
            let entries = book.load().await.map_err(failure)?;
            if leaf.get_flag("verify") && !entries.is_empty() && client.is_none() {
                return Err(CliError::input("proxy book verification requires a client"));
            }
            book_list(
                ctx,
                paths,
                entries,
                if leaf.get_flag("verify") {
                    client
                } else {
                    None
                },
            )
            .await
        }
        "remove" => {
            let name = name_arg()?;
            let existed = book.remove(name).await.map_err(failure)?;
            book_detail(ctx, "removed proxy", json!({"name":name,"existed":existed}))
        }
        "update" => {
            let name = name_arg()?;
            let updates = ["address", "note"]
                .into_iter()
                .filter_map(|key| text(leaf, key).map(|value| (key.to_owned(), json!(value))))
                .collect();
            let entry = book
                .update(name, updates)
                .await
                .map_err(failure)?
                .ok_or_else(|| failure(format!("proxy {} not found", pyfmt::string_repr(name))))?;
            book_detail(ctx, "updated proxy", entry)
        }
        "clear" => {
            let count = book.clear().await.map_err(failure)?;
            book_detail(
                ctx,
                "cleared proxies",
                json!({"removed":count,"path":book.path()}),
            )
        }
        _ => Err(CliError::input(format!(
            "unknown proxy book command {name:?}"
        ))),
    }
}

fn book_detail(ctx: &AppContext, title: &str, payload: Value) -> Result<(), CliError> {
    let fields = payload
        .as_object()
        .map(|object| {
            object
                .iter()
                .map(|(key, value)| (key.clone(), value.clone()))
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    ctx.output
        .detail(Some(title), &fields, Some(&payload))
        .map_err(failure)
}

async fn book_list(
    ctx: &AppContext,
    paths: &Paths,
    mut entries: Vec<Value>,
    client: Option<&Client>,
) -> Result<(), CliError> {
    if let Some(client) = client {
        for entry in &mut entries {
            let status = book_status(client, entry).await?;
            entry
                .as_object_mut()
                .ok_or_else(|| failure("proxy book entry must be an object"))?
                .insert("status".into(), json!(status));
        }
    }
    let book = ProxyBook::new(paths);
    if ctx.globals.json {
        return ctx
            .output
            .value(&json!({"path":book.path(),"proxies":entries}))
            .map_err(failure);
    }
    let names = ctx.output.address_names().map_err(failure)?;
    let nodes = entries
        .iter()
        .map(|entry| {
            let string = |key| {
                entry
                    .get(key)
                    .and_then(Value::as_str)
                    .filter(|value| !value.is_empty())
            };
            let mut meta = vec![string("proxy_type").unwrap_or("Staking").to_owned()];
            if let Some(delay) = entry.get("delay").filter(|value| public_json_truthy(value)) {
                meta.push(format!("delay {}", crate::globals::config_string(delay)));
            }
            if let Some(spawner) = string("spawner") {
                meta.push(format!(
                    "pure, spawned by {}",
                    names.get(spawner).map(String::as_str).unwrap_or(spawner)
                ));
            }
            let mut leaves = vec![
                string("address").unwrap_or("—").to_owned(),
                meta.join(" · "),
            ];
            for key in ["note", "status"] {
                if let Some(text) = string(key) {
                    leaves.push(text.to_owned());
                }
            }
            (string("name").unwrap_or_default().to_owned(), leaves)
        })
        .collect::<Vec<_>>();
    ctx.output
        .tree(
            &format!("proxy book ({})", book.path().display()),
            &nodes,
            Some(&json!(entries)),
        )
        .map_err(failure)
}

async fn book_status(client: &Client, entry: &Value) -> Result<String, CliError> {
    let Some(address) = entry
        .get("address")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
    else {
        return Ok("no address recorded".into());
    };
    let account = address.parse::<AccountId>().map_err(failure)?;
    let data = rtao::reads::generated::interfaces::proxies(client, account)
        .await
        .map_err(CliError::from)?;
    if data.proxies.is_empty() {
        return Ok("stale: account has no on-chain delegations".into());
    }
    let Some(spawner) = entry
        .get("spawner")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
    else {
        let count = data.proxies.len();
        return Ok(format!(
            "ok: {count} on-chain delegation{}",
            if count == 1 { "" } else { "s" }
        ));
    };
    let wanted = entry
        .get("proxy_type")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .unwrap_or("Staking");
    let mut has_spawner = false;
    for delegation in data.proxies {
        if delegation.delegate.to_string() == spawner {
            has_spawner = true;
            if serde_json::to_value(delegation.proxy_type)
                .map_err(failure)?
                .as_str()
                == Some(wanted)
            {
                return Ok("ok".into());
            }
        }
    }
    Ok(if has_spawner {
        "mismatch: spawner is a delegate but with a different proxy type"
    } else {
        "stale: spawner is not a delegate of this account"
    }
    .into())
}

async fn after_submit(
    ctx: &mut AppContext,
    tx: &TxRequest,
    after: &AfterSubmit,
    receipt: &ExtrinsicResult,
) -> Result<(), CliError> {
    match after {
        AfterSubmit::None => Ok(()),
        AfterSubmit::Kill { name: Some(name) } => {
            context_book(ctx)?.remove(name).await.map_err(failure)?;
            ctx.books
                .proxies
                .retain(|entry| entry.get("name").and_then(Value::as_str) != Some(name));
            ctx.output
                .message(&format!(
                    "removed {} from the proxy book",
                    pyfmt::string_repr(name)
                ))
                .map_err(failure)
        }
        AfterSubmit::Kill { name: None } => Ok(()),
        AfterSubmit::Create { name } => {
            let Some(pure) = receipt
                .data
                .get("pure_proxy")
                .and_then(Value::as_str)
                .filter(|pure| !pure.is_empty())
            else {
                if name.is_some() {
                    ctx.output.message("could not read the spawned address from the PureCreated event — add it manually with `rtao proxy book add`").map_err(failure)?;
                }
                return Ok(());
            };
            if let Some(name) = name {
                let mut entry = json!({"name":name,"address":pure,"spawner":receipt.data.get("spawner").cloned().unwrap_or_else(|| json!("")),
                    "proxy_type":tx.args.get("proxy_type").cloned().unwrap_or_else(|| json!("Staking")),
                    "delay":tx.args.get("delay").cloned().unwrap_or_else(|| json!(0)),
                    "index":tx.args.get("index").cloned().unwrap_or_else(|| json!(0)),"note":""});
                for field in ["height", "ext_index"] {
                    if let Some(value) = receipt.data.get(field).filter(|value| !value.is_null()) {
                        entry[field] = value.clone();
                    }
                }
                let book = context_book(ctx)?;
                book.add(entry.clone()).await.map_err(failure)?;
                ctx.books
                    .proxies
                    .retain(|entry| entry.get("name").and_then(Value::as_str) != Some(name));
                ctx.books.proxies.push(entry);
                ctx.output
                    .message(&format!(
                        "saved to proxy book as {} ({}) — use it with `--proxy-for {name}`",
                        pyfmt::string_repr(name),
                        book.path().display()
                    ))
                    .map_err(failure)
            } else {
                ctx.output.message(&format!("tip: save this pure proxy for later with `rtao proxy book add --name NAME --address {pure}` (or pass --name on create to do it automatically)")).map_err(failure)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        application::command_tree,
        books::Books,
        globals::Globals,
        output::{MemorySink, Output, OutputOptions, Stream},
    };
    use rtao::{
        client::{ClientConfig, Network, config::ConfigFile},
        primitives::CryptoType,
        signer::{
            BoxFuture, DynKeySource, KeyView, KeypairSigner, Signer, SignerError, SignerRole,
        },
    };
    use rtao_testkit::{FakeChain, FakeState, SubmitScript, rpc::RpcClient};
    use std::{
        collections::VecDeque,
        path::{Path, PathBuf},
        sync::{
            Arc, Mutex,
            atomic::{AtomicU64, AtomicUsize, Ordering},
        },
    };

    type TestResult = Result<(), Box<dyn std::error::Error>>;
    const SPAWNER: AccountId = AccountId([3; 32]);
    const PURE: AccountId = AccountId([4; 32]);

    struct TerminalFixture {
        tty: bool,
        replies: VecDeque<String>,
        reads: Arc<AtomicUsize>,
    }
    impl Terminal for TerminalFixture {
        fn stdin_is_tty(&self) -> bool {
            self.tty
        }
        fn stderr_is_tty(&self) -> bool {
            self.tty
        }
        fn read_line(&mut self, _: &str) -> Result<String, PromptError> {
            self.reads.fetch_add(1, Ordering::SeqCst);
            self.replies.pop_front().ok_or(PromptError::Aborted)
        }
        fn read_secret(&mut self, _: &str) -> Result<rtao::wallet::SecretString, PromptError> {
            Err(PromptError::Unavailable)
        }
        fn message(&mut self, _: &str) -> Result<(), PromptError> {
            Ok(())
        }
    }

    fn parse(args: &[&str]) -> Result<ArgMatches, Box<dyn std::error::Error>> {
        Ok(command_tree([command()?]).try_get_matches_from(args)?)
    }

    fn context(
        parsed: &ArgMatches,
        books: Books,
        tty: bool,
        replies: &[&str],
    ) -> Result<(AppContext, MemorySink, Arc<AtomicUsize>), Box<dyn std::error::Error>> {
        let group = parsed.subcommand().ok_or("proxy group")?.1;
        let leaf = group.subcommand().ok_or("proxy command")?.1;
        let config = Map::new();
        let mut globals = Globals::resolve(
            parsed,
            &config,
            Path::new("/isolated/nonexistent-home"),
            |_| None,
        )?;
        globals.apply(leaf, &reserved(group)?)?;
        let reads = Arc::new(AtomicUsize::new(0));
        let mut ctx = AppContext::new(
            globals,
            config,
            books,
            Arc::new(Mutex::new(Box::new(TerminalFixture {
                tty,
                replies: replies.iter().map(|reply| (*reply).to_owned()).collect(),
                reads: reads.clone(),
            }))),
        );
        let sink = MemorySink::default();
        ctx.output = Output::new(
            OutputOptions {
                json_mode: ctx.globals.json,
                quiet: ctx.globals.quiet,
                network: Some("local".into()),
            },
            Arc::new(sink.clone()),
        );
        Ok((ctx, sink, reads))
    }

    fn book() -> Books {
        Books::from_values(
            json!([]),
            json!([{"name":"treasury","address":PURE,"spawner":SPAWNER,
            "proxy_type":"Any","delay":7,"index":9,"height":123,"ext_index":6,"note":"keep exact coordinates"}]),
            json!([]),
        )
    }

    fn transaction(request: &Request) -> Result<&TxRequest, CliError> {
        match &request.kind {
            RequestKind::Transaction { tx, .. } => Ok(tx),
            _ => Err(CliError::failure("expected transaction")),
        }
    }

    #[test]
    fn proxy_parser_preserves_choices_defaults_and_execute_field_ownership() -> TestResult {
        let delegate = SPAWNER.to_string();
        for choice in rtao_intents::proxy::PROXY_TYPES {
            let parsed = parse(&[
                "rtao",
                "proxy",
                "create",
                "--proxy-type",
                choice,
                "--delay",
                "4294967295",
                "--index",
                "65535",
                "--name",
                "treasury",
                "--period",
                "16",
                "--yes",
            ])?;
            let (mut ctx, _, _) = context(&parsed, Books::default(), false, &[])?;
            let request = Request::from_matches(
                &mut ctx,
                parsed.subcommand().ok_or("proxy")?.1,
                &mut PromptHistory::default(),
            )?;
            let tx = transaction(&request)?;
            assert_eq!(tx.op, "create_pure_proxy");
            assert_eq!(tx.args["proxy_type"], choice);
            assert_eq!(tx.args["delay"], u32::MAX);
            assert_eq!(tx.args["index"], u16::MAX);
            assert_eq!(tx.period, Some(16));
            tx.intent()?;
        }
        let parsed = parse(&[
            "rtao",
            "proxy",
            "execute",
            "--delegate",
            &delegate,
            "--real",
            &PURE.to_string(),
            "--inner-op",
            "remove_proxies",
            "--force-proxy-type",
            "Any",
            "--yes",
        ])?;
        let (mut ctx, _, _) = context(&parsed, Books::default(), false, &[])?;
        let request = Request::from_matches(
            &mut ctx,
            parsed.subcommand().ok_or("proxy")?.1,
            &mut PromptHistory::default(),
        )?;
        assert_eq!(transaction(&request)?.args["inner_args"], json!({}));
        assert_eq!(transaction(&request)?.args["force_proxy_type"], "Any");
        assert!(ctx.globals.force_proxy_type.is_none());
        for spelling in ["any", "Staking ", "unknown"] {
            assert!(parse(&["rtao", "proxy", "create", "--proxy-type", spelling]).is_err());
        }
        assert!(parse(&["rtao", "proxy", "add"]).is_err());
        assert!(
            parse(&[
                "rtao",
                "proxy",
                "execute",
                "--delegate",
                &delegate,
                "--real",
                &delegate
            ])
            .is_err()
        );
        Ok(())
    }

    #[test]
    fn kill_uses_book_coordinates_flags_override_zero_and_self_bypasses_book_target() -> TestResult
    {
        let spawner = SPAWNER.to_string();
        let pure = PURE.to_string();
        for target in [None, Some("self"), Some(spawner.as_str())] {
            let mut argv = vec![
                "rtao", "proxy", "kill", "treasury", "--index", "0", "--height", "0", "--yes",
            ];
            if let Some(target) = target {
                argv.extend(["--proxy-for", target]);
            }
            let parsed = parse(&argv)?;
            let (mut ctx, _, reads) = context(&parsed, book(), false, &[])?;
            let request = Request::from_matches(
                &mut ctx,
                parsed.subcommand().ok_or("proxy")?.1,
                &mut PromptHistory::default(),
            )?;
            let tx = transaction(&request)?;
            assert_eq!(tx.args["spawner_ss58"], SPAWNER.to_string());
            assert_eq!(tx.args["proxy_type"], "Any");
            assert_eq!(tx.args["index"], 0);
            assert_eq!(tx.args["height"], 0);
            assert_eq!(tx.args["ext_index"], 6);
            let RequestKind::Transaction { proxy_override, .. } = &request.kind else {
                return Err("not transaction".into());
            };
            assert_eq!(proxy_override.as_deref(), Some(target.unwrap_or(&pure)));
            assert_eq!(reads.load(Ordering::SeqCst), 0);
        }
        let parsed = parse(&["rtao", "proxy", "kill", "missing", "--yes"])?;
        let (mut ctx, _, _) = context(&parsed, book(), false, &[])?;
        assert!(
            Request::from_matches(
                &mut ctx,
                parsed.subcommand().ok_or("proxy")?.1,
                &mut PromptHistory::default()
            )
            .err()
            .ok_or("missing error")?
            .message
            .contains("not found")
        );
        let parsed = parse(&[
            "rtao",
            "proxy",
            "kill",
            "--spawner",
            &SPAWNER.to_string(),
            "--yes",
        ])?;
        let (mut ctx, _, reads) = context(&parsed, book(), false, &[])?;
        let error = Request::from_matches(
            &mut ctx,
            parsed.subcommand().ok_or("proxy")?.1,
            &mut PromptHistory::default(),
        )
        .err()
        .ok_or("coordinates must be required")?;
        assert!(error.message.contains("--height"));
        assert!(error.message.contains("--ext-index"));
        assert_eq!(reads.load(Ordering::SeqCst), 0);
        Ok(())
    }

    #[test]
    fn remove_all_requires_exact_extra_confirmation_and_yes_skips_it() -> TestResult {
        for (reply, succeeds) in [
            ("yes", false),
            ("remove all", true),
            (" REMOVE ALL\u{001f}", true),
        ] {
            let parsed = parse(&["rtao", "proxy", "remove", "--all"])?;
            let (mut ctx, sink, reads) = context(&parsed, Books::default(), true, &[reply])?;
            let request = Request::from_matches(
                &mut ctx,
                parsed.subcommand().ok_or("proxy")?.1,
                &mut PromptHistory::default(),
            );
            assert_eq!(request.is_ok(), succeeds);
            if let Ok(request) = request {
                assert_eq!(transaction(&request)?.op, "remove_proxies");
            }
            assert_eq!(reads.load(Ordering::SeqCst), 1);
            assert!(
                sink.text(Stream::Stderr)?
                    .contains("permanently inaccessible")
            );
        }
        let parsed = parse(&["rtao", "proxy", "remove", "--all", "--yes", "--json"])?;
        let (mut ctx, _, reads) = context(&parsed, Books::default(), true, &[])?;
        let request = Request::from_matches(
            &mut ctx,
            parsed.subcommand().ok_or("proxy")?.1,
            &mut PromptHistory::default(),
        )?;
        assert_eq!(transaction(&request)?.op, "remove_proxies");
        assert_eq!(reads.load(Ordering::SeqCst), 0);
        Ok(())
    }

    #[tokio::test]
    async fn list_keeps_chain_order_exact_json_and_named_human_rows_without_submit() -> TestResult {
        let mut state = FakeState::try_default()?;
        state.seed_python("Proxy", "Proxies", &[json!(PURE)], json!([
            [{"delegate":SPAWNER,"proxy_type":"Any","delay":7}, {"delegate":PURE,"proxy_type":"Staking","delay":0}], 1_000_000_000]))?;
        let chain = FakeChain::from_state(state);
        let client = client(chain.clone()).await?;
        for json_mode in [false, true] {
            let mut args = vec!["rtao", "proxy", "list", "--coldkey", "treasury"];
            if json_mode {
                args.push("--json");
            }
            let parsed = parse(&args)?;
            let (mut ctx, sink, reads) = context(&parsed, book(), false, &[])?;
            let request = Request::from_matches(
                &mut ctx,
                parsed.subcommand().ok_or("proxy")?.1,
                &mut PromptHistory::default(),
            )?;
            execute(
                &mut ctx,
                &client,
                &request,
                SubmissionOptions::default(),
                None,
                &mut PromptHistory::default(),
            )
            .await?;
            let stdout = sink.text(Stream::Stdout)?;
            if json_mode {
                assert_eq!(
                    serde_json::from_str::<Value>(&stdout)?,
                    json!({"account":PURE,"proxies":[
                    {"delegate":SPAWNER,"proxy_type":"Any","delay":7}, {"delegate":PURE,"proxy_type":"Staking","delay":0}],"deposit":"τ1.000000000"})
                );
            } else {
                assert!(stdout.contains("on-chain proxies — treasury"));
                assert!(stdout.contains("deposit τ1.000000000"));
                assert!(
                    stdout.find("Any").ok_or("Any")? < stdout.find("Staking").ok_or("Staking")?
                );
            }
            assert_eq!(reads.load(Ordering::SeqCst), 0);
        }
        assert!(chain.mutate_state(|state| state.submissions.is_empty()));
        client.close().await;
        Ok(())
    }

    async fn client(chain: FakeChain) -> Result<Client, Box<dyn std::error::Error>> {
        let mut config = ClientConfig::new(Network::resolve_with("local", |_| None)?)?;
        config.track_heads = false;
        config.runtime_cache_dir = None;
        config.config_store = None;
        Ok(Client::from_rpc(RpcClient::new(chain), config).await?)
    }

    #[derive(Clone)]
    struct Keys {
        key: KeypairSigner,
        unlocks: Arc<AtomicUsize>,
    }
    impl KeyView for Keys {
        fn coldkey_address(&self) -> Result<AccountId, SignerError> {
            Ok(self.key.account())
        }
        fn hotkey_address(&self) -> Result<AccountId, SignerError> {
            Ok(self.key.account())
        }
        fn hotkey_public_key(&self) -> Result<[u8; 32], SignerError> {
            Ok(self.key.account().0)
        }
    }
    impl DynKeySource for Keys {
        fn signer_for<'a>(
            &'a self,
            _: SignerRole,
        ) -> BoxFuture<'a, Result<Arc<dyn Signer>, SignerError>> {
            Box::pin(async move {
                self.unlocks.fetch_add(1, Ordering::SeqCst);
                Ok(Arc::new(self.key.clone()) as Arc<dyn Signer>)
            })
        }
    }
    struct Factory(Keys);
    impl ExternalSignerFactory for Factory {
        fn keys<'a>(
            &'a self,
            _: &'a Globals,
            role: SignerRole,
        ) -> BoxFuture<'a, Result<Arc<dyn DynKeySource>, CliError>> {
            assert_eq!(role, SignerRole::Coldkey);
            Box::pin(async move { Ok(Arc::new(self.0.clone()) as Arc<dyn DynKeySource>) })
        }
    }

    struct Files {
        root: PathBuf,
        paths: Paths,
    }
    impl Files {
        fn new() -> Result<Self, Box<dyn std::error::Error>> {
            static NEXT: AtomicU64 = AtomicU64::new(0);
            let base = std::env::var_os("RTAO_TEMP_ROOT")
                .map(PathBuf::from)
                .unwrap_or_else(|| {
                    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/m5-proxy-tests")
                });
            let root = base.join("tmp").join(format!(
                "proxy-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            std::fs::create_dir_all(&root)?;
            let paths = Paths::new(&root, |name| {
                (name == "BTCLI_PROXIES_PATH").then(|| root.join("proxies.json").into_os_string())
            });
            Ok(Self { root, paths })
        }
    }
    impl Drop for Files {
        fn drop(&mut self) {
            // All awaited book workers have finished; this exact fixture owns
            // public JSON only and has no service/config/venv dependencies.
            let _ = std::fs::remove_dir_all(&self.root);
        }
    }

    #[tokio::test]
    async fn pure_lifecycle_persists_event_coordinates_only_after_actual_success() -> TestResult {
        let files = Files::new()?;
        let keys = Keys {
            key: KeypairSigner::from_seed(&[19; 32], CryptoType::Ed25519)?,
            unlocks: Arc::new(AtomicUsize::new(0)),
        };
        let mut state = FakeState::try_default()?;
        state.seed_python(
            "System",
            "Account",
            &[json!(keys.key.account())],
            json!({"data":{"free":10_000_000_000u64}}),
        )?;
        state.queue_submit(SubmitScript::InBlockThenFinalized { events: json!([
            {"phase":"ApplyExtrinsic","extrinsic_idx":0,"module_id":"System","event_id":"ExtrinsicSuccess","attributes":{"dispatch_info":{"weight":{"ref_time":1000,"proof_size":0},"class":"Normal","pays_fee":"Yes"}},"topics":[]},
            {"phase":"ApplyExtrinsic","extrinsic_idx":0,"module_id":"Proxy","event_id":"PureCreated","attributes":{"pure":PURE,"who":keys.key.account(),"proxy_type":"Any","disambiguation_index":9},"topics":[]}
        ]) });
        let chain = FakeChain::from_state(state);
        let client = client(chain.clone()).await?;
        let parsed = parse(&[
            "rtao",
            "proxy",
            "create",
            "--name",
            "treasury",
            "--proxy-type",
            "Any",
            "--delay",
            "7",
            "--index",
            "9",
            "--signer",
            "vault",
            "--yes",
            "--json",
            "--no-mev-shield",
        ])?;
        let (mut ctx, sink, _) = context(&parsed, Books::default(), false, &[])?;
        ctx.config_store = Some(rtao::client::config::Store::new(files.paths.clone()));
        let request = Request::from_matches(
            &mut ctx,
            parsed.subcommand().ok_or("proxy")?.1,
            &mut PromptHistory::default(),
        )?;
        ctx.globals.dry_run = true;
        let factory = Factory(keys.clone());
        assert!(
            execute(
                &mut ctx,
                &client,
                &request,
                SubmissionOptions::default(),
                Some(&factory),
                &mut PromptHistory::default()
            )
            .await?
            .is_none()
        );
        assert!(!files.paths.path(ConfigFile::Proxies).exists());
        assert_eq!(keys.unlocks.load(Ordering::SeqCst), 0);
        ctx.globals.dry_run = false;
        let receipt = execute(
            &mut ctx,
            &client,
            &request,
            SubmissionOptions::default(),
            Some(&factory),
            &mut PromptHistory::default(),
        )
        .await?
        .ok_or("create receipt")?;
        assert!(receipt.success);
        assert_eq!(keys.unlocks.load(Ordering::SeqCst), 1);
        // A normal success proves execution for threshold one, but an ordinary
        // approval success cannot retire a threshold-two pure-account entry.
        ctx.books.multisigs = vec![json!({"name":ctx.globals.wallet_name,"threshold":1})];
        assert!(kill_dispatched(&ctx, &receipt));
        ctx.books.multisigs[0]["threshold"] = json!(2);
        assert!(!kill_dispatched(&ctx, &receipt));
        ctx.books.multisigs.clear();
        let stored: Value =
            serde_json::from_slice(&std::fs::read(files.paths.path(ConfigFile::Proxies))?)?;
        assert_eq!(stored[0]["address"], PURE.to_string());
        assert_eq!(stored[0]["spawner"], keys.key.account().to_string());
        assert_eq!(stored[0]["proxy_type"], "Any");
        assert_eq!(stored[0]["delay"], 7);
        assert_eq!(stored[0]["index"], 9);
        assert_eq!(stored[0]["height"], receipt.data["height"]);
        assert_eq!(stored[0]["ext_index"], receipt.data["ext_index"]);
        assert_eq!(
            ctx.books.proxies,
            stored.as_array().ok_or("book array")?.clone()
        );
        assert!(sink.text(Stream::Stderr)?.is_empty());
        let parsed = parse(&[
            "rtao",
            "proxy",
            "kill",
            "treasury",
            "--signer",
            "vault",
            "--yes",
            "--json",
            "--no-mev-shield",
        ])?;
        let kill = Request::from_matches(
            &mut ctx,
            parsed.subcommand().ok_or("proxy")?.1,
            &mut PromptHistory::default(),
        )?;
        chain.mutate_state(|state| {
            state.queue_submit(SubmitScript::Invalid("fixture refusal".into()))
        });
        let failed = execute(
            &mut ctx,
            &client,
            &kill,
            SubmissionOptions::default(),
            Some(&factory),
            &mut PromptHistory::default(),
        )
        .await;
        assert!(
            failed.is_err()
                || failed
                    .as_ref()
                    .is_ok_and(|receipt| receipt.as_ref().is_some_and(|receipt| !receipt.success))
        );
        assert_eq!(
            serde_json::from_slice::<Value>(&std::fs::read(
                files.paths.path(ConfigFile::Proxies)
            )?)?,
            stored
        );
        assert!(ctx.books.proxy("treasury").is_some());
        // The fake runtime accepts the next call; no transfer is injected and
        // only successful kill removes the public creation coordinates.
        let killed = execute(
            &mut ctx,
            &client,
            &kill,
            SubmissionOptions::default(),
            Some(&factory),
            &mut PromptHistory::default(),
        )
        .await?
        .ok_or("kill receipt")?;
        assert!(killed.success);
        assert_eq!(
            serde_json::from_slice::<Value>(&std::fs::read(
                files.paths.path(ConfigFile::Proxies)
            )?)?,
            json!([])
        );
        assert!(ctx.books.proxy("treasury").is_none());
        assert!(ctx.globals.proxy_for.is_none());
        client.close().await;
        Ok(())
    }
}
