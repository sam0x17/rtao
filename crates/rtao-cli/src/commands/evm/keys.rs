// Derived from RaoFoundation/subtensor sdk/python/bittensor/cli/commands/evm/keys.py @ 850f2755bb05a3851f948489f0ab27b0645cebce (Apache-2.0); modified by rtao.
// Derived from RaoFoundation/subtensor sdk/python/bittensor/cli/commands/evm/_shared.py @ 850f2755bb05a3851f948489f0ab27b0645cebce (Apache-2.0); modified by rtao.
// Derived from RaoFoundation/subtensor sdk/python/bittensor/evm/keys.py @ 850f2755bb05a3851f948489f0ab27b0645cebce (Apache-2.0); modified by rtao.
//! Encrypted EVM key creation, import, public inspection and explicit export.

use super::{failure, key_ref};
use crate::{
    context::AppContext,
    error::CliError,
    globals::{Tier, with_globals},
    output::TableSpec,
    prompt::{PromptContext, PromptError, SharedTerminal, python_strip},
    secrets::argv_secret_warning,
};
use clap::{Arg, ArgAction, ArgMatches, Command};
use rtao::evm::{
    ETH_DERIVATION_PATH, EvmAccount, EvmKeyInfo, EvmKeyOptions,
    keys::{self as evm_keys, ImportSource, PasswordResolver},
};
use secrecy::{ExposeSecret, SecretString};
use serde_json::{Value, json};
use std::{
    io::IsTerminal,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};
use zeroize::Zeroizing;

const MAX_PASSWORD_BYTES: usize = 1024 * 1024;

fn option(id: &'static str, long: &'static str, help: &'static str) -> Arg {
    Arg::new(id).long(long).help(help)
}
fn flag(id: &'static str, long: &'static str, help: &'static str) -> Arg {
    option(id, long, help).action(ArgAction::SetTrue)
}
fn storage(command: Command) -> Command {
    with_globals(
        command
            .arg(option("name", "name", "Stored key name.").default_value("default"))
            .arg(flag(
                "overwrite",
                "overwrite",
                "Replace an existing EVM key.",
            )),
        Tier::Unlock,
    )
}
fn key_argument() -> Arg {
    Arg::new("key").help("Stored key NAME or WALLET/NAME; defaults to the wallet's default key.")
}

/// Build Python's five encrypted EVM key commands.
pub fn command() -> Command {
    Command::new("key")
        .about("Manage encrypted Ethereum V3 keys beside the native wallet's hotkeys.")
        .subcommand_required(true)
        .arg_required_else_help(true)
        .subcommands([
            storage(Command::new("new").about("Create a random secp256k1 key.")),
            storage(
                Command::new("import")
                    .about("Import a private key, encrypted keystore, or BIP-39 mnemonic.")
                    .arg(option(
                        "private_key",
                        "private-key",
                        "Raw private key; omit to enter without echo.",
                    ))
                    .arg(option(
                        "keystore",
                        "keystore",
                        "Path to an Ethereum V3 keystore JSON file.",
                    ))
                    .arg(option(
                        "keystore_password",
                        "keystore-password",
                        "Imported keystore password; defaults to the storage password.",
                    ))
                    .arg(option(
                        "mnemonic",
                        "mnemonic",
                        "BIP-39 mnemonic; avoid passing secrets in process arguments.",
                    ))
                    .arg(
                        option(
                            "derivation_path",
                            "derivation-path",
                            "Ethereum BIP-44 derivation path.",
                        )
                        .default_value(ETH_DERIVATION_PATH),
                    ),
            ),
            with_globals(
                Command::new("list").about("List public EVM key metadata."),
                Tier::Read,
            ),
            with_globals(
                Command::new("show")
                    .about("Show an EVM key's address, mirror, and path.")
                    .arg(key_argument()),
                Tier::Read,
            ),
            with_globals(
                Command::new("export")
                    .about("Export an encrypted keystore, or explicitly reveal its private key.")
                    .arg(key_argument())
                    .arg(option(
                        "out",
                        "out",
                        "Write encrypted JSON here instead of stdout.",
                    ))
                    .arg(flag(
                        "private_key",
                        "private-key",
                        "Decrypt the raw key; terminal output defaults to the clipboard.",
                    ))
                    .arg(flag(
                        "show",
                        "show",
                        "With --private-key, print instead of copying to the clipboard.",
                    )),
                Tier::Read,
            ),
        ])
}

/// Reserve command-owned fields from global leaf overrides.
pub fn reserved(group: &ArgMatches) -> Result<Vec<&'static str>, CliError> {
    let (name, _) = group
        .subcommand()
        .ok_or_else(|| CliError::input("missing EVM key command"))?;
    match name {
        "new" => Ok(vec!["name", "overwrite"]),
        "import" => Ok(vec![
            "name",
            "overwrite",
            "private_key",
            "keystore",
            "keystore_password",
            "mnemonic",
            "derivation_path",
        ]),
        "list" => Ok(vec![]),
        "show" => Ok(vec!["key"]),
        "export" => Ok(vec!["key", "out", "private_key", "show"]),
        _ => Err(CliError::input("unknown EVM key command")),
    }
}

fn text<'a>(matches: &'a ArgMatches, id: &str) -> Option<&'a str> {
    matches
        .try_get_one::<String>(id)
        .ok()
        .flatten()
        .map(String::as_str)
}
fn enabled(matches: &ArgMatches, id: &str) -> bool {
    matches
        .try_get_one::<bool>(id)
        .ok()
        .flatten()
        .copied()
        .unwrap_or(false)
}
fn prompt_error(error: PromptError) -> CliError {
    CliError {
        code: error.exit_code(),
        message: error.to_string(),
        help: None,
    }
}
fn expand_path(path: &Path) -> Result<PathBuf, CliError> {
    let Some(text) = path.to_str() else {
        return Ok(path.to_owned());
    };
    if text == "~" || text.starts_with("~/") {
        #[allow(deprecated)]
        let home = std::env::home_dir().ok_or_else(|| failure("home directory is unavailable"))?;
        Ok(home.join(text.strip_prefix("~/").unwrap_or("")))
    } else {
        Ok(path.to_owned())
    }
}
fn read_secret_text(path: &Path, password_file: bool) -> Result<Zeroizing<String>, CliError> {
    let bytes = rtao::wallet::read_keyfile_data(&expand_path(path)?).map_err(|_| {
        failure(if password_file {
            "password file must be a readable regular file no larger than 16 MiB"
        } else {
            "keystore must be a readable regular file no larger than 16 MiB"
        })
    })?;
    let text = std::str::from_utf8(&bytes)
        .map_err(|_| failure("key/password file must contain valid UTF-8"))?;
    let text = if password_file {
        python_strip(text)
    } else {
        text
    };
    if password_file && text.len() > MAX_PASSWORD_BYTES {
        return Err(failure("EVM password exceeds 1 MiB limit"));
    }
    Ok(Zeroizing::new(text.to_owned()))
}

// Keep CLI's file/Keychain precedence ahead of the library's environment and
// prompt fallbacks. In particular, native BT_PW_* coldkey variables do not
// unlock keys in this separate signing domain.
async fn explicit_password(ctx: &AppContext) -> Result<Option<Zeroizing<String>>, CliError> {
    if let Some(password) = ctx
        .passwords
        .password
        .as_ref()
        .filter(|value| !value.expose_secret().is_empty())
    {
        return checked_password(password.expose_secret()).map(Some);
    }
    if let Some(path) = ctx
        .globals
        .wallet_password_file
        .as_ref()
        .or(ctx.passwords.password_file.as_ref())
    {
        let path = path.clone();
        return rtao::core::offload(move || read_secret_text(&path, true))
            .await
            .map_err(failure)?
            .map(Some);
    }
    if ctx.globals.keychain_password || ctx.passwords.keychain {
        let provider = ctx
            .passwords
            .native_provider
            .as_ref()
            .ok_or_else(|| failure("native password provider is unavailable"))?;
        if let Some(password) = provider
            .keychain_load(&ctx.globals.wallet_name)
            .await
            .map_err(failure)?
            && !password.expose_secret().is_empty()
        {
            return checked_password(password.expose_secret()).map(Some);
        }
    }
    Ok(None)
}
fn checked_password(value: &str) -> Result<Zeroizing<String>, CliError> {
    if value.len() > MAX_PASSWORD_BYTES {
        return Err(failure("EVM password exceeds 1 MiB limit"));
    }
    Ok(Zeroizing::new(value.into()))
}

struct KeyPrompt {
    terminal: SharedTerminal,
    context: PromptContext,
    error: Mutex<Option<CliError>>,
}
impl KeyPrompt {
    fn new(ctx: &AppContext) -> Self {
        Self {
            terminal: ctx.terminal.clone(),
            context: ctx.prompt_context(),
            error: Mutex::new(None),
        }
    }
    fn prompt(&self, confirm: bool) -> Result<Zeroizing<String>, CliError> {
        let mut terminal = self
            .terminal
            .lock()
            .map_err(|_| failure("terminal lock poisoned"))?;
        if !self.context.password_interactive(&**terminal) {
            return Err(failure(
                "no password available: pass one explicitly or set BT_WALLET_PASSWORD",
            ));
        }
        let password = terminal
            .read_secret("EVM key password: ")
            .map_err(prompt_error)?;
        if password.expose_secret().is_empty() {
            return Err(failure("empty password"));
        }
        let password = checked_password(password.expose_secret())?;
        if confirm {
            let again = terminal
                .read_secret("Retype password: ")
                .map_err(prompt_error)?;
            if again.expose_secret() != password.as_str() {
                return Err(failure("passwords do not match"));
            }
        }
        Ok(password)
    }
    fn finish<T>(&self, result: Result<T, CliError>) -> Result<T, CliError> {
        if result.is_err()
            && let Some(error) = self
                .error
                .lock()
                .map_err(|_| failure("password prompt lock poisoned"))?
                .take()
        {
            return Err(error);
        }
        result
    }
}
impl PasswordResolver for KeyPrompt {
    fn resolve(&self, confirm: bool) -> rtao::evm::Result<Zeroizing<String>> {
        self.prompt(confirm).map_err(|error| {
            let public = error.to_string();
            if let Ok(mut saved) = self.error.lock() {
                *saved = Some(error);
            }
            rtao::evm::Error::Validation(public)
        })
    }
}

enum KeyImport {
    PrivateKey(Zeroizing<String>),
    Mnemonic {
        phrase: Zeroizing<String>,
        path: String,
    },
    Keystore {
        json: Zeroizing<String>,
        password: Option<Zeroizing<String>>,
    },
}
trait KeyBackend: Send + Sync {
    fn list(&self, wallet: &str, root: &Path) -> Result<Vec<EvmKeyInfo>, CliError>;
    fn info(&self, name: &str, wallet: &str, root: &Path) -> Result<EvmKeyInfo, CliError>;
    fn create(
        &self,
        options: &EvmKeyOptions,
        prompt: &dyn PasswordResolver,
    ) -> Result<EvmKeyInfo, CliError>;
    fn import(
        &self,
        source: KeyImport,
        options: &EvmKeyOptions,
        prompt: &dyn PasswordResolver,
    ) -> Result<EvmKeyInfo, CliError>;
    fn export(&self, name: &str, wallet: &str, root: &Path) -> Result<Value, CliError>;
    fn write(&self, path: &Path, document: &Value) -> Result<(), CliError>;
    fn unlock(
        &self,
        options: &EvmKeyOptions,
        prompt: &dyn PasswordResolver,
    ) -> Result<EvmAccount, CliError>;
}
struct NativeKeys;
impl KeyBackend for NativeKeys {
    fn list(&self, wallet: &str, root: &Path) -> Result<Vec<EvmKeyInfo>, CliError> {
        evm_keys::list_evm_keys(wallet, root).map_err(failure)
    }
    fn info(&self, name: &str, wallet: &str, root: &Path) -> Result<EvmKeyInfo, CliError> {
        evm_keys::get_evm_key_info(name, wallet, root).map_err(failure)
    }
    fn create(
        &self,
        options: &EvmKeyOptions,
        prompt: &dyn PasswordResolver,
    ) -> Result<EvmKeyInfo, CliError> {
        evm_keys::create_evm_key(options, Some(prompt)).map_err(failure)
    }
    fn import(
        &self,
        source: KeyImport,
        options: &EvmKeyOptions,
        prompt: &dyn PasswordResolver,
    ) -> Result<EvmKeyInfo, CliError> {
        let source = match &source {
            KeyImport::PrivateKey(value) => ImportSource::PrivateKey(value),
            KeyImport::Mnemonic { phrase, path } => ImportSource::Mnemonic {
                phrase,
                derivation_path: Some(path),
            },
            KeyImport::Keystore { json, password } => ImportSource::Keystore {
                json,
                password: password.as_deref().map(String::as_str),
            },
        };
        evm_keys::import_evm_key(source, options, Some(prompt)).map_err(failure)
    }
    fn export(&self, name: &str, wallet: &str, root: &Path) -> Result<Value, CliError> {
        evm_keys::export_evm_key(name, wallet, root).map_err(failure)
    }
    fn write(&self, path: &Path, document: &Value) -> Result<(), CliError> {
        evm_keys::write_keystore_file(path, document, true).map_err(failure)
    }
    fn unlock(
        &self,
        options: &EvmKeyOptions,
        prompt: &dyn PasswordResolver,
    ) -> Result<EvmAccount, CliError> {
        evm_keys::unlock_evm_key(options, Some(prompt)).map_err(failure)
    }
}

async fn options(
    ctx: &AppContext,
    wallet: String,
    name: String,
    overwrite: bool,
) -> Result<EvmKeyOptions, CliError> {
    // Validate names before reading a password file or consulting a provider.
    evm_keys::keyfile_path(&name, &wallet, &ctx.globals.wallet_path).map_err(failure)?;
    Ok(EvmKeyOptions {
        name,
        wallet_name: wallet,
        wallet_path: ctx.globals.wallet_path.clone(),
        password: explicit_password(ctx).await?,
        overwrite,
    })
}

/// Decrypt only after the caller has completed transaction review and confirmation.
/// Filesystem and key derivation execute on the shared blocking worker.
pub(super) async fn unlock_key(
    ctx: &AppContext,
    value: Option<&str>,
) -> Result<EvmAccount, CliError> {
    unlock_with(ctx, value, Arc::new(NativeKeys)).await
}
async fn unlock_with(
    ctx: &AppContext,
    value: Option<&str>,
    backend: Arc<dyn KeyBackend>,
) -> Result<EvmAccount, CliError> {
    let (wallet, name) = key_ref(ctx, value);
    let options = options(ctx, wallet, name, false).await?;
    let prompt = KeyPrompt::new(ctx);
    rtao::core::offload(move || prompt.finish(backend.unlock(&options, &prompt)))
        .await
        .map_err(failure)?
}

async fn import_source(ctx: &AppContext, leaf: &ArgMatches) -> Result<KeyImport, CliError> {
    let private = text(leaf, "private_key").filter(|value| !value.is_empty());
    let mnemonic = text(leaf, "mnemonic").filter(|value| !value.is_empty());
    let keystore = text(leaf, "keystore");
    let count = usize::from(private.is_some())
        + usize::from(mnemonic.is_some())
        + usize::from(keystore.is_some());
    if count > 1 {
        return Err(failure(
            "provide exactly one of: private_key, keystore_json, mnemonic",
        ));
    }
    if let Some(path) = keystore {
        let password = text(leaf, "keystore_password")
            .filter(|value| !value.is_empty())
            .map(checked_password)
            .transpose()?;
        let path = PathBuf::from(path);
        let json = rtao::core::offload(move || read_secret_text(&path, false))
            .await
            .map_err(failure)??;
        return Ok(KeyImport::Keystore { json, password });
    }
    if let Some(value) = private {
        return Ok(KeyImport::PrivateKey(Zeroizing::new(value.into())));
    }
    let path = text(leaf, "derivation_path")
        .unwrap_or(ETH_DERIVATION_PATH)
        .to_owned();
    if let Some(value) = mnemonic {
        return Ok(KeyImport::Mnemonic {
            phrase: Zeroizing::new(value.into()),
            path,
        });
    }
    let terminal = ctx.terminal.clone();
    let context = ctx.prompt_context();
    let answer = rtao::core::offload(move || {
        let mut terminal = terminal
            .lock()
            .map_err(|_| failure("terminal lock poisoned"))?;
        if !context.interactive(&**terminal) {
            return Err(CliError::input(
                "missing key source: `--private-key`, `--keystore`, or `--mnemonic`",
            )
            .with_help("pass one explicitly, or run on a terminal to be prompted"));
        }
        terminal
            .read_secret("EVM private key or mnemonic: ")
            .map_err(prompt_error)
    })
    .await
    .map_err(failure)??;
    let answer = python_strip(answer.expose_secret());
    if answer.len() > evm_keys::MAX_KEYSTORE_BYTES {
        return Err(CliError::input("EVM key source exceeds 16 MiB limit"));
    }
    let raw = answer.strip_prefix("0x").unwrap_or(answer);
    if raw.len() == 64 && raw.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        Ok(KeyImport::PrivateKey(Zeroizing::new(answer.into())))
    } else {
        Ok(KeyImport::Mnemonic {
            phrase: Zeroizing::new(answer.into()),
            path,
        })
    }
}

fn display_info(ctx: &AppContext, title: &str, info: &EvmKeyInfo) -> Result<(), CliError> {
    ctx.output.detail(Some(title), &[
        ("name".into(), json!(info.name)), ("address".into(), json!(info.address)),
        ("ss58 mirror".into(), json!(info.ss58_mirror)), ("keystore".into(), json!(info.path)),
    ], Some(&json!({"name": info.name, "address": info.address, "ss58_mirror": info.ss58_mirror, "keystore": info.path}))).map_err(failure)
}

fn private_key_hex(account: &EvmAccount) -> Result<Zeroizing<String>, CliError> {
    let key = Zeroizing::new(account.to_bytes().0);
    let mut encoded = Zeroizing::new([0_u8; 66]);
    encoded[..2].copy_from_slice(b"0x");
    hex::encode_to_slice(key.as_slice(), &mut encoded[2..])
        .map_err(|_| failure("could not encode private key"))?;
    std::str::from_utf8(encoded.as_slice())
        .map(|value| Zeroizing::new(value.into()))
        .map_err(|_| failure("could not encode private key"))
}

async fn display_private_key(
    ctx: &AppContext,
    info: &EvmKeyInfo,
    raw: Zeroizing<String>,
    show: bool,
) -> Result<(), CliError> {
    let mut runner = crate::process::SystemProcessRunner::default();
    display_private_key_with(
        ctx,
        info,
        raw,
        show,
        std::io::stdout().is_terminal(),
        &mut runner,
    )
    .await
}
async fn display_private_key_with(
    ctx: &AppContext,
    info: &EvmKeyInfo,
    raw: Zeroizing<String>,
    show: bool,
    stdout_is_tty: bool,
    runner: &mut dyn crate::process::ProcessRunner,
) -> Result<(), CliError> {
    if !show && !ctx.output.options().json_mode && stdout_is_tty {
        let secret = SecretString::new(raw.to_string());
        if crate::secrets::copy_to_clipboard(runner, &secret).await {
            return ctx
                .output
                .message(&format!(
                    "private key for {} ({}) copied to clipboard (not printed)",
                    info.name, info.address
                ))
                .map_err(failure);
        }
        ctx.output.message("warning: no clipboard tool found (pbcopy, wl-copy, xclip, or xsel), printing instead").map_err(failure)?;
    }
    ctx.output.message(&format!("raw private key for {} ({}) controls the account; it never expires and cannot be revoked", info.name, info.address)).map_err(failure)?;
    // The explicit raw export is a scalar in Python JSON, not a detail object.
    // Its fixed ASCII alphabet lets borrowed fragments avoid owned JSON copies.
    if ctx.output.options().json_mode {
        ctx.output.secret_line(&["\"", &raw, "\""]).map_err(failure)
    } else {
        ctx.output.secret_line(&[&raw]).map_err(failure)
    }
}

/// Run the selected key command without opening any chain connection.
pub async fn run(ctx: &AppContext, group: &ArgMatches) -> Result<(), CliError> {
    run_with_backend(ctx, group, Arc::new(NativeKeys)).await
}
async fn run_with_backend(
    ctx: &AppContext,
    group: &ArgMatches,
    backend: Arc<dyn KeyBackend>,
) -> Result<(), CliError> {
    let (name, leaf) = group
        .subcommand()
        .ok_or_else(|| CliError::input("missing EVM key command"))?;
    match name {
        "list" => {
            let wallet = ctx.globals.wallet_name.clone();
            let root = ctx.globals.wallet_path.clone();
            let records = rtao::core::offload(move || backend.list(&wallet, &root))
                .await
                .map_err(failure)??;
            let mut table = TableSpec::new(
                format!("EVM keys in wallet {}", ctx.globals.wallet_name),
                ["name", "address", "ss58 mirror"]
                    .map(String::from)
                    .to_vec(),
                records
                    .iter()
                    .map(|info| {
                        vec![
                            json!(info.name),
                            json!(info.address),
                            json!(info.ss58_mirror),
                        ]
                    })
                    .collect(),
            );
            table.records = Some(Value::Array(records.iter().map(|info| json!({"name":info.name,"address":info.address,"ss58_mirror":info.ss58_mirror})).collect()));
            ctx.output.table(&table).map_err(failure)
        }
        "show" | "export" => {
            if name == "export"
                && enabled(leaf, "private_key")
                && text(leaf, "out").is_some_and(|path| !path.is_empty())
            {
                return Err(CliError::input("--private-key prints to stdout only")
                    .with_help("a raw key on disk defeats the keystore; drop --out"));
            }
            let (wallet, key) = key_ref(ctx, text(leaf, "key"));
            let root = ctx.globals.wallet_path.clone();
            let info_backend = backend.clone();
            let info_wallet = wallet.clone();
            let info_key = key.clone();
            let info =
                rtao::core::offload(move || info_backend.info(&info_key, &info_wallet, &root))
                    .await
                    .map_err(failure)??;
            if name == "show" {
                return display_info(ctx, "EVM key", &info);
            }
            if enabled(leaf, "private_key") {
                let account = unlock_with(ctx, text(leaf, "key"), backend).await?;
                let raw = private_key_hex(&account)?;
                return display_private_key(ctx, &info, raw, enabled(leaf, "show")).await;
            }
            let root = ctx.globals.wallet_path.clone();
            let out = text(leaf, "out")
                .filter(|path| !path.is_empty())
                .map(str::to_owned);
            let destination = out.as_deref().map(Path::new).map(expand_path).transpose()?;
            let document = rtao::core::offload(move || {
                let document = backend.export(&key, &wallet, &root)?;
                if let Some(path) = destination {
                    backend.write(&path, &document)?;
                }
                Ok::<_, CliError>(document)
            })
            .await
            .map_err(failure)??;
            if let Some(out) = out {
                ctx.output
                    .detail(
                        Some("exported EVM key"),
                        &[
                            ("name".into(), json!(info.name)),
                            ("address".into(), json!(info.address)),
                            ("file".into(), json!(out)),
                        ],
                        None,
                    )
                    .map_err(failure)
            } else {
                ctx.output.value(&document).map_err(failure)
            }
        }
        "new" | "import" => {
            let source = if name == "import" {
                if let Some(warning) = argv_secret_warning(&[
                    (
                        "--private-key",
                        text(leaf, "private_key").is_some_and(|value| !value.is_empty()),
                    ),
                    (
                        "--mnemonic",
                        text(leaf, "mnemonic").is_some_and(|value| !value.is_empty()),
                    ),
                    (
                        "--keystore-password",
                        text(leaf, "keystore_password").is_some_and(|value| !value.is_empty()),
                    ),
                ]) {
                    ctx.output.message(&warning).map_err(failure)?;
                }
                Some(import_source(ctx, leaf).await?)
            } else {
                None
            };
            let options = options(
                ctx,
                ctx.globals.wallet_name.clone(),
                text(leaf, "name").unwrap_or("default").to_owned(),
                enabled(leaf, "overwrite"),
            )
            .await?;
            let prompt = KeyPrompt::new(ctx);
            let info = rtao::core::offload(move || {
                let result = match source {
                    Some(source) => backend.import(source, &options, &prompt),
                    None => backend.create(&options, &prompt),
                };
                prompt.finish(result)
            })
            .await
            .map_err(failure)??;
            display_info(
                ctx,
                if name == "new" {
                    "created EVM key"
                } else {
                    "imported EVM key"
                },
                &info,
            )?;
            if name == "new" {
                ctx.output.message("fund it with `rtao evm fund`; TAO sent to the ss58 mirror appears as this account's balance in any Ethereum tool").map_err(failure)?;
            }
            Ok(())
        }
        _ => Err(CliError::input("unknown EVM key command")),
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;
    use crate::{
        application::command_tree,
        books::Books,
        globals::Globals,
        output::{MemorySink, Output, OutputOptions, Stream},
        process::{ProcessCommand, ProcessFuture, ProcessOutput, ProcessRunner},
        prompt::Terminal,
    };
    use std::{collections::VecDeque, thread::ThreadId};

    struct Clipboard {
        success: bool,
        calls: Vec<ProcessCommand>,
    }
    impl ProcessRunner for Clipboard {
        fn which(&self, name: &std::ffi::OsStr) -> Option<PathBuf> {
            (name == "pbcopy").then(|| PathBuf::from("/fixture/pbcopy"))
        }
        fn run(&mut self, command: ProcessCommand) -> ProcessFuture<'_> {
            self.calls.push(command);
            let success = self.success;
            Box::pin(async move {
                Ok(ProcessOutput {
                    success,
                    code: Some(if success { 0 } else { 1 }),
                    stdout: Zeroizing::new(Vec::new()),
                    stderr: Zeroizing::new(Vec::new()),
                })
            })
        }
    }

    const RAW: &str = "0x0000000000000000000000000000000000000000000000000000000000000001";
    const PRIVATE_MARKER: &str = "private-argv-marker";

    #[derive(Default)]
    struct TerminalState {
        tty: bool,
        replies: VecDeque<Result<SecretString, PromptError>>,
        secret_labels: Vec<String>,
    }
    struct FakeTerminal(Arc<Mutex<TerminalState>>);
    impl Terminal for FakeTerminal {
        fn stdin_is_tty(&self) -> bool {
            self.0.lock().unwrap().tty
        }
        fn stderr_is_tty(&self) -> bool {
            self.0.lock().unwrap().tty
        }
        fn read_line(&mut self, _: &str) -> Result<String, PromptError> {
            Err(PromptError::Unavailable)
        }
        fn read_secret(&mut self, label: &str) -> Result<SecretString, PromptError> {
            let mut state = self.0.lock().unwrap();
            state.secret_labels.push(label.into());
            state
                .replies
                .pop_front()
                .unwrap_or(Err(PromptError::Aborted))
        }
        fn message(&mut self, _: &str) -> Result<(), PromptError> {
            Ok(())
        }
    }
    fn fixture(
        args: &[&str],
        tty: bool,
        replies: &[&str],
    ) -> (
        AppContext,
        ArgMatches,
        MemorySink,
        Arc<Mutex<TerminalState>>,
    ) {
        let matches = command_tree([command()])
            .try_get_matches_from(["rtao", "key"].into_iter().chain(args.iter().copied()))
            .unwrap();
        let group = matches.subcommand().unwrap().1.clone();
        let mut globals = Globals::resolve(
            &matches,
            &Default::default(),
            Path::new("/fixture/home"),
            |_| None,
        )
        .unwrap();
        globals
            .apply(group.subcommand().unwrap().1, &reserved(&group).unwrap())
            .unwrap();
        let terminal = Arc::new(Mutex::new(TerminalState {
            tty,
            replies: replies
                .iter()
                .map(|value| Ok(SecretString::new((*value).into())))
                .collect(),
            ..Default::default()
        }));
        let mut ctx = AppContext::new(
            globals,
            Default::default(),
            Books::default(),
            Arc::new(Mutex::new(Box::new(FakeTerminal(terminal.clone())))),
        );
        // No environment, process, filesystem, or password provider is needed
        // by this fake. Native key derivation is covered by the EVM crate.
        ctx.passwords.password = Some(SecretString::new("fixture-password".into()));
        let sink = MemorySink::default();
        ctx.output = Output::new(
            OutputOptions {
                json_mode: ctx.globals.json,
                ..Default::default()
            },
            Arc::new(sink.clone()),
        );
        (ctx, group, sink, terminal)
    }

    struct FakeKeys {
        owner: ThreadId,
        calls: Mutex<Vec<String>>,
    }
    impl FakeKeys {
        fn new() -> Arc<Self> {
            Arc::new(Self {
                owner: std::thread::current().id(),
                calls: Mutex::new(Vec::new()),
            })
        }
        fn record(&self, call: String) {
            assert_ne!(
                std::thread::current().id(),
                self.owner,
                "filesystem/KDF operation ran on the current-thread executor"
            );
            self.calls.lock().unwrap().push(call);
        }
        fn info_for(name: &str) -> EvmKeyInfo {
            EvmKeyInfo {
                name: name.into(),
                address: "0x7e5f4552091a69125d5dfcb7b8c2659029395bdf".into(),
                ss58_mirror: "fixture-mirror".into(),
                path: PathBuf::from("/fixture/wallet/evmkeys").join(name),
            }
        }
        fn calls(&self) -> Vec<String> {
            self.calls.lock().unwrap().clone()
        }
    }
    impl KeyBackend for FakeKeys {
        fn list(&self, wallet: &str, _: &Path) -> Result<Vec<EvmKeyInfo>, CliError> {
            self.record(format!("list:{wallet}"));
            Ok(vec![Self::info_for("default")])
        }
        fn info(&self, name: &str, wallet: &str, _: &Path) -> Result<EvmKeyInfo, CliError> {
            self.record(format!("info:{wallet}/{name}"));
            Ok(Self::info_for(name))
        }
        fn create(
            &self,
            options: &EvmKeyOptions,
            _: &dyn PasswordResolver,
        ) -> Result<EvmKeyInfo, CliError> {
            self.record(format!(
                "create:{}/{}:{}",
                options.wallet_name, options.name, options.overwrite
            ));
            Ok(Self::info_for(&options.name))
        }
        fn import(
            &self,
            source: KeyImport,
            options: &EvmKeyOptions,
            _: &dyn PasswordResolver,
        ) -> Result<EvmKeyInfo, CliError> {
            let kind = match source {
                KeyImport::PrivateKey(_) => "private",
                KeyImport::Mnemonic { .. } => "mnemonic",
                KeyImport::Keystore { .. } => "keystore",
            };
            self.record(format!(
                "import:{kind}:{}/{}",
                options.wallet_name, options.name
            ));
            Ok(Self::info_for(&options.name))
        }
        fn export(&self, name: &str, wallet: &str, _: &Path) -> Result<Value, CliError> {
            self.record(format!("export:{wallet}/{name}"));
            Ok(json!({"version":3,"crypto":{"ciphertext":"encrypted"},"extra":"retained"}))
        }
        fn write(&self, path: &Path, _: &Value) -> Result<(), CliError> {
            self.record(format!("write:{}", path.display()));
            Ok(())
        }
        fn unlock(
            &self,
            options: &EvmKeyOptions,
            _: &dyn PasswordResolver,
        ) -> Result<EvmAccount, CliError> {
            self.record(format!("unlock:{}/{}", options.wallet_name, options.name));
            RAW.parse().map_err(failure)
        }
    }

    #[test]
    fn key_surface_defaults_and_reserved_fields_match_python() {
        let tree = command();
        let names = tree
            .get_subcommands()
            .map(|command| command.get_name())
            .collect::<Vec<_>>();
        assert_eq!(names, ["new", "import", "list", "show", "export"]);
        let (_, group, _, _) = fixture(&["new"], false, &[]);
        assert_eq!(text(group.subcommand().unwrap().1, "name"), Some("default"));
        assert_eq!(reserved(&group).unwrap(), ["name", "overwrite"]);
        let (_, group, _, _) = fixture(&["import", "--private-key", ""], false, &[]);
        assert_eq!(
            text(group.subcommand().unwrap().1, "derivation_path"),
            Some(ETH_DERIVATION_PATH)
        );
        assert!(reserved(&group).unwrap().contains(&"private_key"));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn list_and_show_never_resolve_password_or_unlock_and_use_python_json_fields() {
        for command in ["list", "show"] {
            let (mut ctx, group, sink, terminal) = fixture(&[command, "--json"], false, &[]);
            ctx.passwords.password = None;
            ctx.globals.wallet_password_file = Some("/must-not-read-public-password-file".into());
            let backend = FakeKeys::new();
            run_with_backend(&ctx, &group, backend.clone())
                .await
                .unwrap();
            let value: Value = serde_json::from_str(&sink.text(Stream::Stdout).unwrap()).unwrap();
            if command == "list" {
                assert_eq!(
                    value,
                    json!([{"name":"default","address":FakeKeys::info_for("default").address,"ss58_mirror":"fixture-mirror"}])
                );
                assert_eq!(backend.calls(), ["list:default"]);
            } else {
                assert_eq!(value["keystore"], "/fixture/wallet/evmkeys/default");
                assert!(value.get("ss58_mirror").is_some());
                assert!(value.get("ss58 mirror").is_none());
                assert_eq!(backend.calls(), ["info:default/default"]);
            }
            assert!(terminal.lock().unwrap().secret_labels.is_empty());
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn encrypted_export_preserves_metadata_and_never_resolves_password() {
        let (mut ctx, group, sink, _) = fixture(&["export", "other/trade", "--json"], false, &[]);
        ctx.passwords.password = None;
        ctx.globals.wallet_password_file = Some("/must-not-read-export-password-file".into());
        let backend = FakeKeys::new();
        run_with_backend(&ctx, &group, backend.clone())
            .await
            .unwrap();
        let value: Value = serde_json::from_str(&sink.text(Stream::Stdout).unwrap()).unwrap();
        assert_eq!(value["extra"], "retained");
        assert_eq!(backend.calls(), ["info:other/trade", "export:other/trade"]);
        assert!(!sink.text(Stream::Stdout).unwrap().contains(RAW));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn encrypted_export_file_uses_worker_and_reports_public_fields_only() {
        let (ctx, group, sink, _) = fixture(
            &["export", "--out", "/fixture/export.json", "--json"],
            false,
            &[],
        );
        let backend = FakeKeys::new();
        run_with_backend(&ctx, &group, backend.clone())
            .await
            .unwrap();
        assert_eq!(
            backend.calls(),
            [
                "info:default/default",
                "export:default/default",
                "write:/fixture/export.json"
            ]
        );
        let value: Value = serde_json::from_str(&sink.text(Stream::Stdout).unwrap()).unwrap();
        assert_eq!(value["file"], "/fixture/export.json");
        assert!(value.get("crypto").is_none());
    }

    #[tokio::test(flavor = "current_thread")]
    async fn raw_export_file_conflict_is_rejected_before_any_key_or_password_access() {
        let (ctx, group, sink, terminal) = fixture(
            &["export", "--private-key", "--out", "/fixture/raw"],
            false,
            &[],
        );
        let backend = FakeKeys::new();
        let error = run_with_backend(&ctx, &group, backend.clone())
            .await
            .unwrap_err();
        assert_eq!(error.code, 2);
        assert!(backend.calls().is_empty());
        assert!(terminal.lock().unwrap().secret_labels.is_empty());
        assert!(sink.text(Stream::Stdout).unwrap().is_empty());
    }

    #[tokio::test(flavor = "current_thread")]
    async fn raw_export_is_an_explicit_json_scalar_and_unlocks_only_the_selected_key() {
        let (ctx, group, sink, _) = fixture(
            &["export", "other/trade", "--private-key", "--json"],
            false,
            &[],
        );
        let backend = FakeKeys::new();
        run_with_backend(&ctx, &group, backend.clone())
            .await
            .unwrap();
        let value: Value = serde_json::from_str(&sink.text(Stream::Stdout).unwrap()).unwrap();
        assert_eq!(value, RAW);
        assert_eq!(backend.calls(), ["info:other/trade", "unlock:other/trade"]);
        assert!(!sink.text(Stream::Stderr).unwrap().contains(RAW));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn terminal_raw_export_copies_via_secret_stdin_and_prints_only_on_explicit_show_or_fallback()
     {
        for (success, show, expect_copy, expect_print) in [
            (true, false, true, false),
            (false, false, true, true),
            (true, true, false, true),
        ] {
            let (ctx, _, sink, _) = fixture(&["export", "--private-key"], true, &[]);
            let mut runner = Clipboard {
                success,
                calls: Vec::new(),
            };
            display_private_key_with(
                &ctx,
                &FakeKeys::info_for("default"),
                Zeroizing::new(RAW.into()),
                show,
                true,
                &mut runner,
            )
            .await
            .unwrap();
            assert_eq!(runner.calls.len(), usize::from(expect_copy));
            if let Some(command) = runner.calls.first() {
                assert_eq!(command.input.as_deref().unwrap().as_slice(), RAW.as_bytes());
                assert!(!format!("{command:?}").contains(RAW));
            }
            assert_eq!(
                sink.text(Stream::Stdout).unwrap().contains(RAW),
                expect_print
            );
            assert!(!sink.text(Stream::Stderr).unwrap().contains(RAW));
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn json_and_piped_raw_exports_bypass_clipboard() {
        for (json_mode, stdout_is_tty) in [(true, true), (false, false)] {
            let args = if json_mode {
                vec!["export", "--private-key", "--json"]
            } else {
                vec!["export", "--private-key"]
            };
            let (ctx, _, sink, _) = fixture(&args, false, &[]);
            let mut runner = Clipboard {
                success: true,
                calls: Vec::new(),
            };
            display_private_key_with(
                &ctx,
                &FakeKeys::info_for("default"),
                Zeroizing::new(RAW.into()),
                false,
                stdout_is_tty,
                &mut runner,
            )
            .await
            .unwrap();
            assert!(runner.calls.is_empty());
            let stdout = sink.text(Stream::Stdout).unwrap();
            if json_mode {
                assert_eq!(serde_json::from_str::<Value>(&stdout).unwrap(), RAW);
            } else {
                assert_eq!(stdout, format!("{RAW}\n"));
            }
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn creation_and_import_keep_secrets_out_of_ordinary_json_and_diagnostics() {
        for args in [
            vec!["new", "--name", "trade", "--overwrite", "--json"],
            vec![
                "import",
                "--name",
                "trade",
                "--private-key",
                PRIVATE_MARKER,
                "--json",
            ],
        ] {
            let (ctx, group, sink, _) = fixture(&args, false, &[]);
            let backend = FakeKeys::new();
            run_with_backend(&ctx, &group, backend.clone())
                .await
                .unwrap();
            let stdout = sink.text(Stream::Stdout).unwrap();
            let stderr = sink.text(Stream::Stderr).unwrap();
            assert!(!stdout.contains(PRIVATE_MARKER));
            assert!(!stderr.contains(PRIVATE_MARKER));
            assert!(!stdout.contains("fixture-password"));
            assert_eq!(
                serde_json::from_str::<Value>(&stdout).unwrap()["name"],
                "trade"
            );
            if args[0] == "new" {
                assert_eq!(backend.calls(), ["create:default/trade:true"]);
            } else {
                assert_eq!(backend.calls(), ["import:private:default/trade"]);
            }
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn missing_or_conflicting_import_sources_fail_before_backend_and_password_access() {
        for args in [
            vec!["import"],
            vec!["import", "--private-key", ""],
            vec![
                "import",
                "--private-key",
                PRIVATE_MARKER,
                "--mnemonic",
                "mnemonic-marker",
            ],
        ] {
            let (mut ctx, group, _, terminal) = fixture(&args, false, &[]);
            ctx.globals.wallet_password_file =
                Some("/must-not-read-invalid-source-password".into());
            ctx.passwords.password = None;
            let backend = FakeKeys::new();
            let error = run_with_backend(&ctx, &group, backend.clone())
                .await
                .unwrap_err();
            assert_eq!(error.code, if args.len() > 3 { 1 } else { 2 });
            assert!(!error.message.contains(PRIVATE_MARKER));
            assert!(backend.calls().is_empty());
            assert!(terminal.lock().unwrap().secret_labels.is_empty());
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn hidden_import_detects_hex_and_mnemonic_without_echoing_sources() {
        for (reply, expected) in [(RAW, "private"), ("recovery-phrase-marker", "mnemonic")] {
            let (ctx, group, sink, terminal) = fixture(&["import"], true, &[reply]);
            let backend = FakeKeys::new();
            run_with_backend(&ctx, &group, backend.clone())
                .await
                .unwrap();
            assert_eq!(
                backend.calls(),
                [format!("import:{expected}:default/default")]
            );
            assert_eq!(
                terminal.lock().unwrap().secret_labels,
                ["EVM private key or mnemonic: "]
            );
            assert!(!sink.text(Stream::Stdout).unwrap().contains(reply));
            assert!(!sink.text(Stream::Stderr).unwrap().contains(reply));
        }
    }

    #[test]
    fn password_prompt_preserves_whitespace_and_requires_confirmation_only_for_storage() {
        let (ctx, _, _, state) = fixture(&["new"], true, &["  password  ", "  password  "]);
        assert_eq!(&*KeyPrompt::new(&ctx).prompt(true).unwrap(), "  password  ");
        assert_eq!(
            state.lock().unwrap().secret_labels,
            ["EVM key password: ", "Retype password: "]
        );
        let (ctx, _, _, state) = fixture(&["export"], true, &["unlock-password"]);
        assert_eq!(
            &*KeyPrompt::new(&ctx).prompt(false).unwrap(),
            "unlock-password"
        );
        assert_eq!(state.lock().unwrap().secret_labels.len(), 1);
    }

    #[test]
    fn password_prompt_errors_do_not_echo_secrets_and_preserve_abort_status() {
        for (replies, expected) in [
            (vec![""], "empty password"),
            (vec!["one", "two"], "passwords do not match"),
        ] {
            let (ctx, _, _, _) = fixture(&["new"], true, &replies);
            assert_eq!(
                KeyPrompt::new(&ctx).prompt(true).unwrap_err().message,
                expected
            );
        }
        let (ctx, _, _, _) = fixture(&["new"], true, &[]);
        let prompt = KeyPrompt::new(&ctx);
        let error = prompt.resolve(true).map_err(failure);
        assert_eq!(prompt.finish(error).unwrap_err().code, 130);
        assert!(
            checked_password(&"x".repeat(MAX_PASSWORD_BYTES + 1))
                .unwrap_err()
                .message
                .contains("exceeds")
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn empty_export_destination_uses_stdout_and_nonempty_raw_destinations_still_refuse() {
        for private in [false, true] {
            let args = if private {
                vec!["export", "--private-key", "--out", "", "--json"]
            } else {
                vec!["export", "--out", "", "--json"]
            };
            let (ctx, group, sink, terminal) = fixture(&args, false, &[]);
            let backend = FakeKeys::new();
            run_with_backend(&ctx, &group, backend.clone())
                .await
                .unwrap();
            let value: Value = serde_json::from_str(&sink.text(Stream::Stdout).unwrap()).unwrap();
            if private {
                assert_eq!(value, RAW);
                assert_eq!(
                    backend.calls(),
                    ["info:default/default", "unlock:default/default"]
                );
            } else {
                assert_eq!(value["extra"], "retained");
                assert_eq!(
                    backend.calls(),
                    ["info:default/default", "export:default/default"]
                );
            }
            assert!(terminal.lock().unwrap().secret_labels.is_empty());
        }
        for destination in ["/fixture/raw", " "] {
            let (ctx, group, sink, terminal) = fixture(
                &["export", "--private-key", "--out", destination],
                false,
                &[],
            );
            let backend = FakeKeys::new();
            let error = run_with_backend(&ctx, &group, backend.clone())
                .await
                .unwrap_err();
            assert_eq!(error.code, 2);
            assert!(backend.calls().is_empty());
            assert!(terminal.lock().unwrap().secret_labels.is_empty());
            assert!(sink.text(Stream::Stdout).unwrap().is_empty());
        }
    }
}
