// Derived from RaoFoundation/subtensor sdk/python/bittensor/cli/commands/addresses.py @ 850f2755bb05a3851f948489f0ab27b0645cebce (Apache-2.0); modified by rtao.
//! Offline named contacts and explicitly requested public Vault/extension imports.

use crate::{
    books::{AddressBook, is_bittensor_address, public_json_truthy},
    context::AppContext,
    error::CliError,
    globals::{Tier, with_globals},
    prompt::{PromptError, python_strip},
};
use clap::{Arg, ArgAction, ArgMatches, Command};
use rtao::{client::config::Paths, signer::BoxFuture};
use serde_json::{Value, json};

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
fn argument<'a>(leaf: &'a ArgMatches, name: &str) -> Option<&'a str> {
    leaf.try_get_one::<String>(name)
        .ok()
        .flatten()
        .map(String::as_str)
}

/// Public contact-import request. Neither variant signs or unlocks a wallet.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ContactImport {
    /// Scan an address QR from Polkadot Vault.
    Vault,
    /// Select a browser-extension public account.
    Extension,
}

/// Address returned by an injected public import boundary.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ImportedContact {
    /// Exact SS58 spelling to validate and save.
    pub address: String,
    /// Optional Vault network identity; another network is advisory only.
    pub genesis: Option<String>,
}

/// External address discovery, injectable without cameras, browsers or sockets.
pub trait ContactSource: Send + Sync {
    /// Discover one public contact only after all conflicting inputs are refused.
    fn import<'a>(
        &'a self,
        ctx: &'a AppContext,
        kind: ContactImport,
    ) -> BoxFuture<'a, Result<ImportedContact, CliError>>;
}

struct NativeContacts;
impl ContactSource for NativeContacts {
    fn import<'a>(
        &'a self,
        ctx: &'a AppContext,
        kind: ContactImport,
    ) -> BoxFuture<'a, Result<ImportedContact, CliError>> {
        Box::pin(async move {
            #[cfg(not(any(feature = "vault", feature = "extension")))]
            let _ = ctx;
            match kind {
                #[cfg(feature = "vault")]
                ContactImport::Vault => {
                    use rtao::signer::vault::{
                        VaultScanOptions, VaultServerOptions, scan_vault_address,
                    };
                    let output = ctx.output.clone();
                    let options = VaultScanOptions {
                        server: VaultServerOptions {
                            open_browser: visible_terminal(ctx)?,
                            browser: ctx.globals.extension_browser.clone(),
                            ..Default::default()
                        },
                        on_status: Some(std::sync::Arc::new(move |status| {
                            let _ = output.message(status);
                        })),
                        ..Default::default()
                    };
                    let (address, genesis) = scan_vault_address(&options).await.map_err(failure)?;
                    Ok(ImportedContact { address, genesis })
                }
                #[cfg(feature = "extension")]
                ContactImport::Extension => extension_contact(ctx).await,
                #[allow(unreachable_patterns)]
                _ => Err(CliError::input(format!(
                    "{} contact import requires the corresponding CLI feature",
                    match kind {
                        ContactImport::Vault => "Vault",
                        ContactImport::Extension => "extension",
                    }
                ))),
            }
        })
    }
}

#[cfg(any(feature = "vault", feature = "extension"))]
fn visible_terminal(ctx: &AppContext) -> Result<bool, CliError> {
    Ok(!ctx.globals.quiet
        && ctx
            .terminal
            .lock()
            .map_err(|_| failure("terminal lock poisoned"))?
            .stderr_is_tty())
}

#[cfg(feature = "extension")]
async fn extension_contact(ctx: &AppContext) -> Result<ImportedContact, CliError> {
    use rtao::signer::extension::{
        BridgeClient, EnsureBridgeOptions, account_crypto_type, ensure_bridge,
    };
    let mut options = EnsureBridgeOptions::from_environment().map_err(failure)?;
    if let Some(url) = &ctx.globals.extension_bridge_url {
        options.url = url.clone();
    }
    options.allow_spawn = true;
    options.open_browser = visible_terminal(ctx)?;
    options.browser = ctx.globals.extension_browser.clone();
    options.fresh = true;
    options.on_waiting = Some(super::extension::waiting_observer(&ctx.output));
    let url = ensure_bridge(&options).await.map_err(failure)?;
    let token = options
        .paths
        .read_token()
        .map_err(failure)?
        .ok_or_else(|| failure("extension bridge client token is missing"))?;
    let client = BridgeClient::new(&url, token.as_str()).map_err(failure)?;
    let accounts = client.list_accounts().await;
    client.close().await;
    let accounts = accounts.map_err(failure)?;
    let accounts: Vec<_> = accounts
        .iter()
        .filter(|row| {
            ctx.globals
                .extension_source
                .as_ref()
                .is_none_or(|source| source == &row.source)
        })
        .collect();
    let account = match accounts.as_slice() {
        [] => return Err(failure("no extension account matched the given filters")),
        [account] => *account,
        _ => {
            let mut terminal = ctx
                .terminal
                .lock()
                .map_err(|_| failure("terminal lock poisoned"))?;
            if ctx.globals.json || !terminal.stdin_is_tty() {
                return Err(failure(
                    "multiple extension accounts available; select a single --extension-source",
                ));
            }
            for (index, account) in accounts.iter().enumerate() {
                terminal
                    .message(&crate::output::terminal_text(&format!(
                        "{}: {} ({}, {})",
                        index + 1,
                        account.name,
                        account.address,
                        account.source
                    )))
                    .map_err(prompt_error)?;
            }
            loop {
                let answer = terminal
                    .read_line("Select extension account [1]: ")
                    .map_err(prompt_error)?;
                let answer = python_strip(&answer);
                let index = if answer.is_empty() {
                    Some(0)
                } else {
                    rtao::primitives::pyfmt::parse_integer(answer)
                        .and_then(|value| value.to_string().parse::<usize>().ok())
                        .and_then(|value| value.checked_sub(1))
                };
                if let Some(account) = index.and_then(|index| accounts.get(index)) {
                    break *account;
                }
                terminal
                    .message("Select one of the listed account numbers.")
                    .map_err(prompt_error)?;
            }
        }
    };
    account_crypto_type(&account.account_type).map_err(failure)?;
    ctx.output
        .message(&format!(
            "picked extension account {} ({}, {})",
            account.name, account.address, account.source
        ))
        .map_err(failure)?;
    Ok(ImportedContact {
        address: account.address.clone(),
        genesis: None,
    })
}

/// Build all five pinned contact commands; import backends remain explicit flags.
pub fn command() -> Command {
    let contact = |name: &'static str, required: bool| {
        Command::new(name)
            .arg(
                Arg::new("name")
                    .required(true)
                    .help("Contact name to save."),
            )
            .arg(
                Arg::new("ss58")
                    .required(required)
                    .help("SS58 address for that name."),
            )
            .arg(
                Arg::new("note")
                    .long("note")
                    .default_value("")
                    .help("Optional note stored with the entry."),
            )
    };
    let add = contact("add", false)
        .about("Save a named address, import one, or retag an existing contact.")
        .arg(
            Arg::new("from_vault")
                .long("vault")
                .action(ArgAction::SetTrue),
        )
        .arg(
            Arg::new("from_extension")
                .long("extension")
                .action(ArgAction::SetTrue),
        )
        .arg(
            Arg::new("contact_signer")
                .long("signer")
                .help("Contact signing backend: vault, ledger, or extension."),
        );
    // The command owns --signer as a contact tag, while root --signer retains
    // its ordinary meaning. Reservation prevents cross-routing those values.
    Command::new("addr")
        .about("Save and reuse named SS58 addresses.")
        .subcommand_required(true)
        .arg_required_else_help(true)
        .subcommands([
            with_globals(add, Tier::Read),
            with_globals(
                contact("save", true).about("Save a named address (alias for add)."),
                Tier::Read,
            ),
            with_globals(
                Command::new("list").about("List saved address-book entries."),
                Tier::Read,
            ),
            with_globals(
                Command::new("show")
                    .about("Show one saved address.")
                    .arg(Arg::new("name").required(true)),
                Tier::Read,
            ),
            with_globals(
                Command::new("remove")
                    .about("Remove a saved address.")
                    .arg(Arg::new("name").required(true)),
                Tier::Read,
            ),
        ])
}

/// Reserve the contact tag from root signing-backend resolution.
pub fn reserved(group: &ArgMatches) -> Vec<&'static str> {
    if group.subcommand_name() == Some("add") {
        vec!["contact_signer", "signer_backend"]
    } else {
        Vec::new()
    }
}

/// Execute using the native public-import boundary and explicit shared paths.
pub async fn run(ctx: &AppContext, paths: &Paths, group: &ArgMatches) -> Result<(), CliError> {
    run_with_source(ctx, paths, group, &NativeContacts).await
}

/// Execute entirely offline unless a Vault or extension import was requested.
pub async fn run_with_source(
    ctx: &AppContext,
    paths: &Paths,
    group: &ArgMatches,
    source: &dyn ContactSource,
) -> Result<(), CliError> {
    let (name, leaf) = group
        .subcommand()
        .ok_or_else(|| CliError::input("missing addr command"))?;
    let book = AddressBook::new(paths);
    match name {
        "list" => {
            let entries = book.load().await.map_err(failure)?;
            if ctx.globals.json {
                return ctx
                    .output
                    .value(&json!({"path":book.path(),"addresses":entries}))
                    .map_err(failure);
            }
            let nodes = entries
                .iter()
                .map(|row| {
                    let text = |key| {
                        row.get(key)
                            .and_then(Value::as_str)
                            .filter(|value| !value.is_empty())
                            .unwrap_or("—")
                            .to_owned()
                    };
                    let mut leaves = vec![text("address"), format!("note · {}", text("note"))];
                    if let Some(signer) =
                        row.get("signer").filter(|value| public_json_truthy(value))
                    {
                        leaves.push(format!(
                            "signer · {}",
                            crate::globals::config_string(signer)
                        ));
                    }
                    (
                        row.get("name")
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_owned(),
                        leaves,
                    )
                })
                .collect::<Vec<_>>();
            ctx.output
                .tree(
                    &format!("Addresses ({})", book.path().display()),
                    &nodes,
                    None,
                )
                .map_err(failure)?;
            ctx.output
                .value(&format!(
                    "{} {}",
                    entries.len(),
                    if entries.len() == 1 {
                        "entry"
                    } else {
                        "entries"
                    }
                ))
                .map_err(failure)
        }
        "show" => {
            let contact =
                argument(leaf, "name").ok_or_else(|| CliError::input("missing contact name"))?;
            let entries = book.load().await.map_err(failure)?;
            let entry = entries
                .iter()
                .find(|entry| entry.get("name").and_then(Value::as_str) == Some(contact))
                .ok_or_else(|| {
                    failure(format!(
                        "address {} not found",
                        rtao::primitives::pyfmt::string_repr(contact)
                    ))
                })?;
            detail(ctx, contact, entry)
        }
        "remove" => {
            let contact =
                argument(leaf, "name").ok_or_else(|| CliError::input("missing contact name"))?;
            let existed = book.remove(contact).await.map_err(failure)?;
            detail(
                ctx,
                "removed address",
                &json!({"name":contact,"existed":existed}),
            )
        }
        "add" | "save" => {
            let contact =
                argument(leaf, "name").ok_or_else(|| CliError::input("missing contact name"))?;
            let mut address = argument(leaf, "ss58").map(str::to_owned);
            let mut note = argument(leaf, "note").unwrap_or_default().to_owned();
            let vault = leaf
                .try_get_one::<bool>("from_vault")
                .ok()
                .flatten()
                .copied()
                .unwrap_or(false);
            let extension = leaf
                .try_get_one::<bool>("from_extension")
                .ok()
                .flatten()
                .copied()
                .unwrap_or(false);
            let mut signer =
                argument(leaf, "contact_signer").map(|value| python_strip(value).to_lowercase());
            if vault && extension {
                return Err(CliError::input(
                    "give either --vault or --extension, not both",
                ));
            }
            if (vault || extension) && address.is_some() {
                return Err(CliError::input(
                    "give either an SS58 address or --vault/--extension, not both",
                ));
            }
            if vault || extension {
                let implied = if vault { "vault" } else { "extension" };
                if signer.as_deref().is_some_and(|signer| signer != implied) {
                    return Err(CliError::input(format!(
                        "--{implied} implies --signer {implied}"
                    )));
                }
                signer = Some(implied.into());
            }
            if signer
                .as_deref()
                .is_some_and(|value| !matches!(value, "vault" | "ledger" | "extension"))
            {
                return Err(CliError::input(
                    "unknown --signer; pass vault, ledger, or extension",
                ));
            }
            if contact.is_empty() {
                return Err(failure("address entry requires a 'name' field"));
            }
            if vault || extension {
                let imported = source
                    .import(
                        ctx,
                        if vault {
                            ContactImport::Vault
                        } else {
                            ContactImport::Extension
                        },
                    )
                    .await?;
                if let Some(genesis) = imported.genesis.filter(|genesis| {
                    genesis.to_lowercase() != rtao_catalog::py::FINNEY_GENESIS_HASH
                }) {
                    let short: String = genesis.chars().take(10).collect();
                    ctx.output.message(&format!("note: the key was exported from a network with genesis {short}…, not Bittensor mainnet — saving the address anyway")).map_err(failure)?;
                }
                address = Some(imported.address);
            }
            if address.is_none() && signer.is_some() {
                let entries = book.load().await.map_err(failure)?;
                if let Some(existing) = entries.iter().find(|row| {
                    row.get("name").and_then(Value::as_str) == Some(contact)
                        || row.get("address").and_then(Value::as_str) == Some(contact)
                }) {
                    address = Some(
                        existing
                            .get("address")
                            .map(crate::globals::config_string)
                            .unwrap_or_default(),
                    );
                    if note.is_empty() {
                        note = existing
                            .get("note")
                            .filter(|value| public_json_truthy(value))
                            .map(crate::globals::config_string)
                            .unwrap_or_default();
                    }
                }
            }
            let address = match address {
                Some(address) => address,
                None => prompt_address(ctx)?,
            };
            if !is_bittensor_address(&address) {
                return Err(failure(format!(
                    "invalid ss58 address {}",
                    rtao::primitives::pyfmt::string_repr(&address)
                )));
            }
            let entry = book
                .save_contact(contact.into(), address, note, signer)
                .await
                .map_err(failure)?;
            detail(
                ctx,
                "saved address",
                &json!({"entry":entry,"path":book.path()}),
            )
        }
        _ => Err(CliError::input(format!("unknown addr command {name:?}"))),
    }
}

fn prompt_address(ctx: &AppContext) -> Result<String, CliError> {
    let mut terminal = ctx
        .terminal
        .lock()
        .map_err(|_| failure("terminal lock poisoned"))?;
    if !ctx.prompt_context().interactive(&**terminal) {
        return Err(CliError::input("missing address").with_help("pass the ss58, --vault to scan it from your phone, or --extension to pick it from your browser extension"));
    }
    loop {
        let address = terminal.read_line("SS58 address: ").map_err(prompt_error)?;
        let address = python_strip(&address);
        if is_bittensor_address(address) {
            return Ok(address.to_owned());
        }
        terminal
            .message("Enter a valid Bittensor SS58 address.")
            .map_err(prompt_error)?;
    }
}

fn detail(ctx: &AppContext, title: &str, value: &Value) -> Result<(), CliError> {
    let fields = value
        .as_object()
        .map(|object| {
            object
                .iter()
                .map(|(key, value)| (key.clone(), value.clone()))
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    ctx.output
        .detail(Some(title), &fields, Some(value))
        .map_err(failure)
}
