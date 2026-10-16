// Derived from RaoFoundation/subtensor sdk/python/bittensor/cli/tx.py @ 850f2755bb05a3851f948489f0ab27b0645cebce (Apache-2.0); modified by rtao.
// Derived from RaoFoundation/subtensor sdk/python/bittensor/cli/query.py @ 850f2755bb05a3851f948489f0ab27b0645cebce (Apache-2.0); modified by rtao.
// Derived from RaoFoundation/subtensor sdk/python/bittensor/cli/stake_picker.py @ 850f2755bb05a3851f948489f0ab27b0645cebce (Apache-2.0); modified by rtao.
// Derived from RaoFoundation/subtensor sdk/python/bittensor/cli/intent_prompts.py @ 850f2755bb05a3851f948489f0ab27b0645cebce (Apache-2.0); modified by rtao.
//! Real schema-built command dispatch over implemented SDK intents and reads.

use super::{
    catalog::RustTool,
    schema::{self, Field},
};
use crate::{
    books::{FsAccountLookup, NameResolver},
    call_names::{report_resolved_account, resolve_intent_args_with},
    context::{AppContext, ExternalSignerFactory, SubmissionOptions},
    error::CliError,
    globals::{Tier, with_globals},
    intent_prompts::{apply_intent_prompt_policy, validate_intent_prompt_policy},
    output::Output,
    prompt::{
        PromptError, PromptFlow, PromptHistory, PromptResolver, PromptSpec, PromptValidation,
        Terminal, WalletConfirmation, ask_with, fill_missing,
    },
};
use clap::{Arg, ArgAction, ArgMatches, Command};
use rtao::{
    client::{BlockRef, Client},
    reads::{ReadName, RenderHint},
    signer::DynKeySource,
    tx::ExtrinsicResult,
};
use rtao_catalog::IntentSpec;
use rtao_intents::{AnyIntent, InterfaceMetadata};
use serde_json::{Map, Value, json};
use std::collections::BTreeSet;

fn input(error: impl std::fmt::Display) -> CliError {
    CliError::input(error.to_string())
}
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
fn panel(spec: &IntentSpec) -> String {
    match spec.origin {
        "root" => "Root (chain sudo)".into(),
        "subnet_owner" => format!(
            "{} — subnet owner",
            spec.wraps.first().map(|p| p.0).unwrap_or("Other")
        ),
        _ => spec.wraps.first().map(|p| p.0).unwrap_or("Other").into(),
    }
}
fn metadata(op: &str) -> Option<InterfaceMetadata> {
    rtao_intents::interface_metadata()
        .into_iter()
        .find(|row| row.name == op)
}
fn intent(op: &str) -> Result<&'static IntentSpec, CliError> {
    rtao_catalog::INTENTS
        .iter()
        .find(|spec| spec.name == op)
        .ok_or_else(|| CliError::input(format!("unknown transaction {op:?}")))
}

/// All compiled interfaces carry real Rust type/Role/Output metadata; CLI is
/// present only for behavior emitted by the actual implementation generator.
pub fn rust_tools() -> Result<Vec<RustTool>, CliError> {
    rtao_intents::interface_metadata()
        .into_iter()
        .filter(|row| row.compiled)
        .map(|row| {
            Ok(RustTool {
                name: row.name.into(),
                type_name: row
                    .type_name
                    .ok_or_else(|| failure("compiled intent type metadata missing"))?
                    .into(),
                role: row
                    .role
                    .ok_or_else(|| failure("compiled intent role metadata missing"))?
                    .into(),
                output: row
                    .output
                    .ok_or_else(|| failure("compiled intent output metadata missing"))?
                    .into(),
                cli: row
                    .implemented
                    .then(|| format!("rtao tx {}", row.name.replace('_', "-"))),
            })
        })
        .collect()
}

/// Build one generated semantic command; unavailable or pending bodies refuse.
pub fn intent_command(op: &str) -> Result<Command, CliError> {
    let spec = intent(op)?;
    if !metadata(op).is_some_and(|row| row.compiled && row.implemented) {
        return Err(CliError::input(format!(
            "transaction {op:?} is unavailable in this binary"
        )));
    }
    let fields = schema::intent_fields(spec)?;
    let mut description = spec.description.to_owned();
    if spec.origin == "root" {
        description.push_str("\n\nRequires: the chain sudo key; the call is wrapped in Sudo.sudo.");
    }
    if spec.origin == "subnet_owner" {
        description.push_str("\n\nRequires: the subnet's owner coldkey.");
    }
    if let Some(verify) = spec.verify {
        description.push_str(&format!(
            "\n\nVerify afterwards: rtao query {}",
            verify.replace('_', "-")
        ));
    }
    description.push_str(&format!(
        "\n\nDocs: {}/tx/{}",
        rtao_catalog::py::DOCS_URL,
        spec.name.replace('_', "-")
    ));
    let mut command = schema::arguments(
        Command::new(spec.name.replace('_', "-"))
            .about(spec.summary)
            .long_about(description)
            .next_help_heading("Arguments"),
        &fields,
        true,
    );
    if !spec.all_amount_fields.is_empty() {
        command = command.arg(
            Arg::new("__all_amount")
                .long("all")
                .action(ArgAction::SetTrue)
                .help("Use the entire available amount, read from chain state at build time."),
        );
    }
    if shared_netuid(spec) {
        command = command.arg(
            Arg::new("__shared_netuid")
                .long("netuid")
                .value_parser(clap::value_parser!(u16))
                .help(
                    "Use this subnet as both origin and destination; explicit fields override it.",
                ),
        );
    }
    if !fields.iter().any(|field| field.name == "period") {
        command = command.arg(
            Arg::new("__period")
                .long("period")
                .value_parser(clap::value_parser!(u32))
                .help("Mortal transaction period in blocks (rtao addition)."),
        );
    }
    Ok(with_globals(command, Tier::Tx))
}

/// Build transactions from real feature/pending metadata, grouped as in Python.
pub fn tx_command() -> Result<Command, CliError> {
    let mut specs = rtao_intents::interface_metadata()
        .into_iter()
        .filter(|row| row.compiled && row.implemented)
        .map(|row| intent(row.name))
        .collect::<Result<Vec<_>, _>>()?;
    specs.sort_by_key(|spec| (panel(spec), spec.name));
    let mut command = Command::new("tx")
        .about("Submit transactions (generated from intents).")
        .subcommand_required(true)
        .arg_required_else_help(true);
    let help = grouped_help(
        "Submit transactions (generated from intents).",
        "rtao tx <COMMAND>",
        specs
            .iter()
            .map(|spec| (panel(spec), spec.name.replace('_', "-"), spec.summary)),
    );
    for spec in specs {
        command = command.subcommand(intent_command(spec.name)?);
    }
    Ok(command.override_help(help))
}

/// Build a real read command and D014's pinned-block option.
pub fn read_command(name: ReadName) -> Result<Command, CliError> {
    if !name.implemented() {
        return Err(CliError::input(format!("query {name} is not implemented")));
    }
    let spec = name.catalog_spec();
    let fields = schema::read_fields(name)?;
    let mut command = schema::arguments(
        Command::new(spec.name.replace('_', "-"))
            .about(spec.summary)
            .long_about(format!(
                "{}\n\nDocs: {}/query/{}",
                spec.description,
                rtao_catalog::py::DOCS_URL,
                spec.name.replace('_', "-")
            ))
            .next_help_heading("Arguments"),
        &fields,
        false,
    );
    if !fields.iter().any(|field| field.name == "block") {
        command = command.arg(
            Arg::new("__block")
                .long("block")
                .value_parser(clap::value_parser!(u64))
                .help("Read every item at this canonical block height (rtao addition)."),
        );
    }
    Ok(with_globals(command, Tier::Read))
}

/// Build queries from the same implementation table as actual read dispatch.
pub fn query_command() -> Result<Command, CliError> {
    let mut names = ReadName::ALL
        .into_iter()
        .filter(|name| name.implemented())
        .collect::<Vec<_>>();
    names.sort_by_key(|name| (name.catalog_spec().category, name.as_str()));
    let mut command = Command::new("query")
        .about("Query chain state (generated from reads).")
        .subcommand_required(true)
        .arg_required_else_help(true);
    let help = grouped_help(
        "Query chain state (generated from reads).",
        "rtao query <COMMAND>",
        names.iter().map(|name| {
            (
                name.catalog_spec().category.into(),
                name.as_str().replace('_', "-"),
                name.catalog_spec().summary,
            )
        }),
    );
    for name in names {
        command = command.subcommand(read_command(name)?);
    }
    Ok(command.override_help(help))
}

fn grouped_help<'a>(
    title: &str,
    usage: &str,
    rows: impl IntoIterator<Item = (String, String, &'a str)>,
) -> String {
    let mut text = format!("{title}\n\nUsage: {usage}\n");
    let mut previous = String::new();
    for (panel, name, summary) in rows {
        if previous != panel {
            text.push_str(&format!("\n{panel}:\n"));
            previous = panel;
        }
        text.push_str(&format!("  {name:30} {summary}\n"));
    }
    text.push_str("\nOptions:\n  -h, --help  Print help\n");
    text
}

/// Catalog fields are reserved from leaf-global overrides, including collisions.
pub fn intent_reserved(op: &str) -> Result<Vec<&'static str>, CliError> {
    Ok(intent(op)?.fields.iter().map(|field| field.name).collect())
}
/// Read parameters reserve matching global identifiers.
pub fn read_reserved(name: ReadName) -> Vec<&'static str> {
    name.catalog_spec()
        .params
        .iter()
        .map(|field| field.name)
        .collect()
}
fn shared_netuid(spec: &IntentSpec) -> bool {
    spec.fields.iter().any(|f| f.name == "origin_netuid")
        && spec.fields.iter().any(|f| f.name == "dest_netuid")
        && !spec.fields.iter().any(|f| f.name == "netuid")
        && !matches!(spec.name, "swap_stake" | "move_swap_stake")
}

struct ArgumentResolver<'a> {
    ctx: &'a AppContext,
    fields: &'a [Field],
    balance_hint: Option<&'a str>,
}
impl PromptResolver for ArgumentResolver<'_> {
    fn decorate(&mut self, spec: &mut PromptSpec) -> Result<(), PromptError> {
        let hint = self.balance_hint.ok_or_else(|| {
            PromptError::InvalidValue("this amount prompt needs a connected client".into())
        })?;
        spec.help = Some(match &spec.help {
            Some(help) => format!("{hint}\n{help}"),
            None => hint.into(),
        });
        Ok(())
    }
    fn parse(&mut self, spec: &PromptSpec, raw: &str) -> Result<Value, PromptValidation> {
        let field = self
            .fields
            .iter()
            .find(|field| field.name == spec.field)
            .ok_or_else(|| PromptValidation::Message(format!("unknown field {}", spec.field)))?;
        let value = schema::coerce(field, raw).map_err(|e| PromptValidation::Message(e.message))?;
        if field.name.ends_with("_ss58") {
            return self
                .ctx
                .resolve_address(field.name, Some(raw))
                .map(|value| json!(value))
                .map_err(|e| PromptValidation::Message(e.message));
        }
        Ok(value)
    }
    fn custom(
        &mut self,
        spec: &PromptSpec,
        terminal: &mut dyn Terminal,
        answers: &mut Map<String, Value>,
    ) -> Result<Vec<String>, PromptError> {
        let candidates = super::stake::picker::local_candidates(self.ctx)
            .map_err(|error| PromptError::InvalidValue(error.to_string()))?;
        let mut choices = Vec::<(String, String)>::new();
        let mut seen = BTreeSet::new();
        if spec.flow == PromptFlow::DestinationAccount {
            for entry in &self.ctx.books.addresses {
                if let (Some(name), Some(address)) =
                    (entry["name"].as_str(), entry["address"].as_str())
                    && !address.is_empty()
                    && seen.insert(address.to_owned())
                {
                    choices.push((name.into(), address.into()));
                }
            }
            for candidate in candidates.into_iter().filter(|candidate| {
                candidate.source == "wallet" && candidate.name != self.ctx.globals.wallet_name
            }) {
                if seen.insert(candidate.address.clone()) {
                    choices.push((candidate.name, candidate.address));
                }
            }
        } else {
            // A named hotkey is a choice even when another key file carries
            // the same public account. Python numbers key names, not accounts.
            for candidate in candidates.into_iter().filter(|candidate| {
                candidate.source == "hotkey"
                    && candidate
                        .name
                        .starts_with(&format!("{}/", self.ctx.globals.wallet_name))
            }) {
                choices.push((candidate.name, candidate.address));
            }
        }
        for (index, (name, address)) in choices.iter().enumerate() {
            terminal.message(&format!("{}  {name}  {address}", index + 1))?;
        }
        let (value, _) = ask_with(&self.ctx.prompt_context(), terminal, spec, |raw| {
            let numbered = !raw.is_empty()
                && raw.chars().all(|character| {
                    unicode_general_category::get_general_category(character)
                        == unicode_general_category::GeneralCategory::DecimalNumber
                });
            let selected = if numbered && !choices.is_empty() {
                Some(
                    rtao::primitives::pyfmt::parse_integer(raw)
                        .and_then(|value| value.to_string().parse::<usize>().ok())
                        .and_then(|index| index.checked_sub(1))
                        .and_then(|index| choices.get(index))
                        .ok_or_else(|| {
                            PromptValidation::Message(format!(
                                "enter a number between 1 and {}",
                                choices.len()
                            ))
                        })?,
                )
            } else {
                None
            };
            if let Some((name, address)) = selected {
                // Validate before caching the selected key's name. Normal
                // resolution may find a contact alias for the same account;
                // Python records the numeric menu choice first.
                address
                    .parse::<rtao::primitives::AccountId>()
                    .map_err(|error| PromptValidation::Message(error.to_string()))?;
                self.ctx
                    .output
                    .name_address(address, name)
                    .map_err(|error| PromptValidation::Message(error.to_string()))?;
            }
            self.parse(spec, selected.map_or(raw, |(_, address)| address.as_str()))
        })?;
        let text = value
            .as_str()
            .ok_or_else(|| PromptError::InvalidValue("selected account is not text".into()))?
            .to_owned();
        answers.insert(spec.field.clone(), value);
        Ok(vec![spec.flag.clone(), text])
    }
}

fn required(fields: &[Field], answers: &Map<String, Value>) -> Vec<PromptSpec> {
    fields
        .iter()
        .filter(|field| field.required && !answers.get(field.name).is_some_and(|v| !v.is_null()))
        .map(|field| {
            let mut prompt = PromptSpec::new(field.name, field.flag());
            prompt.help = Some(field.help.clone());
            if field.query {
                prompt.placeholder = match field.kind.as_str() {
                    "integer" => Some("integer"),
                    "number" => Some("number"),
                    "boolean" => Some("y/n"),
                    "array" => Some("comma-separated"),
                    _ => None,
                }
                .map(str::to_owned);
            }
            if field.name.ends_with("_ss58") {
                prompt.flow = if field.name.contains("hotkey") {
                    PromptFlow::StakeTarget
                } else {
                    PromptFlow::DestinationAccount
                };
            }
            prompt
        })
        .collect()
}
fn names(
    ctx: &AppContext,
    fields: &[Field],
    mut answers: Map<String, Value>,
) -> Result<Map<String, Value>, CliError> {
    for field in fields.iter().filter(|field| field.name.ends_with("_ss58")) {
        let raw = answers
            .get(field.name)
            .filter(|v| !v.is_null())
            .and_then(Value::as_str);
        if raw.is_none() && field.preserve_null() {
            continue;
        }
        if let Some(address) = ctx
            .resolve_address(field.name, raw)
            .map_err(|e| CliError { code: 2, ..e })?
        {
            answers.insert(field.name.into(), json!(address));
        }
    }
    for field in fields.iter().filter(|field| field.name.ends_with("_ss58s")) {
        let Some(value) = answers.get(field.name).filter(|value| !value.is_null()) else {
            continue;
        };
        let values = value
            .as_array()
            .ok_or_else(|| input(format!("{} requires a list of accounts", field.flag())))?;
        // Resolve each explicit entry using its scalar account kind. In
        // particular, a hotkey name must retain hotkey lookup semantics.
        let scalar = field
            .name
            .strip_suffix('s')
            .ok_or_else(|| failure(format!("invalid account-list field {:?}", field.name)))?;
        let addresses = values
            .iter()
            .enumerate()
            .map(|(index, value)| {
                let raw = value.as_str().ok_or_else(|| {
                    input(format!(
                        "{} account {} must be text",
                        field.flag(),
                        index + 1
                    ))
                })?;
                ctx.resolve_address(scalar, Some(raw))
                    .map_err(|error| CliError { code: 2, ..error })?
                    .map(|address| json!(address))
                    .ok_or_else(|| input(format!("cannot resolve {}", field.flag())))
            })
            .collect::<Result<Vec<_>, CliError>>()?;
        answers.insert(field.name.into(), Value::Array(addresses));
    }
    let lookup = FsAccountLookup::new(ctx.globals.wallet_path.clone()).map_err(input)?;
    let resolver = NameResolver::new(
        &ctx.books,
        &lookup,
        &ctx.globals.wallet_name,
        &ctx.globals.hotkey_name,
    );
    let mut output_error = None;
    let result = resolve_intent_args_with(&resolver, &answers, None, &mut |param, resolved| {
        if output_error.is_none() {
            output_error = report_resolved_account(&ctx.output, param, resolved).err();
        }
    });
    if let Some(error) = output_error {
        return Err(failure(error));
    }
    result.map_err(input)
}

/// Parsed semantic request; replay and execution options remain separate from args.
pub struct TxRequest {
    /// Authenticated catalog operation.
    pub op: &'static str,
    /// Exact typed tool arguments with account names resolved recursively.
    pub args: Map<String, Value>,
    /// Explicit D030 period override in blocks.
    pub period: Option<u32>,
}
impl TxRequest {
    /// Parse supplied args, prompt missing fields, and preserve native money units.
    pub fn from_matches(
        ctx: &mut AppContext,
        op: &str,
        matches: &ArgMatches,
        history: &mut PromptHistory,
    ) -> Result<Self, CliError> {
        let mut prepared = Self::prepare_matches(ctx, op, matches, history)?;
        let missing = prepared.missing.clone();
        prepared.prompt(ctx, &missing, None, history)?;
        prepared.finish(ctx)
    }
    /// Parse with live prompt decorations from the same connected SDK client.
    pub async fn from_matches_with_client(
        ctx: &mut AppContext,
        client: &Client,
        op: &str,
        matches: &ArgMatches,
        history: &mut PromptHistory,
    ) -> Result<Self, CliError> {
        let mut prepared = Self::prepare_matches(ctx, op, matches, history)?;
        for prompt in prepared.missing.clone() {
            if crate::prompt::supplied(&prepared.answers, &prompt.field) {
                continue;
            }
            match &prompt.flow {
                PromptFlow::StakeSource { netuid_field } => {
                    super::stake::picker::source(
                        ctx,
                        client,
                        &prompt.field,
                        netuid_field.as_deref(),
                        &mut prepared.answers,
                        history,
                    )
                    .await?;
                    continue;
                }
                PromptFlow::StakeTarget => {
                    super::stake::picker::target(
                        ctx,
                        &prompt.field,
                        &mut prepared.answers,
                        history,
                    )?;
                    continue;
                }
                PromptFlow::DestinationAccount => {
                    super::stake::picker::destination(
                        ctx,
                        &prompt.field,
                        &mut prepared.answers,
                        history,
                    )?;
                    continue;
                }
                PromptFlow::ClaimRootSource => {
                    super::stake::picker::claim_root_source(
                        ctx,
                        client,
                        &prompt.field,
                        &mut prepared.answers,
                        history,
                    )
                    .await?;
                    continue;
                }
                _ => {}
            }
            let hint = if prompt.free_balance {
                let owner = match ctx.dispatch_proxy(None)? {
                    Some(owner) => owner,
                    None => {
                        let account = if ctx.globals.uses_external_signer()
                            && ctx.books.multisig(&ctx.globals.wallet_name).is_none()
                        {
                            ctx.external_signer_public_account().await?
                        } else {
                            ctx.resolve_address("coldkey_ss58", None)?
                        };
                        account.ok_or_else(|| input("amount prompt needs a public coldkey"))?
                    }
                };
                let free = rtao::reads::generated::interfaces::balance(client, owner)
                    .await
                    .map_err(failure)?;
                Some(format!("free balance of {owner}: {free}"))
            } else {
                None
            };
            prepared.prompt(ctx, &[prompt], hint.as_deref(), history)?;
        }
        prepared.finish(ctx)
    }
    fn prepare_matches(
        ctx: &mut AppContext,
        op: &str,
        matches: &ArgMatches,
        history: &mut PromptHistory,
    ) -> Result<PreparedTx, CliError> {
        let spec = intent(op)?;
        if !metadata(op).is_some_and(|row| row.compiled && row.implemented) {
            return Err(CliError::input("transaction behavior is unavailable"));
        }
        let fields = schema::intent_fields(spec)?;
        let mut answers = schema::values(matches, &fields)?;
        if matches
            .try_get_one::<bool>("__all_amount")
            .ok()
            .flatten()
            .copied()
            .unwrap_or(false)
        {
            for name in spec.all_amount_fields {
                if answers
                    .get(*name)
                    .filter(|v| !v.is_null())
                    .is_some_and(|v| {
                        v.as_str().is_none_or(|s| {
                            let text = crate::prompt::python_strip(s);
                            text != "all" && !text.is_empty()
                        })
                    })
                {
                    return Err(CliError::input(format!(
                        "pass --{} or --all, not both",
                        name.replace('_', "-")
                    )));
                }
                answers.insert((*name).into(), json!("all"));
            }
        }
        if let Some(netuid) = matches.try_get_one::<u16>("__shared_netuid").ok().flatten() {
            for name in ["origin_netuid", "dest_netuid"] {
                if !answers.get(name).is_some_and(|v| !v.is_null()) {
                    answers.insert(name.into(), json!(netuid));
                }
            }
        }
        let context = ctx.prompt_context();
        let mut terminal = ctx.terminal.clone();
        let missing = apply_intent_prompt_policy(
            &context,
            &mut terminal,
            spec.name,
            required(&fields, &answers),
            &answers,
        )
        .map_err(prompt_error)?;
        ctx.confirm_wallet(
            &WalletConfirmation {
                require_coldkey: spec.signer == "coldkey",
                hotkey_help: (spec.signer == "hotkey")
                    .then(|| "Hotkey signing this transaction.".into()),
                ..Default::default()
            },
            history,
        )?;
        let period = matches
            .try_get_one::<u32>("__period")
            .ok()
            .flatten()
            .copied();
        Ok(PreparedTx {
            spec,
            fields,
            answers,
            missing,
            period,
        })
    }
    /// Construct the real generated interface and validate its native fields.
    pub fn intent(&self) -> Result<AnyIntent, CliError> {
        AnyIntent::from_tool(self.op, Value::Object(self.args.clone())).map_err(input)
    }
}

struct PreparedTx {
    spec: &'static IntentSpec,
    fields: Vec<Field>,
    answers: Map<String, Value>,
    missing: Vec<PromptSpec>,
    period: Option<u32>,
}
impl PreparedTx {
    fn prompt(
        &mut self,
        ctx: &AppContext,
        missing: &[PromptSpec],
        balance_hint: Option<&str>,
        history: &mut PromptHistory,
    ) -> Result<(), CliError> {
        let mut terminal = ctx.terminal.clone();
        fill_missing(
            &ctx.prompt_context(),
            &mut terminal,
            missing,
            &mut self.answers,
            &mut ArgumentResolver {
                ctx,
                fields: &self.fields,
                balance_hint,
            },
            history,
        )
        .map_err(prompt_error)
    }
    fn finish(self, ctx: &AppContext) -> Result<TxRequest, CliError> {
        validate_intent_prompt_policy(self.spec.name, &self.answers).map_err(|error| {
            prompt_error(error).with_help("pass it explicitly, or run on a terminal to pick one")
        })?;
        let mut answers = names(ctx, &self.fields, self.answers)?;
        answers.retain(|_, value| !value.is_null());
        let request = TxRequest {
            op: self.spec.name,
            args: answers,
            period: self.period,
        };
        request.intent()?;
        Ok(request)
    }
}

/// Execute a real generated intent through the application's semantic route.
/// Actual PoW submissions can prepare public work before strict SDK planning;
/// dry runs never mine and retain the SDK's challenge-validation failures.
pub async fn execute_tx(
    ctx: &mut AppContext,
    client: &Client,
    request: &TxRequest,
    options: SubmissionOptions<'_>,
    external: Option<&dyn ExternalSignerFactory>,
    history: &mut PromptHistory,
) -> Result<Option<ExtrinsicResult>, CliError> {
    let intent = request.intent()?;
    let execution = super::pow::Execution::new(&intent, &ctx.output);
    let options = SubmissionOptions {
        period: request.period.or(options.period),
        execution: execution
            .as_ref()
            .map(|e| e as &dyn crate::context::SubmissionExecution)
            .or(options.execution),
        ..options
    };
    ctx.submit_with_options(client, intent.as_dyn(), options, external, history)
        .await
}
/// Inject a real lazy key source while retaining actual SDK plan/submit behavior.
pub async fn execute_tx_with_keys(
    ctx: &AppContext,
    client: &Client,
    request: &TxRequest,
    keys: &dyn DynKeySource,
    options: SubmissionOptions<'_>,
) -> Result<Option<ExtrinsicResult>, CliError> {
    let intent = request.intent()?;
    let execution = super::pow::Execution::new(&intent, &ctx.output);
    ctx.submit_with_keys_options(
        client,
        intent.as_dyn(),
        keys,
        SubmissionOptions {
            period: request.period.or(options.period),
            execution: execution
                .as_ref()
                .map(|e| e as &dyn crate::context::SubmissionExecution)
                .or(options.execution),
            ..options
        },
    )
    .await
}

/// Parsed read request, including an optional actual historical block identity.
pub struct QueryRequest {
    /// Catalog read selected by this request.
    pub name: ReadName,
    /// Defaulted and recursively name-resolved params.
    pub params: Map<String, Value>,
    /// D014 canonical block height, pinned once by execution.
    pub block: Option<u64>,
}
impl QueryRequest {
    /// Resolve source defaults and required inputs without acquiring any signer.
    pub fn from_matches(
        ctx: &AppContext,
        name: ReadName,
        matches: &ArgMatches,
        history: &mut PromptHistory,
    ) -> Result<Self, CliError> {
        if !name.implemented() {
            return Err(CliError::input("read behavior is unavailable"));
        }
        let fields = schema::read_fields(name)?;
        let mut params = schema::values(matches, &fields)?;
        let mut terminal = ctx.terminal.clone();
        fill_missing(
            &ctx.prompt_context(),
            &mut terminal,
            &required(&fields, &params),
            &mut params,
            &mut ArgumentResolver {
                ctx,
                fields: &fields,
                balance_hint: None,
            },
            history,
        )
        .map_err(prompt_error)?;
        let params = names(ctx, &fields, params)?;
        Ok(Self {
            name,
            params,
            block: matches
                .try_get_one::<u64>("__block")
                .ok()
                .flatten()
                .copied(),
        })
    }
}

/// Dispatch through the real typed SDK, pinning every access for --block.
pub async fn execute_query(
    client: &Client,
    request: &QueryRequest,
    output: &Output,
) -> Result<Value, CliError> {
    let payload = if let Some(block) = request.block {
        let snapshot = client.at(BlockRef::Number(block)).await.map_err(failure)?;
        rtao::reads::dispatch(&snapshot, request.name, &request.params).await
    } else {
        rtao::reads::dispatch(client, request.name, &request.params).await
    }
    .map_err(|error| match error {
        rtao::reads::ReadError::InvalidParam { .. } => input(error),
        _ => failure(error),
    })?;
    render_query(output, request.name, &payload)?;
    Ok(payload)
}

/// Apply human render hints without changing any machine payload.
pub fn render_query(output: &Output, name: ReadName, payload: &Value) -> Result<(), CliError> {
    if output.options.json_mode {
        return output.value(payload).map_err(failure);
    }
    if payload.is_null()
        || payload.as_array().is_some_and(Vec::is_empty)
        || payload.as_object().is_some_and(Map::is_empty)
    {
        return output
            .detail(Some(name.as_str()), &[], None)
            .map_err(failure);
    }
    let view = match name.spec().render {
        Some(RenderHint::Grouped { key }) => {
            let groups = payload
                .as_object()
                .ok_or_else(|| CliError::failure("grouped read is not an object"))?;
            let mut records = Vec::new();
            for (group, items) in groups {
                if items.is_null() {
                    continue;
                }
                for item in items
                    .as_array()
                    .ok_or_else(|| CliError::failure("grouped read items are not a list"))?
                {
                    let mut row = Map::from_iter([(key.into(), json!(group))]);
                    row.extend(
                        item.as_object()
                            .ok_or_else(|| CliError::failure("grouped read item is not a record"))?
                            .clone(),
                    );
                    records.push(Value::Object(row));
                }
            }
            Value::Array(records)
        }
        Some(RenderHint::Matrix { row, col, value }) => {
            let mut records = Vec::new();
            for (row_name, cells) in payload
                .as_object()
                .ok_or_else(|| CliError::failure("matrix read is not an object"))?
            {
                for (column, amount) in cells
                    .as_object()
                    .ok_or_else(|| CliError::failure("matrix row is not an object"))?
                {
                    records.push(json!({row:row_name,col:column,value:amount}));
                }
            }
            Value::Array(records)
        }
        _ => payload.clone(),
    };
    output.query(name.as_str(), &view).map_err(failure)
}
