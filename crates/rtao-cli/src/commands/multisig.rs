// Derived from RaoFoundation/subtensor sdk/python/bittensor/cli/commands/multisig.py @ 850f2755bb05a3851f948489f0ab27b0645cebce (Apache-2.0); modified by rtao.
// Additional sources at the same commit/license: cli/multisig_helpers.py, cli/commands/upgrade.py and cli/upgrade_helpers.py.
//! Saved multisig books and public pending-operation inspection.

use crate::{
    books::{AccountLookup, FsAccountLookup, MultisigBook, split_signatory_refs},
    call::{
        FsRawCallCache, MultisigCallSpec, RawCallCache, RawMultisig, decode_multisig_call_spec_at,
        multisig_replay_command, resolve_preset, signatory_labels,
    },
    context::AppContext,
    error::CliError,
    globals::{Tier, with_globals},
    output::Output,
    prompt::{
        PromptError, PromptHistory, PromptResolver, PromptSpec, PromptValidation,
        WalletConfirmation, fill_missing,
    },
};
use clap::{Arg, ArgAction, ArgMatches, Command};
use futures::StreamExt;
use rtao::{
    client::{BlockRef, Client, Snapshot, ViewExt},
    core::{DynValue, codec::Value as CoreValue, value_ext::ValueExt},
    primitives::{AccountId, pyfmt},
    signer::{BoxFuture, SignerRole},
    tx::Multisig,
};
use rtao_runtime::{
    CallBytes, RuntimeHandle,
    models::{MultisigInfo, MultisigTimepoint, ProxyType},
    storage as st,
};
use serde_json::{Map, Value, json};
use std::{collections::BTreeSet, time::Duration};

fn failure(error: impl std::fmt::Display) -> CliError {
    CliError::failure(error.to_string())
}
fn truthy(value: &Value) -> bool {
    match value {
        Value::Null => false,
        Value::Bool(value) => *value,
        Value::Number(value) => value.as_f64() != Some(0.0),
        Value::String(value) => !value.is_empty(),
        Value::Array(value) => !value.is_empty(),
        Value::Object(value) => !value.is_empty(),
    }
}
fn input(error: impl std::fmt::Display) -> CliError {
    CliError::input(error.to_string())
}
fn prompt_error(error: PromptError) -> CliError {
    CliError {
        code: error.exit_code(),
        message: error.to_string(),
        help: None,
    }
}
fn option(id: &'static str, long: &'static str, help: &'static str) -> Arg {
    Arg::new(id).long(long).help(help)
}

/// The five public-book and pending-operation commands, all without unlocking.
pub fn command() -> Command {
    let add = Command::new("add")
        .about("Save a named multisig signer set for pending and call --multisig.")
        .arg(Arg::new("name").help("Saved name; defaults to the -w wallet name."))
        .arg(
            option(
                "threshold",
                "threshold",
                "Approvals required (the M in M-of-N).",
            )
            .required(true)
            .value_parser(clap::value_parser!(u16).range(1..)),
        )
        .arg(option(
            "signatories",
            "signatories",
            "Full signer set: wallet names, contacts or ss58; comma/space separated.",
        ))
        .arg(
            option(
                "signatory",
                "signatory",
                "One signer reference; repeat for each member.",
            )
            .action(ArgAction::Append),
        )
        .arg(option("note", "note", "Free-form note stored with the entry.").default_value(""))
        .arg(
            option(
                "overwrite",
                "overwrite",
                "Replace an existing entry with this exact name.",
            )
            .action(ArgAction::SetTrue),
        );
    let named = |name: &'static str, help: &'static str| {
        Command::new(name)
            .about(help)
            .arg(Arg::new("name").required(true).help("Saved multisig name."))
    };
    let pending = Command::new("pending")
        .about("List pending multisig operations with approval status and co-signer commands.")
        .arg(option(
            "multisig",
            "multisig",
            "Saved multisig; defaults to -w when saved.",
        ))
        .arg(
            option(
                "multisig_threshold",
                "multisig-threshold",
                "Approvals required for an inline signer set.",
            )
            .value_parser(clap::value_parser!(u16).range(1..)),
        )
        .arg(option(
            "signatories",
            "signatories",
            "Full signer set, including yourself; comma/space separated.",
        ))
        .arg(option(
            "other_signatories",
            "other-signatories",
            "Other members; your selected wallet key is added.",
        ))
        .arg(
            option(
                "signer_backend",
                "signer",
                "Which wallet key belongs to the set.",
            )
            .default_value("coldkey")
            .value_parser(["coldkey", "hotkey"]),
        )
        .arg(option(
            "call_hash",
            "call-hash",
            "Show one pending operation by call hash.",
        ))
        .arg(option(
            "call_data",
            "call-data",
            "SCALE call hex for the selected --call-hash when missing from cache.",
        ));
    Command::new("multisig")
        .about("Save multisig signer sets and track pending multisig operations.")
        .subcommand_required(true)
        .arg_required_else_help(true)
        .subcommands([
            with_globals(add, Tier::Read),
            with_globals(
                Command::new("list").about("List saved multisig signer sets."),
                Tier::Read,
            ),
            with_globals(
                named(
                    "show",
                    "Derive the on-chain address for a saved signer set.",
                ),
                Tier::Read,
            ),
            with_globals(
                named("remove", "Remove a saved multisig signer set."),
                Tier::Read,
            ),
            with_globals(pending, Tier::Read),
        ])
}

/// Reserve command-owned repeated references and the pending key-role option.
pub fn reserved(matches: &ArgMatches) -> Result<Vec<&'static str>, CliError> {
    match matches.subcommand().map(|(name, _)| name) {
        Some("add") => Ok(vec!["signatory"]),
        Some("pending") => Ok(vec!["signer_backend"]),
        Some("list" | "show" | "remove") => Ok(Vec::new()),
        _ => Err(CliError::input("missing or unknown multisig command")),
    }
}

fn text<'a>(matches: &'a ArgMatches, id: &str) -> Option<&'a str> {
    matches.get_one::<String>(id).map(String::as_str)
}
fn name(matches: &ArgMatches) -> Result<String, CliError> {
    text(matches, "name")
        .map(str::to_owned)
        .ok_or_else(|| input("missing multisig name"))
}
fn resolve_refs(ctx: &AppContext, refs: &[String]) -> Result<Vec<AccountId>, CliError> {
    let mut result = Vec::new();
    for reference in refs {
        let account = ctx
            .resolve_address("coldkey_ss58", Some(reference))?
            .ok_or_else(|| input(format!("cannot resolve {}", pyfmt::string_repr(reference))))?;
        if !result.contains(&account) {
            result.push(account);
        }
    }
    Ok(result)
}
fn collect_refs<'a>(
    raw: Option<&'a str>,
    repeated: Option<clap::parser::ValuesRef<'a, String>>,
) -> Vec<String> {
    let mut seen = BTreeSet::new();
    raw.into_iter()
        .chain(repeated.into_iter().flatten().map(String::as_str))
        .flat_map(split_signatory_refs)
        .filter(|reference| seen.insert((*reference).to_owned()))
        .map(str::to_owned)
        .collect()
}
struct RefResolver<'a>(&'a AppContext);
impl PromptResolver for RefResolver<'_> {
    fn parse(&mut self, _spec: &PromptSpec, raw: &str) -> Result<Value, PromptValidation> {
        let refs: Vec<_> = split_signatory_refs(raw)
            .into_iter()
            .map(str::to_owned)
            .collect();
        if refs.is_empty() {
            return Err(PromptValidation::Message(
                "need at least one signatory (wallet name, address-book name, or ss58)".into(),
            ));
        }
        resolve_refs(self.0, &refs)
            .map_err(|error| PromptValidation::Message(error.to_string()))?;
        Ok(json!(refs))
    }
}
fn confirm_name(
    ctx: &mut AppContext,
    help: &str,
    history: &mut PromptHistory,
) -> Result<(), CliError> {
    ctx.confirm_wallet(
        &WalletConfirmation {
            help: help.into(),
            must_exist: false,
            require_coldkey: false,
            ..WalletConfirmation::default()
        },
        history,
    )?;
    Ok(())
}

/// Public requests carry resolved account identities and portable stored refs.
pub enum Request {
    /// Store these original references before deriving their live address.
    Add {
        /// Exact book row, including note.
        entry: Value,
        /// Distinct resolved account identities in input order.
        group: RawMultisig,
        /// Explicit permission to replace a matching name.
        overwrite: bool,
    },
    /// Read the local book without connecting.
    List,
    /// Derive one saved entry using live metadata.
    Show {
        /// Saved entry name.
        name: String,
        /// Resolved saved signer set.
        group: RawMultisig,
    },
    /// Delete only rows with this exact name, without connecting.
    Remove {
        /// Saved entry name.
        name: String,
    },
    /// Inspect pending operations and optionally authenticate supplied call bytes.
    Pending {
        /// Selected saved or inline signer set.
        group: RawMultisig,
        /// Optional complete call-hash filter.
        call_hash: Option<[u8; 32]>,
        /// Supplied hex is used only when a call hash is selected and uncached.
        call_data: Option<String>,
    },
}
impl Request {
    /// Resolve prompts, aliases and public wallet identities before connection.
    pub fn from_matches(
        ctx: &mut AppContext,
        matches: &ArgMatches,
        history: &mut PromptHistory,
    ) -> Result<Self, CliError> {
        let (command, matches) = matches
            .subcommand()
            .ok_or_else(|| input("missing multisig command"))?;
        match command {
            "list" => Ok(Self::List),
            "remove" => Ok(Self::Remove {
                name: name(matches)?,
            }),
            "show" => {
                let name = name(matches)?;
                let group = resolve_preset(ctx, &name).map_err(|error| {
                    if ctx.books.multisig(&name).is_none() {
                        failure(format!("multisig {} not found", pyfmt::string_repr(&name)))
                    } else {
                        error
                    }
                })?;
                Ok(Self::Show { name, group })
            }
            "add" => {
                let name = if let Some(name) = text(matches, "name") {
                    name.to_owned()
                } else {
                    confirm_name(ctx, "Name to save the multisig under (-w name).", history)?;
                    ctx.globals.wallet_name.clone()
                };
                let mut refs = collect_refs(
                    text(matches, "signatories"),
                    matches.get_many::<String>("signatory"),
                );
                if refs.is_empty() {
                    let mut spec = PromptSpec::new("signatories", "--signatories");
                    spec.help = Some("Full signer set: wallet names, address-book names, or ss58 addresses (comma-separated, space-separated, or mixed).".into());
                    spec.placeholder = Some("name1, name2  or  name1 name2".into());
                    let mut answers = Map::new();
                    let mut terminal = ctx.terminal.clone();
                    fill_missing(
                        &ctx.prompt_context(),
                        &mut terminal,
                        &[spec],
                        &mut answers,
                        &mut RefResolver(ctx),
                        history,
                    )
                    .map_err(prompt_error)?;
                    refs = serde_json::from_value(
                        answers.remove("signatories").unwrap_or(Value::Null),
                    )
                    .map_err(failure)?;
                }
                let signatories = resolve_refs(ctx, &refs)?;
                let threshold = *matches
                    .get_one::<u16>("threshold")
                    .ok_or_else(|| input("missing --threshold"))?;
                if usize::from(threshold) > signatories.len() {
                    return Err(failure(format!(
                        "threshold {threshold} exceeds {} signatories",
                        signatories.len()
                    )));
                }
                let overwrite = matches.get_flag("overwrite");
                if ctx.books.multisig(&name).is_some() && !overwrite {
                    return Err(failure(format!(
                        "multisig {} already exists",
                        pyfmt::string_repr(&name)
                    ))
                    .with_help("pass `--overwrite` to replace it"));
                }
                let entry = json!({"name":name,"threshold":threshold,"signatories":refs,"note":text(matches,"note").unwrap_or("")});
                Ok(Self::Add {
                    entry,
                    group: RawMultisig {
                        threshold,
                        signatories,
                        preset: Some(name),
                        refs,
                    },
                    overwrite,
                })
            }
            "pending" => {
                let preset = text(matches, "multisig").filter(|value| !value.is_empty());
                let full = text(matches, "signatories").filter(|value| !value.is_empty());
                let others = text(matches, "other_signatories").filter(|value| !value.is_empty());
                if preset.is_none() && full.is_none() && others.is_none() {
                    confirm_name(ctx, "Multisig wallet name (-w name).", history)?;
                }
                let threshold = matches.get_one::<u16>("multisig_threshold").copied();
                let inline = threshold.is_some() || full.is_some() || others.is_some();
                if preset.is_some() && inline {
                    return Err(failure(
                        "use either --multisig NAME or inline multisig flags, not both",
                    ));
                }
                let default = (!inline && ctx.books.multisig(&ctx.globals.wallet_name).is_some())
                    .then_some(ctx.globals.wallet_name.as_str());
                let group = if let Some(preset) = preset.or(default) {
                    resolve_preset(ctx, preset)?
                } else if let Some(threshold) = threshold {
                    if full.is_some() && others.is_some() {
                        return Err(failure(
                            "pass either --signatories or --other-signatories, not both",
                        ));
                    }
                    let raw = full.or(others).ok_or_else(|| {
                        failure(
                            "with --multisig-threshold, pass --signatories or --other-signatories",
                        )
                    })?;
                    let mut refs: Vec<_> = split_signatory_refs(raw)
                        .into_iter()
                        .map(str::to_owned)
                        .collect();
                    let mut signatories = resolve_refs(ctx, &refs)?;
                    if others.is_some() {
                        let role = match text(matches, "signer_backend").unwrap_or("coldkey") {
                            "coldkey" => "coldkey_ss58",
                            "hotkey" => "hotkey_ss58",
                            _ => return Err(failure("signer must be 'coldkey' or 'hotkey'")),
                        };
                        let member = ctx
                            .resolve_address(role, None)?
                            .ok_or_else(|| failure("selected wallet key has no public account"))?;
                        if !signatories.contains(&member) {
                            signatories.push(member);
                        }
                        refs.push(ctx.globals.wallet_name.clone());
                    }
                    RawMultisig {
                        threshold,
                        signatories,
                        preset: None,
                        refs,
                    }
                } else {
                    let mut saved: Vec<_> = ctx
                        .books
                        .multisigs
                        .iter()
                        .filter_map(|entry| entry.get("name").and_then(Value::as_str))
                        .filter(|name| !name.is_empty())
                        .collect();
                    saved.sort_unstable();
                    let help = if saved.is_empty() {
                        format!(
                            "save one with `rtao multisig add {} --threshold N --signatories a,b,c`, or pass `--multisig-threshold N --signatories a,b,c` inline",
                            ctx.globals.wallet_name
                        )
                    } else {
                        format!(
                            "pick a saved multisig ({}) with `--multisig NAME` or `-w NAME`, or pass `--multisig-threshold N --signatories a,b,c` inline",
                            saved.join(", ")
                        )
                    };
                    return Err(failure(format!(
                        "no multisig configured for wallet {}",
                        pyfmt::string_repr(&ctx.globals.wallet_name)
                    ))
                    .with_help(help));
                };
                let call_hash = text(matches, "call_hash")
                    .filter(|value| !value.is_empty())
                    .map(parse_hash)
                    .transpose()?;
                Ok(Self::Pending {
                    group,
                    call_hash,
                    call_data: text(matches, "call_data").map(str::to_owned),
                })
            }
            _ => Err(input(format!("unknown multisig command {command:?}"))),
        }
    }
    /// Only book listing and removal are entirely offline.
    pub fn needs_client(&self) -> bool {
        !matches!(self, Self::List | Self::Remove { .. })
    }
}

fn parse_hash(text: &str) -> Result<[u8; 32], CliError> {
    let bytes = hex::decode(text.strip_prefix("0x").unwrap_or(text))
        .map_err(|_| input("call hash must be 32 bytes of hex"))?;
    bytes
        .try_into()
        .map_err(|_| input("call hash must be 32 bytes of hex"))
}
fn hex_hash(hash: [u8; 32]) -> String {
    format!("0x{}", hex::encode(hash))
}
fn parse_blob(text: &str) -> Result<Vec<u8>, CliError> {
    let text = crate::prompt::python_strip(text);
    let text = text.strip_prefix("0x").unwrap_or(text);
    if text.is_empty() {
        return Err(input("call data is not a valid hex string"));
    }
    hex::decode(text).map_err(|_| input("call data is not a valid hex string"))
}
fn hash_blob(blob: &[u8]) -> Result<[u8; 32], CliError> {
    rtao::core::codec::storage::hash_param("Blake2_256", blob)
        .map_err(failure)?
        .try_into()
        .map_err(|_| failure("blake2_256 did not return 32 bytes"))
}
fn detail(output: &Output, title: Option<&str>, payload: Value) -> Result<(), CliError> {
    let fields = payload
        .as_object()
        .ok_or_else(|| failure("detail is not an object"))?
        .iter()
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect::<Vec<_>>();
    output
        .detail(title, &fields, Some(&payload))
        .map_err(failure)
}

/// Run a family request using the application's resolved shared paths.
pub async fn run(
    ctx: &mut AppContext,
    paths: &rtao::client::config::Paths,
    matches: &ArgMatches,
    history: &mut PromptHistory,
) -> Result<(), CliError> {
    let request = Request::from_matches(ctx, matches, history)?;
    if request.needs_client() {
        // An accepted add is local configuration even when connecting fails.
        let saved = save_add(paths, &request).await?;
        let context = ctx.clone();
        let paths = paths.clone();
        ctx.run(move |client| async move {
            execute_saved(
                &context,
                &paths,
                &request,
                Some(&client),
                &GithubFetch,
                saved,
            )
            .await
        })
        .await
    } else {
        execute(ctx, paths, &request, None).await
    }
}

/// Execute public reads or exact book mutations with an optional injected client.
/// The caller owns the client lifetime; no signer is acquired by this path.
pub async fn execute(
    ctx: &AppContext,
    paths: &rtao::client::config::Paths,
    request: &Request,
    client: Option<&Client>,
) -> Result<(), CliError> {
    execute_with_fetch(ctx, paths, request, client, &GithubFetch).await
}
async fn execute_with_fetch(
    ctx: &AppContext,
    paths: &rtao::client::config::Paths,
    request: &Request,
    client: Option<&Client>,
    fetch: &dyn UpgradeFetch,
) -> Result<(), CliError> {
    let saved = save_add(paths, request).await?;
    execute_saved(ctx, paths, request, client, fetch, saved).await
}
async fn save_add(
    paths: &rtao::client::config::Paths,
    request: &Request,
) -> Result<Option<Value>, CliError> {
    if let Request::Add {
        entry, overwrite, ..
    } = request
    {
        MultisigBook::new(paths)
            .add(entry.clone(), *overwrite)
            .await
            .map(Some)
            .map_err(failure)
    } else {
        Ok(None)
    }
}
async fn execute_saved(
    ctx: &AppContext,
    paths: &rtao::client::config::Paths,
    request: &Request,
    client: Option<&Client>,
    fetch: &dyn UpgradeFetch,
    saved: Option<Value>,
) -> Result<(), CliError> {
    use rtao::client::config::ConfigFile;
    let book = MultisigBook::new(paths);
    match request {
        Request::List => {
            return detail(
                &ctx.output,
                Some("multisig book"),
                json!({"path":paths.path(ConfigFile::Multisigs),"multisigs":book.load().await.map_err(failure)?}),
            );
        }
        Request::Remove { name } => {
            return detail(
                &ctx.output,
                Some("removed multisig"),
                json!({"name":name,"existed":book.remove(name).await.map_err(failure)?}),
            );
        }
        _ => {}
    }
    let client = client.ok_or_else(|| failure("this multisig command requires a client"))?;
    let snapshot = client.at(BlockRef::Best).await.map_err(failure)?;
    let group = match request {
        Request::Add { group, .. }
        | Request::Show { group, .. }
        | Request::Pending { group, .. } => group,
        _ => return Err(failure("unexpected offline multisig request")),
    };
    let multisig = Multisig::new(
        &snapshot.block().runtime,
        &group.signatories,
        group.threshold,
    )
    .map_err(failure)?;
    let sudo = snapshot
        .storage(&st::sudo::KEY, ())
        .await
        .map_err(failure)?;
    match request {
        Request::Add { entry, .. } => {
            let name = entry
                .get("name")
                .and_then(Value::as_str)
                .ok_or_else(|| input("missing saved name"))?;
            let lookup = FsAccountLookup::new(ctx.globals.wallet_path.clone()).map_err(failure)?;
            let mut result = json!({"entry":saved,"path":paths.path(ConfigFile::Multisigs),"multisig_address":multisig.address,"coldkey_wallet_also_exists":lookup.coldkey(name).is_ok()});
            if sudo == Some(multisig.address) {
                result["is_chain_sudo_key"] = json!(true);
            }
            detail(&ctx.output, Some("saved multisig"), result)
        }
        Request::Show { name, .. } => {
            let mut result = json!({"threshold":group.threshold,"signatories":group.signatories,"multisig_address":multisig.address});
            if sudo == Some(multisig.address) {
                result["is_chain_sudo_key"] = json!(true);
            }
            detail(&ctx.output, Some(name), result)
        }
        Request::Pending {
            call_hash,
            call_data,
            ..
        } => {
            let records = pending_records(
                ctx,
                client,
                &snapshot,
                group,
                &multisig,
                &FsRawCallCache::new(paths),
                *call_hash,
                call_data.as_deref(),
            )
            .await?;
            let upgrades = if sudo == Some(multisig.address) && call_hash.is_none() {
                load_pending_upgrades(ctx, &snapshot, multisig.address, fetch).await?
            } else {
                Vec::new()
            };
            render_pending(&ctx.output, group, &records, &upgrades)
        }
        _ => Err(failure("unexpected offline multisig request")),
    }
}

async fn list_pending(
    snapshot: &Snapshot,
    address: AccountId,
) -> Result<Vec<([u8; 32], MultisigInfo)>, CliError> {
    let rows = snapshot.storage_iter(&st::multisig::MULTISIGS, (address,));
    tokio::pin!(rows);
    let mut result = Vec::new();
    while let Some(row) = rows.next().await {
        let (keys, pending) = row.map_err(failure)?;
        let key = keys
            .first()
            .filter(|_| keys.len() == 1)
            .ok_or_else(|| failure("multisig map did not return its call-hash key"))?;
        let bytes = key
            .as_core()
            .value_bytes()
            .ok_or_else(|| failure("multisig map call-hash key is not bytes"))?;
        let hash = bytes
            .try_into()
            .map_err(|_| failure("multisig map call-hash key is not 32 bytes"))?;
        result.push((hash, pending));
    }
    result.sort_by_key(|(_, pending)| std::cmp::Reverse(pending.when.height));
    Ok(result)
}

// Recover only a full `as_multi` opening, using its historical parent runtime.
// Pruned or undecodable blocks degrade to unknown call details. Imported bytes
// still have to hash to the on-chain key and decode in the selected live runtime.
async fn opening_call(
    client: &Client,
    when: &MultisigTimepoint,
    hash: [u8; 32],
    runtime: &RuntimeHandle,
) -> Option<CallBytes> {
    let snapshot = client
        .at(BlockRef::Number(u64::from(when.height)))
        .await
        .ok()?;
    let block: Value = client
        .request("chain_getBlock", json!([hex_hash(snapshot.block().hash.0)]))
        .await
        .ok()?;
    let extrinsic = block
        .get("block")?
        .get("extrinsics")?
        .as_array()?
        .get(when.index as usize)?
        .as_str()?;
    let bytes = parse_blob(extrinsic).ok()?;
    let bytes = opening_inner_bytes(&bytes, &snapshot.block().runtime)?;
    if hash_blob(&bytes).ok()? != hash {
        return None;
    }
    CallBytes::decode(&bytes, runtime).ok()
}

// Metadata-guided offsets preserve the exact call bytes, including byte fields
// whose decoded inspection strings can be ambiguous (literal UTF-8 "0x41").
fn opening_inner_bytes(extrinsic: &[u8], runtime: &RuntimeHandle) -> Option<Vec<u8>> {
    use rtao::core::{
        codec::decode::{Cursor, compact_u128},
        ir::MetadataIr,
    };
    let core = runtime.core();
    core.decode_extrinsic(extrinsic, true).ok()?;
    let mut cursor = Cursor::new(extrinsic);
    cursor.strict = true;
    compact_u128(&mut cursor).ok()?;
    let version = cursor.byte().ok()?;
    if version & 0x80 != 0 {
        core.decode_id(core.extrinsic.address_type?, &mut cursor)
            .ok()?;
        core.decode_id(core.extrinsic.signature_type?, &mut cursor)
            .ok()?;
        for extension in &core.extrinsic.signed_extensions {
            core.decode_id(extension.ty, &mut cursor).ok()?;
        }
    }
    let pallet_index = cursor.byte().ok()?;
    let call_index = cursor.byte().ok()?;
    let ir = MetadataIr::from_runtime(core).ok()?;
    let pallet = ir
        .pallets
        .iter()
        .find(|pallet| pallet.index == pallet_index && pallet.name == "Multisig")?;
    let call = pallet
        .calls
        .iter()
        .find(|call| call.index == call_index && call.name == "as_multi")?;
    for argument in &call.args {
        let start = cursor.offset;
        core.decode_id(argument.type_id, &mut cursor).ok()?;
        if argument.name == "call" {
            return Some(extrinsic.get(start..cursor.offset)?.to_vec());
        }
    }
    None
}

#[allow(clippy::too_many_arguments)]
async fn pending_records(
    ctx: &AppContext,
    client: &Client,
    snapshot: &Snapshot,
    group: &RawMultisig,
    multisig: &Multisig,
    cache: &FsRawCallCache,
    filter: Option<[u8; 32]>,
    supplied: Option<&str>,
) -> Result<Vec<Value>, CliError> {
    let mut records = Vec::new();
    for (hash, pending) in list_pending(snapshot, multisig.address).await? {
        if filter.is_some_and(|filter| filter != hash) {
            continue;
        }
        let override_data = supplied.filter(|_| filter == Some(hash));
        let call = if let Some(call) = cache
            .load_call(&hex_hash(hash), &snapshot.block().runtime)
            .await?
        {
            Some(call)
        } else if let Some(data) = override_data {
            let bytes = parse_blob(data)?;
            let actual = hash_blob(&bytes)?;
            if actual != hash {
                return Err(failure(format!(
                    "--call-data does not match call hash {} (blake2_256 of the data is {})",
                    hex_hash(hash),
                    hex_hash(actual)
                )));
            }
            CallBytes::decode(&bytes, &snapshot.block().runtime).ok()
        } else {
            opening_call(client, &pending.when, hash, &snapshot.block().runtime).await
        };
        // Pinned list_pending_with_commands defaults followup's signer role to
        // coldkey; --signer only controls --other-signatories self resolution.
        let spec = if let Some(call) = &call {
            decode_multisig_call_spec_at(&snapshot.block().runtime, call, SignerRole::Coldkey)
        } else {
            MultisigCallSpec::unknown(SignerRole::Coldkey)
        };
        if let Some(call) = &call
            && override_data.is_some()
            && spec.target.is_some()
        {
            cache.save(hex_hash(hash), json!({"target":spec.target,"params":spec.params,"sudo":spec.sudo,"proxy_for":spec.proxy_for,"force_proxy_type":spec.force_proxy_type,"call_data":format!("0x{}",hex::encode(call.as_bytes())),"call_hash":hex_hash(hash),"threshold":group.threshold,"signatories":group.signatories,"multisig_address":multisig.address,"multisig_preset":group.preset,"network":ctx.globals.network})).await?;
        }
        records.push(pending_record(
            ctx,
            group,
            multisig.address,
            hash,
            &pending,
            &spec,
            call.as_ref(),
        )?);
    }
    Ok(records)
}

fn pending_record(
    ctx: &AppContext,
    group: &RawMultisig,
    address: AccountId,
    hash: [u8; 32],
    pending: &MultisigInfo,
    spec: &MultisigCallSpec,
    call: Option<&CallBytes>,
) -> Result<Value, CliError> {
    let labels = signatory_labels(ctx, group)?;
    let display = |account: AccountId| {
        let address = account.to_string();
        labels
            .get(&account)
            .filter(|name| **name != address)
            .map_or_else(|| address.clone(), |name| format!("{name} ({address})"))
    };
    let remaining: Vec<_> = group
        .signatories
        .iter()
        .copied()
        .filter(|member| !pending.approvals.contains(member))
        .collect();
    let mut commands = Vec::new();
    if spec.target.is_some() {
        for member in &remaining {
            commands.push(json!({"ss58":member,"label":display(*member),"name":labels.get(member).cloned().unwrap_or_else(||member.to_string()),"command":multisig_replay_command(ctx,spec,group,*member,&labels)?}));
        }
    }
    let available = !commands.is_empty();
    Ok(json!({
        "status":"pending","target":spec.target,"sudo":spec.sudo,"proxy_for":spec.proxy_for,"force_proxy_type":spec.force_proxy_type,"params":spec.params,
        "approvals":pending.approvals.len(),"threshold":group.threshold,"call_hash":hex_hash(hash),"call_data":call.map(|call|format!("0x{}",hex::encode(call.as_bytes()))),
        "timepoint":pending.when,"timepoint_display":format!("{}:{}",pending.when.height,pending.when.index),"multisig_address":address,"multisig_preset":group.preset,
        "depositor":pending.depositor,"depositor_label":display(pending.depositor),"approvals_so_far":pending.approvals,"approval_labels":pending.approvals.iter().copied().map(display).collect::<Vec<_>>(),
        "remaining_signatories":remaining,"remaining_labels":remaining.iter().copied().map(display).collect::<Vec<_>>(),"co_signer_commands":commands,"commands_available":available,
        "decode_note":(!available).then_some("call details unknown — the opening block may be pruned on this RPC node, the op may have used approve_as_multi (hash only), or local cache may be missing"),
        "decode_hint":(!available).then_some("pass `--call-data 0x..` once, or re-open the op with a current SDK build")
    }))
}

fn render_pending(
    output: &Output,
    group: &RawMultisig,
    records: &[Value],
    upgrades: &[Value],
) -> Result<(), CliError> {
    if output.options.json_mode && !upgrades.is_empty() {
        return output.value(&json!({"pending_multisigs":records,"count":records.len(),"pending_upgrades":upgrades})).map_err(failure);
    }
    let label = group
        .preset
        .clone()
        .unwrap_or_else(|| format!("{}-of-{}", group.threshold, group.signatories.len()));
    output
        .pending_multisigs(&format!("pending multisig — {label}"), records)
        .map_err(failure)?;
    for (index, record) in upgrades.iter().enumerate() {
        let fields = upgrade_fields(record);
        output
            .detail(
                (index == 0).then_some("pending runtime upgrade"),
                &fields,
                None,
            )
            .map_err(failure)?;
    }
    Ok(())
}

const UPGRADE_REPO: &str = "RaoFoundation/subtensor";
const MAX_FETCH_BYTES: usize = 64 * 1024 * 1024;
trait UpgradeFetch: Send + Sync {
    fn bytes<'a>(&'a self, url: &'a str, api: bool) -> BoxFuture<'a, Result<Vec<u8>, CliError>>;
}
struct GithubFetch;
impl UpgradeFetch for GithubFetch {
    fn bytes<'a>(&'a self, url: &'a str, api: bool) -> BoxFuture<'a, Result<Vec<u8>, CliError>> {
        Box::pin(async move {
            let tls = rtao_rpc::tls_config(&rtao_rpc::SessionConfig::default()).map_err(failure)?;
            let client = reqwest::Client::builder()
                .tls_backend_preconfigured((*tls).clone())
                .timeout(Duration::from_secs(60))
                .redirect(reqwest::redirect::Policy::limited(10))
                .build()
                .map_err(|_| failure("cannot initialize upgrade manifest HTTP client"))?;
            let mut request = client.get(url).header("User-Agent", "rtao-upgrade");
            if api {
                request = request.header("Accept", "application/vnd.github+json");
                if let Some(token) = std::env::var("GH_TOKEN")
                    .ok()
                    .filter(|value| !value.is_empty())
                    .or_else(|| {
                        std::env::var("GITHUB_TOKEN")
                            .ok()
                            .filter(|value| !value.is_empty())
                    })
                {
                    request = request.bearer_auth(token);
                }
            }
            let mut response = request
                .send()
                .await
                .map_err(|_| failure("upgrade manifest fetch failed"))?
                .error_for_status()
                .map_err(|_| failure("upgrade manifest HTTP status refused"))?;
            if response
                .content_length()
                .is_some_and(|bytes| bytes > MAX_FETCH_BYTES as u64)
            {
                return Err(failure("upgrade asset exceeds the 64 MiB fetch cap"));
            }
            let mut bytes = Vec::new();
            while let Some(chunk) = response
                .chunk()
                .await
                .map_err(|_| failure("upgrade asset body read failed"))?
            {
                if chunk.len() > MAX_FETCH_BYTES.saturating_sub(bytes.len()) {
                    return Err(failure("upgrade asset exceeds the 64 MiB fetch cap"));
                }
                bytes.extend_from_slice(&chunk);
            }
            Ok(bytes)
        })
    }
}
async fn fetch_json(fetch: &dyn UpgradeFetch, url: &str, api: bool) -> Option<Value> {
    let bytes = fetch.bytes(url, api).await.ok()?;
    if bytes.len() > MAX_FETCH_BYTES {
        return None;
    }
    serde_json::from_slice(&bytes).ok()
}
async fn release_manifests(fetch: &dyn UpgradeFetch) -> Vec<Value> {
    let url = format!("https://api.github.com/repos/{UPGRADE_REPO}/releases?per_page=15");
    let Some(Value::Array(releases)) = fetch_json(fetch, &url, true).await else {
        return Vec::new();
    };
    let mut manifests = Vec::new();
    for release in releases.into_iter().take(15) {
        let asset = release
            .get("assets")
            .and_then(Value::as_array)
            .and_then(|assets| {
                assets.iter().find(|asset| {
                    asset.get("name").and_then(Value::as_str) == Some("upgrade-manifest.json")
                })
            });
        let Some(url) = asset
            .and_then(|asset| asset.get("browser_download_url"))
            .and_then(Value::as_str)
        else {
            continue;
        };
        let Some(mut manifest @ Value::Object(_)) = fetch_json(fetch, url, false).await else {
            continue;
        };
        manifest["release_url"] = release.get("html_url").cloned().unwrap_or(Value::Null);
        manifest["prerelease"] = json!(release.get("prerelease").is_some_and(truthy));
        manifests.push(manifest);
    }
    manifests
}

async fn discover_pending_upgrades(
    snapshot: &Snapshot,
    sudo: AccountId,
) -> Result<Vec<Value>, CliError> {
    let (proxies, _) = snapshot
        .storage(&st::proxy::PROXIES, (sudo,))
        .await
        .map_err(failure)?;
    let mut records = Vec::new();
    for proxy in proxies {
        if proxy.proxy_type != ProxyType::SudoUncheckedSetCode || proxy.delay != 0 {
            continue;
        }
        for (hash, pending) in list_pending(snapshot, proxy.delegate).await? {
            let Ok(group) = Multisig::new(&snapshot.block().runtime, &[pending.depositor, sudo], 2)
            else {
                continue;
            };
            if group.address != proxy.delegate {
                continue;
            }
            records.push(json!({"kind":"runtime-upgrade-proposal","call_hash":hex_hash(hash),"timepoint":pending.when,"timepoint_display":format!("{}:{}",pending.when.height,pending.when.index),"sudo_key":sudo,"deployment_multisig":proxy.delegate,"ci_address":pending.depositor,"deployment_approvals":pending.approvals}));
        }
    }
    Ok(records)
}
async fn load_pending_upgrades(
    ctx: &AppContext,
    snapshot: &Snapshot,
    sudo: AccountId,
    fetch: &dyn UpgradeFetch,
) -> Result<Vec<Value>, CliError> {
    let mut records = discover_pending_upgrades(snapshot, sudo).await?;
    if records.is_empty() {
        return Ok(records);
    }
    let manifests = release_manifests(fetch).await;
    for record in &mut records {
        let hash = record["call_hash"].as_str().unwrap_or("").to_owned();
        let Some(manifest) = manifests.iter().find(|manifest| {
            manifest
                .get("call_hash")
                .and_then(Value::as_str)
                .is_some_and(|candidate| candidate.eq_ignore_ascii_case(&hash))
        }) else {
            continue;
        };
        record["release"] = json!({"url":manifest.get("release_url"),"repo":manifest.get("repo"),"tag":manifest.get("tag"),"commit":manifest.get("commit"),"spec_version":manifest.get("spec_version"),"prerelease":manifest.get("prerelease")});
        if let Some(url) = manifest
            .get("release_url")
            .and_then(Value::as_str)
            .filter(|url| !url.is_empty())
        {
            // Upgrade signing is provided by the pinned Python CLI; this read
            // path does not advertise an unimplemented native signing command.
            let prefix = if ctx.globals.network == "finney" {
                "btcli".into()
            } else {
                format!("btcli -n {}", ctx.globals.network)
            };
            record["sign_command"] = json!(format!(
                "{prefix} upgrade sign --url {url} -w <your-wallet>"
            ));
        }
        if let Some(threshold) = manifest
            .get("sudo")
            .and_then(|sudo| sudo.get("threshold"))
            .filter(|value| truthy(value))
        {
            let threshold = threshold
                .as_u64()
                .or_else(|| threshold.as_str().and_then(|value| value.parse().ok()));
            if let Some(threshold) = threshold {
                record["sudo_threshold"] = json!(threshold);
            }
        }
        let Some(url) = manifest
            .get("assets")
            .and_then(|assets| assets.get("call_data"))
            .and_then(Value::as_str)
        else {
            continue;
        };
        let Ok(bytes) = fetch.bytes(url, false).await else {
            continue;
        };
        if bytes.len() > MAX_FETCH_BYTES {
            continue;
        }
        let Some(blob) = std::str::from_utf8(&bytes)
            .ok()
            .and_then(|text| parse_blob(text).ok())
        else {
            continue;
        };
        if hash_blob(&blob).map(hex_hash).ok().as_deref() != Some(hash.as_str()) {
            continue;
        }
        let call = CallBytes::decode(&blob, &snapshot.block().runtime).map_err(failure)?;
        let ci: AccountId =
            serde_json::from_value(record["ci_address"].clone()).map_err(failure)?;
        let when: MultisigTimepoint =
            serde_json::from_value(record["timepoint"].clone()).map_err(failure)?;
        let weight = DynValue::new(CoreValue::record(vec![
            ("ref_time".into(), CoreValue::Uint(80_000_000_000)),
            ("proof_size".into(), CoreValue::Uint(50_000)),
        ]));
        let finalizing =
            rtao_runtime::calls::multisig::as_multi(2, vec![ci], Some(when), call, weight)
                .compose(&snapshot.block().runtime)
                .map_err(failure)?;
        let pending = snapshot
            .storage(&st::multisig::MULTISIGS, (sudo, finalizing.hash()))
            .await
            .map_err(failure)?;
        record["sudo_layer"] = pending.map_or(Value::Null,|pending|json!({"call_hash":hex_hash(finalizing.hash()),"timepoint":pending.when,"approvals":pending.approvals,"depositor":pending.depositor}));
    }
    Ok(records)
}
fn upgrade_fields(record: &Value) -> Vec<(String, Value)> {
    let mut fields = Vec::new();
    let mut push = |key: &str, value: Value| fields.push((key.to_owned(), value));
    let release = record.get("release").filter(|value| !value.is_null());
    if let Some(release) = release {
        if let Some(version) = release.get("spec_version").filter(|value| !value.is_null()) {
            push("spec_version", version.clone());
        }
        push(
            "release",
            release.get("url").cloned().unwrap_or(Value::Null),
        );
        if let Some(tag) = release
            .get("tag")
            .and_then(Value::as_str)
            .filter(|tag| !tag.is_empty())
        {
            push(
                "source",
                json!(format!(
                    "{}@{tag} ({})",
                    release
                        .get("repo")
                        .and_then(Value::as_str)
                        .unwrap_or("None"),
                    release
                        .get("commit")
                        .and_then(Value::as_str)
                        .unwrap_or("None")
                )),
            );
        }
    }
    for (key, source) in [
        ("call_hash", "call_hash"),
        ("proposed at", "timepoint_display"),
        ("CI key", "ci_address"),
        ("deployment multisig", "deployment_multisig"),
        ("sudo key", "sudo_key"),
    ] {
        push(key, record.get(source).cloned().unwrap_or(Value::Null));
    }
    let approvals = record
        .get("deployment_approvals")
        .and_then(Value::as_array)
        .map_or(0, Vec::len);
    push(
        "deployment layer",
        json!(format!("{approvals} of 2 approvals (CI)")),
    );
    if let Some(layer) = record.get("sudo_layer").filter(|value| !value.is_null()) {
        let approvals = layer
            .get("approvals")
            .and_then(Value::as_array)
            .map_or(0, Vec::len);
        let threshold = record
            .get("sudo_threshold")
            .and_then(Value::as_u64)
            .unwrap_or(0);
        let summary = if threshold == 0 {
            format!("{approvals} approvals so far")
        } else {
            format!("{approvals} of {threshold} approvals")
        };
        push(
            "sudo multisig layer",
            json!(format!(
                "{summary} — opened at {}:{}",
                layer["timepoint"]["height"], layer["timepoint"]["index"]
            )),
        );
    } else if release.is_some() && record.get("sudo_layer").is_some() {
        push(
            "sudo multisig layer",
            json!("not opened yet — the next signer opens it"),
        );
    } else if release.is_some() {
        push(
            "sudo multisig layer",
            json!("unknown — proposal call data could not be authenticated"),
        );
    }
    if let Some(command) = record.get("sign_command") {
        push("sign with", command.clone());
    } else if release.is_none() {
        push(
            "note",
            json!(
                "no matching release manifest found — pass the proposal's release URL to `btcli misc upgrade check/sign` directly"
            ),
        );
    }
    fields
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        application::command_tree,
        books::Books,
        globals::Globals,
        output::{MemorySink, OutputOptions, Stream},
        prompt::Terminal,
    };
    use rtao::{
        client::{
            ClientConfig, Network,
            config::{ConfigFile, Paths},
        },
        primitives::TaoBalance,
    };
    use rtao_testkit::{FakeChain, FakeState, rpc::RpcClient};
    use std::{
        collections::BTreeMap,
        fs,
        path::{Path, PathBuf},
        sync::{
            Arc, Mutex,
            atomic::{AtomicU64, Ordering},
        },
    };
    type TestResult = Result<(), Box<dyn std::error::Error>>;

    struct NoTerminal;
    impl Terminal for NoTerminal {
        fn stdin_is_tty(&self) -> bool {
            false
        }
        fn stderr_is_tty(&self) -> bool {
            false
        }
        fn read_line(&mut self, _: &str) -> Result<String, PromptError> {
            Err(PromptError::Unavailable)
        }
        fn read_secret(&mut self, _: &str) -> Result<rtao::wallet::SecretString, PromptError> {
            Err(PromptError::Unavailable)
        }
        fn message(&mut self, _: &str) -> Result<(), PromptError> {
            Ok(())
        }
    }
    fn context(
        args: &[&str],
        books: Books,
    ) -> Result<(AppContext, ArgMatches, MemorySink), Box<dyn std::error::Error>> {
        let parsed = command_tree([command()]).try_get_matches_from(
            std::iter::once("rtao")
                .chain(std::iter::once("multisig"))
                .chain(args.iter().copied()),
        )?;
        let family = parsed.subcommand().ok_or("family")?.1;
        let leaf = family.subcommand().ok_or("leaf")?.1;
        let config = Map::new();
        let mut globals =
            Globals::resolve(&parsed, &config, Path::new("/fixture-no-real-home"), |_| {
                None
            })?;
        globals.apply_with_env(leaf, &reserved(family)?, |_| None)?;
        let mut ctx = AppContext::new(
            globals,
            config,
            books,
            Arc::new(Mutex::new(Box::new(NoTerminal))),
        );
        let sink = MemorySink::default();
        ctx.output = Output::new(
            OutputOptions {
                json_mode: true,
                quiet: false,
                network: Some("local".into()),
            },
            Arc::new(sink.clone()),
        );
        Ok((ctx, family.clone(), sink))
    }
    fn accounts() -> Vec<AccountId> {
        vec![
            AccountId([11; 32]),
            AccountId([12; 32]),
            AccountId([13; 32]),
        ]
    }
    fn books() -> Books {
        let addresses: Vec<_> = ["alice", "bob", "carol", "alice-alias"]
            .into_iter()
            .zip([accounts()[0], accounts()[1], accounts()[2], accounts()[0]])
            .map(|(name, address)| json!({"name":name,"address":address}))
            .collect();
        Books::from_values(
            json!(addresses),
            json!([]),
            json!([{"name":"team","threshold":2,"signatories":["alice","bob","carol"],"note":"saved"}]),
        )
    }
    fn group() -> RawMultisig {
        RawMultisig {
            threshold: 2,
            signatories: accounts(),
            preset: Some("team".into()),
            refs: vec!["alice".into(), "bob".into(), "carol".into()],
        }
    }
    fn runtime() -> Result<RuntimeHandle, Box<dyn std::error::Error>> {
        Ok(RuntimeHandle::parse(
            include_bytes!("../../../../vendor/metadata/finney/478.v15.scale"),
            478,
            1,
            42,
        )?)
    }
    fn pending(height: u32, approvals: Vec<AccountId>) -> MultisigInfo {
        MultisigInfo {
            when: MultisigTimepoint { height, index: 1 },
            deposit: TaoBalance(99),
            depositor: accounts()[0],
            approvals,
        }
    }
    fn seed_pending(
        state: &mut FakeState,
        address: AccountId,
        hash: [u8; 32],
        value: &MultisigInfo,
    ) -> Result<(), Box<dyn std::error::Error>> {
        state.seed_python("Multisig","Multisigs",&[json!(address),json!(hex_hash(hash))],json!({"when":value.when,"deposit":value.deposit.0,"depositor":value.depositor,"approvals":value.approvals}))?;
        Ok(())
    }
    async fn client(state: FakeState) -> Result<(Client, FakeChain), Box<dyn std::error::Error>> {
        let chain = FakeChain::from_state(state);
        let mut config = ClientConfig::new(Network::resolve_with("local", |_| None)?)?;
        config.track_heads = false;
        config.runtime_cache_dir = None;
        config.config_store = None;
        let client = Client::from_rpc(RpcClient::new(chain.clone()), config).await?;
        Ok((client, chain))
    }
    fn clear_sudo(state: &mut FakeState) -> Result<(), Box<dyn std::error::Error>> {
        let storage = state
            .runtime
            .storage_entry("Sudo", "Key")
            .ok_or("sudo metadata")?;
        let key = state.runtime.storage_key(storage, &[])?;
        state.seed_encoded(key, None);
        Ok(())
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
                    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                        .join("../../target/m5-multisig-command-tests")
                });
            let root = base.join("tmp").join(format!(
                "multisig-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir_all(&root)?;
            let paths = Paths::new(&root, |name| match name {
                "BTCLI_MULTISIGS_PATH" => Some(root.join("shared/book.json").into_os_string()),
                "BTCLI_MULTISIG_CACHE" => Some(root.join("shared/calls.json").into_os_string()),
                _ => None,
            });
            Ok(Self { root, paths })
        }
    }
    impl Drop for Files {
        fn drop(&mut self) {
            // Awaited workers have finished. These fixture-owned public files
            // have no processes, launchd jobs, symlinks or virtualenv consumers.
            let _ = fs::remove_dir_all(&self.root);
        }
    }
    #[derive(Default)]
    struct Fetch {
        responses: BTreeMap<String, Vec<u8>>,
        requests: Mutex<Vec<(String, bool)>>,
    }
    impl UpgradeFetch for Fetch {
        fn bytes<'a>(
            &'a self,
            url: &'a str,
            api: bool,
        ) -> BoxFuture<'a, Result<Vec<u8>, CliError>> {
            Box::pin(async move {
                self.requests
                    .lock()
                    .map_err(|_| failure("fetch recorder"))?
                    .push((url.into(), api));
                self.responses
                    .get(url)
                    .cloned()
                    .ok_or_else(|| failure("fixture network unavailable"))
            })
        }
    }
    fn output(sink: &MemorySink) -> Result<Value, Box<dyn std::error::Error>> {
        Ok(serde_json::from_str(&sink.text(Stream::Stdout)?)?)
    }

    #[test]
    fn parser_retains_portable_refs_and_checks_threshold_after_account_deduplication() -> TestResult
    {
        let (mut ctx, matches, _) = context(
            &[
                "add",
                "new",
                "--threshold",
                "2",
                "--signatories",
                "alice,bob alice",
                "--signatory",
                "carol,alice-alias",
                "--signatory",
                "bob",
                "--note",
                "portable",
            ],
            books(),
        )?;
        let Request::Add {
            entry,
            group,
            overwrite,
        } = Request::from_matches(&mut ctx, &matches, &mut PromptHistory::default())?
        else {
            return Err("add request".into());
        };
        assert_eq!(
            entry,
            json!({"name":"new","threshold":2,"signatories":["alice","bob","carol","alice-alias"],"note":"portable"})
        );
        assert_eq!(group.signatories, accounts());
        assert!(!overwrite);
        assert!(ctx.globals.signatories.is_empty());
        let (mut ctx, matches, _) = context(
            &[
                "add",
                "new",
                "--threshold",
                "2",
                "--signatories",
                "alice,alice-alias",
            ],
            books(),
        )?;
        let error = Request::from_matches(&mut ctx, &matches, &mut PromptHistory::default())
            .err()
            .ok_or("threshold error")?;
        assert_eq!(error.message, "threshold 2 exceeds 1 signatories");
        for args in [
            vec![
                "add",
                "new",
                "--threshold",
                "0",
                "--signatories",
                "alice,bob",
            ],
            vec!["pending", "--signer", "ledger"],
        ] {
            assert!(context(&args, books()).is_err());
        }
        Ok(())
    }

    #[test]
    fn pending_uses_saved_wallet_and_rejects_inline_preset_ambiguity_before_connection()
    -> TestResult {
        let (mut ctx, matches, _) = context(&["pending", "-w", "team"], books())?;
        let Request::Pending { group, .. } =
            Request::from_matches(&mut ctx, &matches, &mut PromptHistory::default())?
        else {
            return Err("pending request".into());
        };
        assert_eq!(group.preset.as_deref(), Some("team"));
        assert_eq!(group.signatories, accounts());
        let (mut ctx, matches, _) = context(
            &[
                "pending",
                "--multisig",
                "team",
                "--multisig-threshold",
                "2",
                "--signatories",
                "alice,bob",
            ],
            books(),
        )?;
        assert!(Request::from_matches(&mut ctx, &matches, &mut PromptHistory::default()).is_err());
        let (mut ctx, matches, _) = context(
            &[
                "pending",
                "--multisig-threshold",
                "2",
                "--signatories",
                "alice,bob",
                "--other-signatories",
                "carol",
            ],
            books(),
        )?;
        assert!(Request::from_matches(&mut ctx, &matches, &mut PromptHistory::default()).is_err());
        let (mut ctx, matches, _) = context(&["pending", "-w", "unknown"], books())?;
        let error = Request::from_matches(&mut ctx, &matches, &mut PromptHistory::default())
            .err()
            .ok_or("missing preset")?;
        assert_eq!(error.message, "no multisig configured for wallet 'unknown'");
        assert!(error.help.ok_or("help")?.contains("team"));
        Ok(())
    }

    #[tokio::test]
    async fn offline_list_remove_use_the_shared_override_without_connecting() -> TestResult {
        let files = Files::new()?;
        let row = json!({"name":"team","threshold":2,"signatories":["alice","bob"],"note":"retained","extra":{"future":true}});
        MultisigBook::new(&files.paths)
            .add(row.clone(), false)
            .await?;
        let (ctx, _, sink) = context(&["list"], books())?;
        execute(&ctx, &files.paths, &Request::List, None).await?;
        assert_eq!(
            output(&sink)?,
            json!({"path":files.paths.path(ConfigFile::Multisigs),"multisigs":[row]})
        );
        let (ctx, _, sink) = context(&["remove", "TEAM"], books())?;
        execute(
            &ctx,
            &files.paths,
            &Request::Remove {
                name: "TEAM".into(),
            },
            None,
        )
        .await?;
        assert_eq!(output(&sink)?, json!({"name":"TEAM","existed":false}));
        let (ctx, _, sink) = context(&["remove", "team"], books())?;
        execute(
            &ctx,
            &files.paths,
            &Request::Remove {
                name: "team".into(),
            },
            None,
        )
        .await?;
        assert_eq!(output(&sink)?, json!({"name":"team","existed":true}));
        assert_eq!(
            fs::read_to_string(files.paths.path(ConfigFile::Multisigs))?,
            "[]\n"
        );
        assert!(!files.paths.path(ConfigFile::MultisigCache).exists());
        Ok(())
    }

    #[test]
    fn pending_other_signatories_uses_selected_public_hotkey_without_unlocking() -> TestResult {
        let files = Files::new()?;
        let hot = AccountId([44; 32]);
        let wallet = files.root.join("wallets/member");
        fs::create_dir_all(wallet.join("hotkeys"))?;
        fs::write(
            wallet.join("coldkeypub.txt"),
            serde_json::to_vec(&json!({"ss58Address":accounts()[0],"cryptoType":1}))?,
        )?;
        fs::write(
            wallet.join("hotkeys/default"),
            serde_json::to_vec(&json!({"ss58Address":hot,"cryptoType":1}))?,
        )?;
        let (mut ctx, matches, _) = context(
            &[
                "pending",
                "-w",
                "member",
                "--multisig-threshold",
                "2",
                "--other-signatories",
                "alice,bob",
                "--signer",
                "hotkey",
            ],
            books(),
        )?;
        ctx.globals.wallet_path = files.root.join("wallets");
        let Request::Pending { group, .. } =
            Request::from_matches(&mut ctx, &matches, &mut PromptHistory::default())?
        else {
            return Err("pending request".into());
        };
        assert_eq!(group.signatories, vec![accounts()[0], accounts()[1], hot]);
        assert_eq!(group.refs, vec!["alice", "bob", "member"]);
        assert_eq!(ctx.globals.signer_backend, None);
        Ok(())
    }

    #[tokio::test]
    async fn accepted_add_survives_unavailable_client_without_duplicate_persistence() -> TestResult
    {
        let files = Files::new()?;
        let (mut ctx, matches, _) = context(
            &[
                "add",
                "new",
                "--threshold",
                "2",
                "--signatories",
                "alice,bob",
            ],
            books(),
        )?;
        let request = Request::from_matches(&mut ctx, &matches, &mut PromptHistory::default())?;
        let error = execute(&ctx, &files.paths, &request, None)
            .await
            .err()
            .ok_or("client unavailable")?;
        assert_eq!(error.message, "this multisig command requires a client");
        assert_eq!(
            MultisigBook::new(&files.paths).load().await?,
            vec![json!({"name":"new","threshold":2,"signatories":["alice","bob"],"note":""})]
        );
        let before = fs::read(files.paths.path(ConfigFile::Multisigs))?;
        assert!(execute(&ctx, &files.paths, &request, None).await.is_err());
        assert_eq!(fs::read(files.paths.path(ConfigFile::Multisigs))?, before);
        Ok(())
    }

    #[tokio::test]
    async fn opening_recovery_uses_hex_rpc_hash_and_preserves_exact_nested_byte_fields()
    -> TestResult {
        let runtime = runtime()?;
        let inner = rtao_runtime::calls::system::remark(b"0x41".to_vec()).compose(&runtime)?;
        let outer = rtao_runtime::calls::multisig::as_multi(
            2,
            vec![accounts()[1]],
            None,
            inner.clone(),
            DynValue::new(CoreValue::record(vec![
                ("ref_time".into(), CoreValue::Uint(1_000_000)),
                ("proof_size".into(), CoreValue::Uint(1000)),
            ])),
        )
        .compose(&runtime)?;
        let mut body = vec![4];
        body.extend_from_slice(outer.as_bytes());
        let mut extrinsic = Vec::new();
        rtao::core::codec::encode::compact(body.len() as u128, &mut extrinsic)?;
        extrinsic.extend(body);
        assert_eq!(
            opening_inner_bytes(&extrinsic, &runtime),
            Some(inner.as_bytes().to_vec())
        );
        let mut state = FakeState::try_default()?;
        state.set_result(
            "chain_getBlock",
            json!({"block":{"extrinsics":[format!("0x{}",hex::encode(extrinsic))]}}),
        );
        let (client, chain) = client(state).await?;
        let recovered = opening_call(
            &client,
            &MultisigTimepoint {
                height: 100,
                index: 0,
            },
            inner.hash(),
            &runtime,
        )
        .await;
        assert_eq!(recovered, Some(inner));
        {
            let state = chain.state();
            let locked = state.lock().map_err(|_| "state")?;
            let call = locked
                .requests
                .iter()
                .find(|request| request.method == "chain_getBlock")
                .ok_or("block RPC")?;
            let hash = call.params[0]
                .as_str()
                .ok_or("block hash must be an RPC hex string")?;
            assert!(hash.starts_with("0x"));
            assert_eq!(hash.len(), 66);
        }
        client.close().await;
        Ok(())
    }

    #[tokio::test]
    async fn add_persists_original_refs_and_show_derives_live_sudo_address() -> TestResult {
        let files = Files::new()?;
        let group = group();
        let ms = Multisig::new(&runtime()?, &group.signatories, group.threshold)?;
        let mut state = FakeState::try_default()?;
        state.seed_python("Sudo", "Key", &[], json!(ms.address))?;
        let (client, chain) = client(state).await?;
        let (mut ctx, matches, sink) = context(
            &[
                "add",
                "team",
                "--threshold",
                "2",
                "--signatories",
                "alice,bob,carol",
                "--note",
                "portable",
                "--overwrite",
            ],
            books(),
        )?;
        let request = Request::from_matches(&mut ctx, &matches, &mut PromptHistory::default())?;
        execute(&ctx, &files.paths, &request, Some(&client)).await?;
        let result = output(&sink)?;
        assert_eq!(
            result["entry"]["signatories"],
            json!(["alice", "bob", "carol"])
        );
        assert_eq!(result["multisig_address"], json!(ms.address));
        assert_eq!(result["is_chain_sudo_key"], true);
        assert_eq!(result["coldkey_wallet_also_exists"], false);
        assert_eq!(
            MultisigBook::new(&files.paths).load().await?,
            vec![result["entry"].clone()]
        );
        let (ctx, _, sink) = context(&["show", "team"], books())?;
        execute(
            &ctx,
            &files.paths,
            &Request::Show {
                name: "team".into(),
                group,
            },
            Some(&client),
        )
        .await?;
        assert_eq!(
            output(&sink)?,
            json!({"threshold":2,"signatories":accounts(),"multisig_address":ms.address,"is_chain_sudo_key":true})
        );
        assert!(
            chain
                .state()
                .lock()
                .map_err(|_| "state")?
                .submissions
                .is_empty()
        );
        client.close().await;
        Ok(())
    }

    #[tokio::test]
    async fn pending_authenticates_shared_cache_bytes_and_preserves_unknown_pending_rows()
    -> TestResult {
        let files = Files::new()?;
        let runtime = runtime()?;
        let group = group();
        let ms = Multisig::new(&runtime, &group.signatories, group.threshold)?;
        let known = rtao_runtime::calls::system::remark(vec![1, 2]).compose(&runtime)?;
        let unknown = [0xab; 32];
        let cache = FsRawCallCache::new(&files.paths);
        cache.save(hex_hash(known.hash()),json!({"target":"Balances.transfer_all","params":{"dest":"untrusted-presentation"},"call_data":format!("0x{}",hex::encode(known.as_bytes()))})).await?;
        let mut state = FakeState::try_default()?;
        clear_sudo(&mut state)?;
        seed_pending(
            &mut state,
            ms.address,
            known.hash(),
            &pending(90, vec![accounts()[0]]),
        )?;
        seed_pending(
            &mut state,
            ms.address,
            unknown,
            &pending(99, vec![accounts()[0], accounts()[1]]),
        )?;
        // A different account's pending entry must never leak into the list.
        seed_pending(
            &mut state,
            AccountId([99; 32]),
            [0xcd; 32],
            &pending(100, vec![]),
        )?;
        let (client, chain) = client(state).await?;
        let (ctx, _, sink) = context(&["pending", "--multisig", "team"], books())?;
        let fetch = Fetch::default();
        execute_with_fetch(
            &ctx,
            &files.paths,
            &Request::Pending {
                group,
                call_hash: None,
                call_data: Some("not hex and intentionally ignored without a filter".into()),
            },
            Some(&client),
            &fetch,
        )
        .await?;
        let result = output(&sink)?;
        assert_eq!(result["count"], 2);
        let rows = result["pending_multisigs"]
            .as_array()
            .ok_or("pending rows")?;
        assert_eq!(rows[0]["call_hash"], hex_hash(unknown));
        assert_eq!(rows[0]["timepoint_display"], "99:1");
        assert_eq!(rows[0]["approvals"], 2);
        assert_eq!(rows[0]["remaining_signatories"], json!([accounts()[2]]));
        assert_eq!(rows[0]["call_data"], Value::Null);
        assert_eq!(rows[0]["commands_available"], false);
        assert!(
            rows[0]["decode_hint"]
                .as_str()
                .ok_or("hint")?
                .contains("--call-data")
        );
        assert_eq!(rows[1]["target"], "System.remark");
        assert_eq!(rows[1]["params"]["remark"], "0x0102");
        assert_eq!(
            rows[1]["approval_labels"],
            json!([format!("alice ({})", accounts()[0])])
        );
        assert_eq!(
            rows[1]["co_signer_commands"]
                .as_array()
                .ok_or("commands")?
                .len(),
            2
        );
        assert!(
            rows[1]["co_signer_commands"][0]["command"]
                .as_str()
                .ok_or("replay")?
                .contains("--signatory bob")
        );
        assert!(fetch.requests.lock().map_err(|_| "fetches")?.is_empty());
        assert!(
            chain
                .state()
                .lock()
                .map_err(|_| "state")?
                .submissions
                .is_empty()
        );
        client.close().await;
        Ok(())
    }

    #[tokio::test]
    async fn explicit_call_data_must_match_pending_hash_and_only_then_updates_shared_cache()
    -> TestResult {
        let files = Files::new()?;
        let runtime = runtime()?;
        let group = group();
        let ms = Multisig::new(&runtime, &group.signatories, group.threshold)?;
        let call = rtao_runtime::calls::system::remark(vec![3, 4]).compose(&runtime)?;
        let mut state = FakeState::try_default()?;
        clear_sudo(&mut state)?;
        seed_pending(
            &mut state,
            ms.address,
            call.hash(),
            &pending(88, vec![accounts()[0]]),
        )?;
        let (client, _) = client(state).await?;
        let (ctx, _, _) = context(&["pending", "--multisig", "team"], books())?;
        let request = Request::Pending {
            group: group.clone(),
            call_hash: Some(call.hash()),
            call_data: Some("0x0102".into()),
        };
        let error = execute_with_fetch(
            &ctx,
            &files.paths,
            &request,
            Some(&client),
            &Fetch::default(),
        )
        .await
        .err()
        .ok_or("mismatch")?;
        assert!(
            error
                .message
                .starts_with("--call-data does not match call hash")
        );
        assert!(!files.paths.path(ConfigFile::MultisigCache).exists());
        let (ctx, _, sink) = context(&["pending", "--multisig", "team"], books())?;
        let request = Request::Pending {
            group,
            call_hash: Some(call.hash()),
            call_data: Some(format!("0x{}", hex::encode(call.as_bytes()))),
        };
        execute_with_fetch(
            &ctx,
            &files.paths,
            &request,
            Some(&client),
            &Fetch::default(),
        )
        .await?;
        assert_eq!(output(&sink)?["count"], 1);
        assert_eq!(
            FsRawCallCache::new(&files.paths)
                .load_call(&hex_hash(call.hash()), &runtime)
                .await?,
            Some(call)
        );
        client.close().await;
        Ok(())
    }

    #[tokio::test]
    async fn pending_sudo_includes_only_authenticated_deployment_proposals_and_advisory_release()
    -> TestResult {
        let files = Files::new()?;
        let runtime = runtime()?;
        let group = group();
        let sudo = Multisig::new(&runtime, &group.signatories, group.threshold)?.address;
        let ci = AccountId([20; 32]);
        let delegate = Multisig::new(&runtime, &[ci, sudo], 2)?.address;
        let other_ci = AccountId([21; 32]);
        let delayed = Multisig::new(&runtime, &[other_ci, sudo], 2)?.address;
        let call = rtao_runtime::calls::system::remark(vec![9]).compose(&runtime)?;
        let when = MultisigTimepoint {
            height: 70,
            index: 3,
        };
        let finalizing = rtao_runtime::calls::multisig::as_multi(
            2,
            vec![ci],
            Some(when.clone()),
            call.clone(),
            DynValue::new(CoreValue::record(vec![
                ("ref_time".into(), CoreValue::Uint(80_000_000_000)),
                ("proof_size".into(), CoreValue::Uint(50_000)),
            ])),
        )
        .compose(&runtime)?;
        let mut state = FakeState::try_default()?;
        state.seed_python("Sudo", "Key", &[], json!(sudo))?;
        state.seed_python("Proxy","Proxies",&[json!(sudo)],json!([[{"delegate":delegate,"proxy_type":"SudoUncheckedSetCode","delay":0},{"delegate":delayed,"proxy_type":"SudoUncheckedSetCode","delay":1},{"delegate":AccountId([80;32]),"proxy_type":"Any","delay":0}],0]))?;
        seed_pending(
            &mut state,
            delegate,
            call.hash(),
            &MultisigInfo {
                when: when.clone(),
                deposit: TaoBalance(100),
                depositor: ci,
                approvals: vec![ci],
            },
        )?;
        // Same delegate with a different depositor does not rederive its identity.
        seed_pending(
            &mut state,
            delegate,
            [0xcd; 32],
            &MultisigInfo {
                depositor: other_ci,
                ..pending(99, vec![other_ci])
            },
        )?;
        seed_pending(
            &mut state,
            delayed,
            [0xef; 32],
            &MultisigInfo {
                depositor: other_ci,
                ..pending(98, vec![other_ci])
            },
        )?;
        seed_pending(
            &mut state,
            sudo,
            finalizing.hash(),
            &pending(90, vec![accounts()[0]]),
        )?;
        let (client, chain) = client(state).await?;
        let api = format!("https://api.github.com/repos/{UPGRADE_REPO}/releases?per_page=15");
        let manifest_url = "https://fixture.invalid/manifest.json";
        let blob_url = "https://fixture.invalid/blob.hex";
        let release = "https://github.com/RaoFoundation/subtensor/releases/tag/v999-proposal";
        let fetch = Fetch {
            responses: BTreeMap::from([
                (
                    api.clone(),
                    serde_json::to_vec(
                        &json!([{"html_url":release,"prerelease":true,"assets":[{"name":"upgrade-manifest.json","browser_download_url":manifest_url}]}]),
                    )?,
                ),
                (
                    manifest_url.into(),
                    serde_json::to_vec(
                        &json!({"call_hash":hex_hash(call.hash()).to_uppercase(),"repo":UPGRADE_REPO,"tag":"v999","commit":"deadbeef","spec_version":999,"sudo":{"threshold":2},"assets":{"call_data":blob_url}}),
                    )?,
                ),
                (
                    blob_url.into(),
                    format!("0x{}\n", hex::encode(call.as_bytes())).into_bytes(),
                ),
            ]),
            ..Fetch::default()
        };
        let (ctx, _, sink) = context(&["pending", "--multisig", "team", "-n", "local"], books())?;
        execute_with_fetch(
            &ctx,
            &files.paths,
            &Request::Pending {
                group,
                call_hash: None,
                call_data: None,
            },
            Some(&client),
            &fetch,
        )
        .await?;
        let result = output(&sink)?;
        assert_eq!(result["count"], 1);
        let upgrades = result["pending_upgrades"].as_array().ok_or("upgrades")?;
        assert_eq!(upgrades.len(), 1);
        let upgrade = &upgrades[0];
        assert_eq!(upgrade["deployment_multisig"], json!(delegate));
        assert_eq!(upgrade["ci_address"], json!(ci));
        assert_eq!(upgrade["timepoint"], json!(when));
        assert_eq!(
            upgrade["release"],
            json!({"url":release,"repo":UPGRADE_REPO,"tag":"v999","commit":"deadbeef","spec_version":999,"prerelease":true})
        );
        assert_eq!(
            upgrade["sudo_layer"]["call_hash"],
            hex_hash(finalizing.hash())
        );
        assert_eq!(upgrade["sudo_layer"]["approvals"], json!([accounts()[0]]));
        assert_eq!(
            upgrade["sign_command"],
            format!("btcli -n local upgrade sign --url {release} -w <your-wallet>")
        );
        assert_eq!(
            fetch.requests.lock().map_err(|_| "fetches")?.as_slice(),
            &[
                (api, true),
                (manifest_url.into(), false),
                (blob_url.into(), false)
            ]
        );
        assert!(
            chain
                .state()
                .lock()
                .map_err(|_| "state")?
                .submissions
                .is_empty()
        );
        client.close().await;
        Ok(())
    }

    #[tokio::test]
    async fn unavailable_manifests_preserve_discovery_and_filtered_queries_skip_upgrade_fetches()
    -> TestResult {
        let files = Files::new()?;
        let runtime = runtime()?;
        let group = group();
        let sudo = Multisig::new(&runtime, &group.signatories, group.threshold)?.address;
        let ci = AccountId([20; 32]);
        let delegate = Multisig::new(&runtime, &[ci, sudo], 2)?.address;
        let mut state = FakeState::try_default()?;
        state.seed_python("Sudo", "Key", &[], json!(sudo))?;
        state.seed_python(
            "Proxy",
            "Proxies",
            &[json!(sudo)],
            json!([[{"delegate":delegate,"proxy_type":"SudoUncheckedSetCode","delay":0}],0]),
        )?;
        seed_pending(
            &mut state,
            delegate,
            [0xab; 32],
            &MultisigInfo {
                depositor: ci,
                ..pending(70, vec![ci])
            },
        )?;
        let (client, _) = client(state).await?;
        let fetch = Fetch::default();
        let (ctx, _, sink) = context(&["pending", "--multisig", "team"], books())?;
        execute_with_fetch(
            &ctx,
            &files.paths,
            &Request::Pending {
                group: group.clone(),
                call_hash: None,
                call_data: None,
            },
            Some(&client),
            &fetch,
        )
        .await?;
        let result = output(&sink)?;
        assert_eq!(
            result["pending_upgrades"]
                .as_array()
                .ok_or("upgrades")?
                .len(),
            1
        );
        assert!(result["pending_upgrades"][0].get("release").is_none());
        assert_eq!(fetch.requests.lock().map_err(|_| "fetches")?.len(), 1);
        let fetch = Fetch::default();
        let (ctx, _, sink) = context(&["pending", "--multisig", "team"], books())?;
        execute_with_fetch(
            &ctx,
            &files.paths,
            &Request::Pending {
                group,
                call_hash: Some([0xff; 32]),
                call_data: None,
            },
            Some(&client),
            &fetch,
        )
        .await?;
        assert_eq!(output(&sink)?, json!({"pending_multisigs":[],"count":0}));
        assert!(fetch.requests.lock().map_err(|_| "fetches")?.is_empty());
        client.close().await;
        Ok(())
    }

    #[test]
    fn unknown_upgrade_layer_is_not_rendered_as_absent_after_failed_asset_enrichment() -> TestResult
    {
        let mut record = json!({"release":{"url":"https://fixture.invalid/release","tag":"v999","repo":UPGRADE_REPO,"commit":"source"},"call_hash":hex_hash([1;32]),"timepoint_display":"10:2","ci_address":accounts()[0],"deployment_multisig":accounts()[1],"sudo_key":accounts()[2],"deployment_approvals":[accounts()[0]]});
        let fields: Map<String, Value> = upgrade_fields(&record).into_iter().collect();
        assert_eq!(
            fields["sudo multisig layer"],
            "unknown — proposal call data could not be authenticated"
        );
        record["sudo_layer"] = Value::Null;
        let fields: Map<String, Value> = upgrade_fields(&record).into_iter().collect();
        assert_eq!(
            fields["sudo multisig layer"],
            "not opened yet — the next signer opens it"
        );
        Ok(())
    }
}
