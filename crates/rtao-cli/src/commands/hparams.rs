// Derived from RaoFoundation/subtensor sdk/python/bittensor/cli/commands/sudo.py @ 850f2755bb05a3851f948489f0ab27b0645cebce (Apache-2.0); modified by rtao.
// Additional source at the same commit/license: sdk/python/bittensor/cli/hyperparams_view.py.
//! Subnet-owner commands using the ordinary semantic submission boundary.

use super::generated::{self, QueryRequest, TxRequest};
use crate::{
    context::{AppContext, ExternalSignerFactory, SubmissionOptions},
    error::CliError,
    output::{Output, TableSpec},
    prompt::{PromptHistory, WalletConfirmation},
};
use clap::{Arg, ArgMatches, Command};
use rtao::{
    client::{BlockRef, Client, View, ViewExt},
    core::codec::Value as CoreValue,
    primitives::{NetUid, hyperparams::RawInteger, pyfmt},
    reads::ReadName,
    tx::ExtrinsicResult,
};
use rtao_catalog::{hyperparams::TABLE, owner_map::OWNER_HYPERPARAMETERS};
use serde_json::{Map, Value, json};

fn failure(error: impl std::fmt::Display) -> CliError {
    CliError::failure(error.to_string())
}

fn mutation(name: &str) -> Option<&'static str> {
    match name {
        "set" => Some("set_hyperparameter"),
        "start" => Some("start_call"),
        "set-symbol" => Some("update_symbol"),
        "set-identity" => Some("set_subnet_identity"),
        "trim" => Some("trim_subnet"),
        "trim-null-batch" => Some("trim_null_subnet_batch"),
        _ => None,
    }
}

/// Functional owner commands; sequential Null pruning is a separate workflow.
pub fn command() -> Result<Command, CliError> {
    let mut group = Command::new("hparams")
        .about("Subnet-owner config and governance.")
        .subcommand_required(true)
        .arg_required_else_help(true);
    for name in [
        "set",
        "start",
        "set-symbol",
        "set-identity",
        "trim",
        "trim-null-batch",
    ] {
        let op = mutation(name).ok_or_else(|| failure("missing owner operation"))?;
        let mut child = generated::intent_command(op)?.name(name);
        if name == "set-identity" {
            child = child
                .mut_arg("subnet_name", |arg| arg.long("name").alias("subnet-name"))
                .mut_arg("subnet_url", |arg| arg.long("url").alias("subnet-url"));
        }
        group = group.subcommand(child);
    }
    let get =
        generated::read_command(ReadName::SubnetHyperparameters)?
            .name("get")
            .arg(Arg::new("name").long("name").help(
                "Explain one hyperparameter, its raw value, and the command that changes it.",
            ));
    let identity = generated::read_command(ReadName::SubnetIdentity)?
        .name("get-identity")
        .mut_arg("netuid", |arg| {
            arg.long(clap::builder::Resettable::<clap::builder::Str>::Reset)
                .index(1)
                .required(true)
        });
    let count = generated::intent_command("set_mechanism_count")?
        .name("count")
        .about("Get mechanism count, or set it with --value (subnet owner only).")
        .mut_arg("mechanism_count", |arg| {
            arg.long("value")
                .help("New mechanism count; omit for a read-only query of the current count.")
        });
    let emissions = generated::read_command(ReadName::MechanismEmissionSplit)?.name("emissions");
    Ok(group
        .subcommand(get)
        .subcommand(generated::read_command(ReadName::DelegateTake)?.name("get-take"))
        .subcommand(generated::read_command(ReadName::SubnetStartSchedule)?.name("check-start"))
        .subcommand(identity)
        .subcommand(
            Command::new("mechanisms")
                .alias("mech")
                .about("Subnet mechanism configuration.")
                .subcommand_required(true)
                .arg_required_else_help(true)
                .subcommand(count)
                .subcommand(emissions),
        ))
}

fn leaf(matches: &ArgMatches) -> Result<(&str, &ArgMatches), CliError> {
    let (name, matches) = matches
        .subcommand()
        .ok_or_else(|| CliError::input("missing hparams command"))?;
    if name == "mechanisms" {
        matches
            .subcommand()
            .ok_or_else(|| CliError::input("missing mechanisms command"))
    } else {
        Ok((name, matches))
    }
}

/// Command-owned identifiers to reserve from leaf-global overrides.
pub fn reserved(matches: &ArgMatches) -> Result<Vec<&'static str>, CliError> {
    let (name, _) = leaf(matches)?;
    if let Some(op) = mutation(name) {
        return generated::intent_reserved(op);
    }
    let mut fields = match name {
        "count" => generated::intent_reserved("set_mechanism_count")?,
        "get" => generated::read_reserved(ReadName::SubnetHyperparameters),
        "get-take" => generated::read_reserved(ReadName::DelegateTake),
        "check-start" => generated::read_reserved(ReadName::SubnetStartSchedule),
        "get-identity" => generated::read_reserved(ReadName::SubnetIdentity),
        "emissions" => generated::read_reserved(ReadName::MechanismEmissionSplit),
        _ => return Err(CliError::input(format!("unknown hparams command {name:?}"))),
    };
    if name == "get" {
        fields.push("name");
    }
    Ok(fields)
}

/// Source-shaped request, prepared before connecting in noninteractive mode.
pub enum Request {
    /// A real SDK intent, retaining all normal review and signing controls.
    Transaction(TxRequest),
    /// Raw hyperparameters plus the owner-settable supplemental storage values.
    Hyperparameters {
        /// Catalog read and optional historical block.
        query: QueryRequest,
        /// Optional parameter explanation.
        name: Option<String>,
    },
    /// A curated read with its original command-specific machine payload.
    Read(QueryRequest),
}

impl Request {
    /// Parse the selected owner command without acquiring signing keys.
    pub fn from_matches(
        ctx: &mut AppContext,
        matches: &ArgMatches,
        history: &mut PromptHistory,
    ) -> Result<Self, CliError> {
        let (name, matches) = leaf(matches)?;
        if let Some(op) = mutation(name) {
            return TxRequest::from_matches(ctx, op, matches, history).map(Self::Transaction);
        }
        if name == "count" && matches.get_one::<String>("mechanism_count").is_some() {
            return TxRequest::from_matches(ctx, "set_mechanism_count", matches, history)
                .map(Self::Transaction);
        }
        let read = match name {
            "get" => ReadName::SubnetHyperparameters,
            "get-take" => ReadName::DelegateTake,
            "check-start" => ReadName::SubnetStartSchedule,
            "get-identity" => ReadName::SubnetIdentity,
            "count" => ReadName::MechanismCount,
            "emissions" => ReadName::MechanismEmissionSplit,
            _ => return Err(CliError::input(format!("unknown hparams command {name:?}"))),
        };
        let query = QueryRequest::from_matches(ctx, read, matches, history)?;
        Ok(if name == "get" {
            Self::Hyperparameters {
                query,
                name: matches.get_one::<String>("name").cloned(),
            }
        } else {
            Self::Read(query)
        })
    }

    /// Show current values before prompting for an omitted parameter name.
    pub async fn from_matches_with_client(
        ctx: &mut AppContext,
        client: &Client,
        matches: &ArgMatches,
        history: &mut PromptHistory,
    ) -> Result<Self, CliError> {
        let (name, args) = leaf(matches)?;
        if name == "set"
            && args.get_one::<String>("name").is_none()
            && ctx.prompt_context().interactive(&ctx.terminal)
        {
            ctx.confirm_wallet(
                &WalletConfirmation {
                    help: "Wallet whose coldkey signs this transaction.".into(),
                    require_coldkey: true,
                    ..Default::default()
                },
                history,
            )?;
            if let Some(netuid) = args.get_one::<String>("netuid") {
                let netuid = netuid.parse::<u16>().map(NetUid).map_err(failure)?;
                let params = fetch_hyperparameters(client, netuid).await?;
                render_hyperparameters(&ctx.output, netuid, &params, None)?;
            }
        }
        Self::from_matches(ctx, matches, history)
    }
}

fn netuid(query: &QueryRequest) -> Result<NetUid, CliError> {
    serde_json::from_value(query.params.get("netuid").cloned().unwrap_or(Value::Null))
        .map_err(|error| CliError::input(error.to_string()))
}

/// Extend the runtime hyperparameter record at one pinned block, as Python does.
pub async fn fetch_hyperparameters<V: View + ?Sized>(
    view: &V,
    netuid: NetUid,
) -> Result<Option<Map<String, Value>>, CliError> {
    let snapshot = view.pin().await.map_err(failure)?;
    let Some(params) = rtao::reads::reads::owner::subnet_hyperparameters(&snapshot, netuid)
        .await
        .map_err(failure)?
    else {
        return Ok(None);
    };
    let mut params: Map<String, Value> = params.into_iter().collect();
    // The extracted storage table is the same source allowlist used by Python's
    // view. Do not substitute the setter's paired/newtype storage representation.
    for row in OWNER_HYPERPARAMETERS {
        if params.contains_key(row.name) {
            continue;
        }
        let Some(item) = TABLE.storage.iter().find(|item| item.parameter == row.name) else {
            continue;
        };
        let mut value = snapshot
            .storage_dyn(
                item.container,
                item.name,
                &[CoreValue::Uint(u128::from(netuid.0))],
            )
            .await
            .map_err(failure)?
            .to_json();
        if value.as_object().is_some_and(|fields| fields.len() == 1)
            && let Some(bits) = value.get("bits")
        {
            value = bits.clone();
        }
        if !value.is_null() {
            params.insert(row.name.into(), value);
        }
    }
    Ok(Some(params))
}

fn integer(value: &Value) -> Option<RawInteger> {
    let Value::Number(value) = value else {
        return None;
    };
    value
        .as_i128()
        .map(RawInteger::Signed)
        .or_else(|| value.as_u128().map(RawInteger::Unsigned))
}

fn text(value: &Value) -> String {
    match value {
        Value::String(value) => value.clone(),
        Value::Bool(true) => "True".into(),
        Value::Bool(false) => "False".into(),
        _ => value.to_string(),
    }
}

/// Render raw machine values independently from human annotations and hints.
pub fn render_hyperparameters(
    output: &Output,
    netuid: NetUid,
    params: &Option<Map<String, Value>>,
    name: Option<&str>,
) -> Result<(), CliError> {
    let params = params
        .as_ref()
        .ok_or_else(|| failure(format!("subnet {netuid} does not exist")))?;
    if let Some(name) = name {
        let raw = params.get(name).ok_or_else(|| {
            CliError::input(format!(
                "no hyperparameter named {name:?} on netuid {netuid}"
            ))
            .with_help("run without --name to list them all")
        })?;
        let normalized = integer(raw).and_then(|raw| TABLE.normalized(name, raw));
        let annotation = integer(raw).and_then(|raw| TABLE.annotate(name, raw));
        let settable = OWNER_HYPERPARAMETERS.iter().any(|row| row.name == name);
        let payload = json!({
            "netuid": netuid,
            "name": name,
            "value": raw,
            "normalized": normalized,
            "kind": TABLE.kind_of(name),
            "doc": TABLE.doc_of(name),
            "owner_settable": settable,
        });
        let mut fields = vec![("value".into(), raw.clone())];
        if let Some(fraction) = normalized {
            fields.push(("normalized".into(), json!(pyfmt::fmt_g(fraction))));
        } else if let Some(reading) = annotation {
            fields.push((
                "reading".into(),
                json!(reading.trim_start_matches(['=', '≈', ' '])),
            ));
        }
        if let Some(doc) = TABLE.doc_of(name) {
            fields.push(("description".into(), json!(doc)));
        }
        fields.push((
            "note".into(),
            json!(if settable {
                format!("--value takes {}", TABLE.value_forms(name))
            } else {
                "not settable by the subnet owner (root/governance only)".into()
            }),
        ));
        if settable {
            // A raw integer is always an exact, accepted copy/paste example.
            let example = if name == "epoch_consensus" {
                if integer(raw).and_then(RawInteger::as_u128) == Some(1) {
                    "Null".into()
                } else {
                    "Yuma".into()
                }
            } else {
                text(raw)
            };
            fields.push((
                "set it".into(),
                json!(format!(
                    "rtao hparams set --netuid {netuid} --name {name} --value {example}"
                )),
            ));
        }
        fields.push((
            "see".into(),
            json!(format!(
                "{}/hyperparameters/{}",
                rtao_catalog::py::DOCS_URL,
                name.replace('_', "-")
            )),
        ));
        return output
            .detail(
                Some(&format!("{name}  netuid {netuid}")),
                &fields,
                Some(&payload),
            )
            .map_err(failure);
    }
    let rows = params
        .iter()
        .map(|(name, raw)| {
            vec![
                json!(name),
                json!(text(raw)),
                json!(
                    integer(raw)
                        .and_then(|raw| TABLE.annotate(name, raw))
                        .unwrap_or_default()
                ),
                json!(TABLE.short_of(name).unwrap_or("")),
            ]
        })
        .collect();
    let mut table = TableSpec::new(
        format!("hyperparameters netuid {netuid}"),
        vec![
            "parameter".into(),
            "value".into(),
            "reading".into(),
            "description".into(),
        ],
        rows,
    );
    table.records = Some(Value::Object(params.clone()));
    table.footer = Some(format!(
        "--name <parameter> explains one parameter and how to set it. {}/hyperparameters",
        rtao_catalog::py::DOCS_URL
    ));
    output.table(&table).map_err(failure)
}

fn render_read(output: &Output, query: &QueryRequest, payload: Value) -> Result<(), CliError> {
    match query.name {
        ReadName::MechanismCount => output.detail(
            None,
            &[
                ("netuid".into(), json!(netuid(query)?)),
                ("mechanism_count".into(), payload),
            ],
            None,
        ),
        ReadName::MechanismEmissionSplit => output.detail(
            None,
            &[
                ("netuid".into(), json!(netuid(query)?)),
                ("split".into(), payload),
            ],
            None,
        ),
        ReadName::DelegateTake => {
            let number = |key: &str| {
                payload
                    .get(key)
                    .and_then(Value::as_f64)
                    .ok_or_else(|| failure(format!("delegate_take lacks {key}")))
            };
            let fields = [
                ("hotkey".into(), payload["hotkey"].clone()),
                (
                    "take".into(),
                    json!(format!(
                        "{:.2}%  ({} as u16)",
                        number("take")? * 100.0,
                        payload["take_u16"]
                    )),
                ),
                (
                    "allowed".into(),
                    json!(format!(
                        "{:.2}% – {:.2}%",
                        number("min")? * 100.0,
                        number("max")? * 100.0
                    )),
                ),
            ];
            output.detail(None, &fields, Some(&payload))
        }
        ReadName::SubnetStartSchedule | ReadName::SubnetIdentity => {
            let title = if query.name == ReadName::SubnetIdentity {
                format!("subnet {} identity", netuid(query)?)
            } else {
                format!("start schedule netuid {}", netuid(query)?)
            };
            let fields = payload
                .as_object()
                .map(|fields| {
                    fields
                        .iter()
                        .map(|(key, value)| (key.clone(), value.clone()))
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default();
            output.detail(Some(&title), &fields, Some(&payload))
        }
        _ => return Err(failure("unrecognized curated owner read")),
    }
    .map_err(failure)
}

/// Execute a request through typed reads or the uniform semantic submit route.
pub async fn execute(
    ctx: &mut AppContext,
    client: &Client,
    request: &Request,
    options: SubmissionOptions<'_>,
    external: Option<&dyn ExternalSignerFactory>,
    history: &mut PromptHistory,
) -> Result<Option<ExtrinsicResult>, CliError> {
    match request {
        Request::Transaction(request) => {
            generated::execute_tx(ctx, client, request, options, external, history).await
        }
        Request::Hyperparameters { query, name } => {
            let snapshot = client
                .at(query.block.map_or(BlockRef::Best, BlockRef::Number))
                .await
                .map_err(failure)?;
            let netuid = netuid(query)?;
            let params = fetch_hyperparameters(&snapshot, netuid).await?;
            render_hyperparameters(&ctx.output, netuid, &params, name.as_deref())?;
            Ok(None)
        }
        Request::Read(query) => {
            let snapshot = client
                .at(query.block.map_or(BlockRef::Best, BlockRef::Number))
                .await
                .map_err(failure)?;
            let payload = rtao::reads::dispatch(&snapshot, query.name, &query.params)
                .await
                .map_err(failure)?;
            render_read(&ctx.output, query, payload)?;
            Ok(None)
        }
    }
}
