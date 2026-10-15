// Derived from RaoFoundation/subtensor sdk/python/bittensor/cli/commands/wallet.py @ 850f2755bb05a3851f948489f0ab27b0645cebce (Apache-2.0); modified by rtao.
// Derived from RaoFoundation/subtensor sdk/python/bittensor/wallets.py @ 850f2755bb05a3851f948489f0ab27b0645cebce (Apache-2.0); modified by rtao.
// Additional sources at the same commit/license: sdk/python/bittensor/wallets.py; sdk/python/bittensor/cli/helpers.py; sdk/python/bittensor/cli/multisig_helpers.py.
//! Explicit wallet key management and public account information.

use crate::{
    books::{FsAccountLookup, NameResolver},
    context::AppContext,
    error::CliError,
    globals::{Tier, with_globals},
    output::{Output, TableSpec},
    prompt::{PromptHistory, WalletConfirmation},
};
use clap::{Arg, ArgAction, ArgMatches, Command};
use rtao::{
    client::Client,
    primitives::{AccountId, CryptoType},
    signer::SignerRole,
    wallet::{Overwrite, RegenerateFrom, Wallet, WalletRef, WriteOptions},
};
use secrecy::{ExposeSecret, SecretString};
use serde_json::{Value, json};
use std::{cmp::Ordering, fs, path::Path, sync::Arc};
use zeroize::Zeroizing;

#[path = "wallet_m8/mod.rs"]
mod m8;

fn failure(error: impl std::fmt::Display) -> CliError {
    CliError::failure(error.to_string())
}
fn input(error: impl std::fmt::Display) -> CliError {
    CliError::input(error.to_string())
}

fn option(id: &'static str, long: &'static str, help: &'static str) -> Arg {
    Arg::new(id).long(long).help(help)
}
fn flag(id: &'static str, long: &'static str, help: &'static str) -> Arg {
    option(id, long, help).action(ArgAction::SetTrue)
}
fn key_options(mut command: Command, generation: bool, cold: bool) -> Command {
    command = command
        .arg(flag(
            "overwrite",
            "overwrite",
            "Replace existing key files; keep their recovery material first.",
        ))
        .arg(
            option(
                "crypto_type",
                "crypto-type",
                "Key scheme: ed25519 (0) or sr25519 (1).",
            )
            .default_value("sr25519"),
        );
    if generation {
        command = command.arg(
            option(
                "n_words",
                "n-words",
                "Mnemonic words: 12, 15, 18, 21, or 24.",
            )
            .value_parser(clap::value_parser!(usize))
            .default_value("12"),
        );
    }
    if cold {
        command = command.arg(flag(
            "no_password",
            "no-password",
            "Store the coldkey unencrypted.",
        ));
    }
    with_globals(command, Tier::Read)
}
fn recovery(mut command: Command, cold: bool) -> Command {
    for (id, long, help) in [
        (
            "mnemonic",
            "mnemonic",
            "Recovery phrase; omit to enter it without echo.",
        ),
        (
            "seed",
            "seed",
            "32-byte hex seed; command-line values are visible to other processes.",
        ),
        (
            "private_key",
            "private-key",
            "64-byte hex private key; this is different from a seed.",
        ),
    ] {
        command = command.arg(option(id, long, help));
    }
    if cold {
        command = command
            .arg(
                option(
                    "json_path",
                    "json-path",
                    "Polkadot-JS encrypted JSON keystore path.",
                )
                .short('j'),
            )
            .arg(option(
                "json_password",
                "json-password",
                "JSON import passphrase; omit to enter it without echo.",
            ));
    }
    key_options(command, false, cold)
}

/// Build wallet commands, including the rtao password-export helper.
pub fn command() -> Command {
    let mut create = key_options(
        Command::new("create").about("Create a coldkey and hotkey."),
        true,
        true,
    );
    create = create.arg(
        option(
            "hotkey_crypto_type",
            "hotkey-crypto-type",
            "Hotkey scheme: ed25519 or sr25519.",
        )
        .default_value("sr25519"),
    );
    let public = |name: &'static str| {
        key_options(
            Command::new(name).about("Write a public key companion without secrets."),
            false,
            false,
        )
        .arg(option("ss58", "ss58", "Bittensor ss58 address.").required(true))
        .arg(
            option(
                "public_key",
                "public-key",
                "Matching 32-byte hex public key.",
            )
            .required(true),
        )
    };
    Command::new("wallet")
        .about("Create and manage wallets.")
        .subcommand_required(true)
        .arg_required_else_help(true)
        .subcommands([
            create,
            key_options(
                Command::new("new-coldkey")
                    .about("Create a coldkey and print its recovery mnemonic."),
                true,
                true,
            ),
            key_options(
                Command::new("new-hotkey")
                    .about("Create an unencrypted hotkey and print its recovery mnemonic."),
                true,
                false,
            ),
            recovery(
                Command::new("regen-coldkey")
                    .about("Recover a coldkey from a phrase, seed, private key, or JSON export."),
                true,
            ),
            recovery(
                Command::new("regen-hotkey").about("Recover an unencrypted hotkey."),
                false,
            ),
            public("regen-coldkeypub"),
            public("regen-hotkeypub"),
            with_globals(
                Command::new("show").about("Show public keys and crypto schemes."),
                Tier::Read,
            ),
            with_globals(
                Command::new("list").about("List local wallets and shared account books."),
                Tier::Read,
            ),
            with_globals(
                Command::new("balance")
                    .about("Show free TAO and stake valued at spot prices.")
                    .arg(
                        Arg::new("address")
                            .help("Coldkey address, wallet, contact, or saved multisig name."),
                    )
                    .arg(
                        flag("all", "all", "Show every local wallet and saved multisig.")
                            .short('a'),
                    )
                    .arg(option(
                        "sort",
                        "sort",
                        "Sort all-wallet rows by name, free, alpha, beta, or total.",
                    ))
                    .arg(flag(
                        "empty",
                        "empty",
                        "Include empty wallets in human output; JSON always includes them.",
                    )),
                Tier::Read,
            ),
            with_globals(
                Command::new("inspect")
                    .about("Show balance, stake, delegation, identity, and root baskets.")
                    .arg(option(
                        "coldkey_ss58",
                        "coldkey",
                        "Coldkey address, wallet, contact, or saved multisig name.",
                    ))
                    .arg(flag(
                        "dust",
                        "dust",
                        "Include dust positions in human output; JSON always includes them.",
                    )),
                Tier::Read,
            ),
            with_globals(
                Command::new("save-password-env")
                    .about("Print the coldkey password's compatible shell export without changing this process.")
                    .after_help("The encoded value is reversible and contains the password. Store it as a secret. The wallet path must produce a valid shell variable name."),
                Tier::Read,
            ),
            Command::new("keychain")
                .about("Manage the wallet coldkey password in macOS Keychain.")
                .subcommand_required(true)
                .arg_required_else_help(true)
                .subcommands([
                    with_globals(
                        Command::new("save")
                            .about("Save a password entered through the native macOS dialog.")
                            .after_help("Anyone with access to your logged-in macOS session can use the stored password."),
                        Tier::Read,
                    ),
                    with_globals(
                        Command::new("show").about("Check whether a password is stored."),
                        Tier::Read,
                    ),
                    with_globals(
                        Command::new("delete").about("Remove the stored password."),
                        Tier::Read,
                    ),
                ]),
        ])
        .subcommands(m8::commands())
}

/// Run a selected wallet group. Public chain commands open one shared client.
/// Callers resolve global settings from the selected leaf before dispatch.
pub async fn run(
    ctx: &mut AppContext,
    group: &ArgMatches,
    history: &mut PromptHistory,
) -> Result<(), CliError> {
    let (name, leaf) = group
        .subcommand()
        .ok_or_else(|| input("missing wallet command"))?;
    if name == "history" {
        return m8::history::run(ctx, leaf, history).await;
    }
    if matches!(name, "balance" | "inspect") || m8::is_chain(name) {
        let runner = ctx.clone();
        return runner
            .run(|client| async move { run_with_client(ctx, &client, group, history).await })
            .await;
    }
    run_local(ctx, name, leaf, history).await
}

/// Inject a connected client for fake-transport tests and embedded commands.
/// Local commands keep using only the explicit wallet directory.
pub async fn run_with_client(
    ctx: &mut AppContext,
    client: &Client,
    group: &ArgMatches,
    history: &mut PromptHistory,
) -> Result<(), CliError> {
    let (name, leaf) = group
        .subcommand()
        .ok_or_else(|| input("missing wallet command"))?;
    match name {
        "balance" => chain::balance(ctx, client, leaf, history).await,
        "inspect" => chain::inspect(ctx, client, leaf, history).await,
        "overview" => chain::overview(ctx, client, leaf, history).await,
        "registrations" => chain::registrations(ctx, client, leaf, history).await,
        "get-identity" => m8::get_identity(ctx, client, leaf, history).await,
        "swap-check" => m8::swap_check(ctx, client, leaf, history).await,
        name if m8::transaction_op(name).is_some() => {
            m8::transaction(ctx, client, name, leaf, history).await
        }
        _ => run_local(ctx, name, leaf, history).await,
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
fn overwrite(matches: &ArgMatches) -> Overwrite {
    if enabled(matches, "overwrite") {
        Overwrite::Yes
    } else {
        Overwrite::No
    }
}
fn crypto(value: &str) -> Result<CryptoType, CliError> {
    let normalized = crate::prompt::python_strip(value).to_ascii_lowercase();
    let numeric = (!normalized.is_empty()
        && normalized.chars().all(|ch| {
            unicode_general_category::get_general_category(ch)
                == unicode_general_category::GeneralCategory::DecimalNumber
        }))
    .then(|| rtao::primitives::pyfmt::parse_integer(&normalized))
    .flatten()
    .map(|integer| integer.to_string());
    match (normalized.as_str(), numeric.as_deref()) {
        ("ed25519", _) | (_, Some("0")) => Ok(CryptoType::Ed25519),
        ("sr25519", _) | (_, Some("1")) => Ok(CryptoType::Sr25519),
        _ => Err(failure(format!(
            "unknown crypto type {}; expected one of: ed25519, sr25519",
            rtao::primitives::pyfmt::string_repr(value)
        ))),
    }
}
fn crypto_name(value: CryptoType) -> &'static str {
    match value {
        CryptoType::Ed25519 => "ed25519",
        CryptoType::Sr25519 => "sr25519",
    }
}
fn prompt_error(error: crate::prompt::PromptError) -> CliError {
    CliError {
        code: error.exit_code(),
        message: error.to_string(),
        help: None,
    }
}
fn secret_prompt(ctx: &AppContext, label: &str) -> Result<SecretString, CliError> {
    let mut terminal = ctx
        .terminal
        .lock()
        .map_err(|_| failure("terminal lock poisoned"))?;
    if !ctx.prompt_context().interactive(&**terminal) {
        return Err(input("missing recovery material")
            .with_help("pass the required source explicitly, or use an interactive terminal"));
    }
    terminal.read_secret(label).map_err(prompt_error)
}
fn checked_hex(value: &str, bytes: usize, private: bool) -> Result<Zeroizing<Vec<u8>>, CliError> {
    let encoded = value.strip_prefix("0x").unwrap_or(value);
    if encoded.len() != bytes * 2 || !encoded.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(input(if private {
            "private key must be 64 bytes of hex (128 hex characters, optional 0x prefix)"
        } else {
            "seed must be 32 bytes of hex (64 hex characters, optional 0x prefix); for a 64-byte keyfile privateKey use --private-key"
        }));
    }
    let mut decoded = Zeroizing::new(vec![0; bytes]);
    hex::decode_to_slice(encoded, &mut decoded).map_err(|_| input("invalid recovery key hex"))?;
    Ok(decoded)
}
fn recovery_secret(
    ctx: &AppContext,
    matches: &ArgMatches,
    hot: bool,
) -> Result<RegenerateFrom, CliError> {
    let provided = ["mnemonic", "seed", "private_key", "json_path"]
        .into_iter()
        .filter(|id| text(matches, id).is_some_and(|value| !value.is_empty()))
        .count();
    if provided > 1 {
        return Err(input(if hot {
            "pass only one of `--mnemonic`, `--seed`, or `--private-key`"
        } else {
            "pass only one of `--mnemonic`, `--seed`, `--private-key`, or `--json-path`"
        }));
    }
    if let Some(path) = text(matches, "json_path").filter(|path| !path.is_empty()) {
        let path = expand_path(path)?;
        let bytes = rtao::wallet::read_keyfile_data(&path).map_err(|_| {
            input("JSON keystore must be a readable regular file no larger than 16 MiB")
        })?;
        let json =
            std::str::from_utf8(&bytes).map_err(|_| input("JSON keystore must be valid UTF-8"))?;
        let password = match text(matches, "json_password").filter(|value| !value.is_empty()) {
            Some(value) => SecretString::new(value.into()),
            None => secret_prompt(ctx, "Enter the backup password for the JSON file: ")?,
        };
        if password.expose_secret().is_empty() {
            return Err(input("JSON keystore password cannot be empty"));
        }
        return Ok(RegenerateFrom::EncryptedJson {
            json: SecretString::new(json.to_owned()),
            passphrase: password,
        });
    }
    if let Some(value) = text(matches, "private_key") {
        checked_hex(value, 64, true)?;
        return Ok(RegenerateFrom::PrivateKey(SecretString::new(value.into())));
    }
    if let Some(value) = text(matches, "seed") {
        return checked_hex(value, 32, false).map(RegenerateFrom::Seed);
    }
    if let Some(value) = text(matches, "mnemonic") {
        return Ok(RegenerateFrom::Mnemonic(SecretString::new(value.into())));
    }
    let answer = secret_prompt(
        ctx,
        if hot {
            "Hotkey mnemonic, hex seed, or private key: "
        } else {
            "Coldkey mnemonic, hex seed, or private key: "
        },
    )?;
    let answer = crate::prompt::python_strip(answer.expose_secret());
    if checked_hex(answer, 64, true).is_ok() {
        Ok(RegenerateFrom::PrivateKey(SecretString::new(answer.into())))
    } else if let Ok(seed) = checked_hex(answer, 32, false) {
        Ok(RegenerateFrom::Seed(seed))
    } else {
        Ok(RegenerateFrom::Mnemonic(SecretString::new(answer.into())))
    }
}
fn expand_path(path: &str) -> Result<std::path::PathBuf, CliError> {
    if path == "~" || path.starts_with("~/") {
        #[allow(deprecated)]
        let home = std::env::home_dir().ok_or_else(|| input("home directory is unavailable"))?;
        Ok(home.join(path.strip_prefix("~/").unwrap_or("")))
    } else {
        Ok(path.into())
    }
}
fn writing_wallet(ctx: &AppContext) -> Result<Wallet, CliError> {
    let mut passwords = ctx.passwords.clone();
    passwords.prompt_callback = Some(Arc::new(crate::prompt::TerminalPasswordPrompt::new(
        ctx.terminal.clone(),
        ctx.prompt_context(),
        true,
    )));
    Wallet::new(
        &ctx.globals.wallet_name,
        &ctx.globals.hotkey_name,
        &ctx.globals.wallet_path,
    )
    .map(|wallet| wallet.with_password_sources(passwords))
    .map_err(failure)
}

async fn run_local(
    ctx: &mut AppContext,
    name: &str,
    matches: &ArgMatches,
    history: &mut PromptHistory,
) -> Result<(), CliError> {
    if m8::is_local(name) {
        return m8::run_local(ctx, name, matches, history).await;
    }
    if name == "list" {
        return list(ctx).await;
    }
    if name == "show" {
        return show(ctx, history);
    }
    if name == "save-password-env" {
        return save_password_env(ctx).await;
    }
    if name == "keychain" {
        return run_keychain_with(
            ctx,
            matches,
            history,
            crate::process::SystemProcessRunner::default(),
            cfg!(target_os = "macos"),
        )
        .await;
    }
    let hot = matches!(name, "new-hotkey" | "regen-hotkey" | "regen-hotkeypub");
    let both = name == "create";
    let recover = matches!(name, "regen-coldkey" | "regen-hotkey");
    let public = matches!(name, "regen-coldkeypub" | "regen-hotkeypub");
    if !matches!(
        name,
        "create"
            | "new-coldkey"
            | "new-hotkey"
            | "regen-coldkey"
            | "regen-hotkey"
            | "regen-coldkeypub"
            | "regen-hotkeypub"
    ) {
        return Err(input(format!("unknown wallet command {name:?}")));
    }
    if recover {
        let warning = crate::secrets::argv_secret_warning(&[
            (
                "--mnemonic",
                text(matches, "mnemonic").is_some_and(|value| !value.is_empty()),
            ),
            (
                "--seed",
                text(matches, "seed").is_some_and(|value| !value.is_empty()),
            ),
            (
                "--private-key",
                text(matches, "private_key").is_some_and(|value| !value.is_empty()),
            ),
            (
                "--json-password",
                text(matches, "json_password").is_some_and(|value| !value.is_empty()),
            ),
        ]);
        if let Some(warning) = warning {
            ctx.output.message(&warning).map_err(failure)?;
        }
    }
    let source = if recover {
        Some(recovery_secret(ctx, matches, hot)?)
    } else {
        None
    };
    ctx.confirm_wallet(
        &WalletConfirmation {
            help: format!("Wallet to {name} in."),
            must_exist: false,
            require_coldkey: false,
            allow_multisig: false,
            hotkey_help: (hot || both).then(|| "Name for the hotkey.".into()),
            hotkey_must_exist: false,
            ..Default::default()
        },
        history,
    )?;
    let scheme = crypto(text(matches, "crypto_type").unwrap_or("sr25519"))?;
    // Reject both schemes before `create` can install its first keyfile.
    let hot_scheme = if both {
        crypto(text(matches, "hotkey_crypto_type").unwrap_or("sr25519"))?
    } else {
        scheme
    };
    let wallet = writing_wallet(ctx)?;
    let path = ctx.globals.wallet_path.to_string_lossy().into_owned();
    if both && overwrite(matches) == Overwrite::No {
        for path in [
            wallet.reference().coldkey_path(),
            wallet.reference().coldkeypub_path(),
            wallet.reference().hotkey_path(),
            wallet.reference().hotkeypub_path(),
        ] {
            if fs::symlink_metadata(&path).is_ok() {
                return Err(failure(format!(
                    "refusing to overwrite existing keyfile {path:?}"
                )));
            }
        }
    }
    if public {
        let ss58 =
            text(matches, "ss58").ok_or_else(|| input("missing required option: `--ss58`"))?;
        let account: AccountId = ss58
            .parse()
            .map_err(|_| input("invalid Bittensor ss58 address"))?;
        let key = text(matches, "public_key")
            .ok_or_else(|| input("missing required option: `--public-key`"))?;
        let key =
            checked_hex(key, 32, false).map_err(|_| input("public key must be 32 bytes of hex"))?;
        if account.0.as_slice() != key.as_slice() {
            return Err(input("public key does not match the ss58 address"));
        }
        if hot {
            wallet
                .regenerate_hotkeypub(account, scheme, overwrite(matches))
                .await
                .map_err(failure)?;
        } else {
            wallet
                .regenerate_coldkeypub(account, scheme, overwrite(matches))
                .await
                .map_err(failure)?;
        }
        return ctx
            .output
            .detail(
                Some(if hot {
                    "regenerated hotkeypub"
                } else {
                    "regenerated coldkeypub"
                }),
                &[
                    ("ss58".into(), json!(ss58)),
                    ("crypto_type".into(), json!(crypto_name(scheme))),
                ],
                None,
            )
            .map_err(failure);
    }
    let options = WriteOptions {
        encrypt: !hot && !enabled(matches, "no_password"),
        password: None,
        overwrite: overwrite(matches),
    };
    if let Some(source) = source {
        let account = if hot {
            wallet.regenerate_hotkey(source, scheme, options).await
        } else {
            wallet.regenerate_coldkey(source, scheme, options).await
        }
        .map_err(failure)?;
        let reported = wallet
            .public_info(if hot {
                SignerRole::Hotkey
            } else {
                SignerRole::Coldkey
            })
            .map_err(failure)?
            .crypto_type;
        let mut fields = vec![
            ("coldkey".into(), json!(ctx.globals.wallet_name)),
            ("crypto_type".into(), json!(crypto_name(reported))),
            ("ss58".into(), json!(account)),
            ("path".into(), json!(path)),
        ];
        if hot {
            fields.insert(1, ("hotkey".into(), json!(ctx.globals.hotkey_name)));
        }
        return ctx
            .output
            .detail(
                Some(if hot {
                    "regenerated hotkey"
                } else {
                    "regenerated coldkey"
                }),
                &fields,
                None,
            )
            .map_err(failure);
    }
    let words = matches.get_one::<usize>("n_words").copied().unwrap_or(12);
    if ![12, 15, 18, 21, 24].contains(&words) {
        return Err(failure("mnemonic word count must be 12, 15, 18, 21, or 24"));
    }
    let generated = if hot {
        wallet.create_new_hotkey(words, scheme, options).await
    } else {
        wallet.create_new_coldkey(words, scheme, options).await
    }
    .map_err(failure)?;
    ctx.output
        .mnemonic(
            if hot { "hotkey" } else { "coldkey" },
            generated.mnemonic.expose_secret(),
        )
        .map_err(failure)?;
    if both {
        let created_hot = wallet
            .create_new_hotkey(
                words,
                hot_scheme,
                WriteOptions {
                    overwrite: overwrite(matches),
                    ..WriteOptions::hotkey()
                },
            )
            .await
            .map_err(failure)?;
        ctx.output
            .mnemonic("hotkey", created_hot.mnemonic.expose_secret())
            .map_err(failure)?;
        let fields = vec![
            ("coldkey".into(), json!(ctx.globals.wallet_name)),
            ("hotkey".into(), json!(ctx.globals.hotkey_name)),
            ("coldkey_crypto_type".into(), json!(crypto_name(scheme))),
            ("hotkey_crypto_type".into(), json!(crypto_name(hot_scheme))),
            ("coldkey_ss58".into(), json!(generated.account)),
            ("path".into(), json!(path)),
        ];
        ctx.output
            .secret_detail(
                Some("created wallet"),
                &fields,
                &[
                    ("coldkey_mnemonic", generated.mnemonic.expose_secret()),
                    ("hotkey_mnemonic", created_hot.mnemonic.expose_secret()),
                ],
            )
            .map_err(failure)?;
    } else {
        let mut fields = vec![
            ("wallet".into(), json!(ctx.globals.wallet_name)),
            ("crypto_type".into(), json!(crypto_name(scheme))),
            ("ss58".into(), json!(generated.account)),
        ];
        if hot {
            fields.insert(1, ("hotkey".into(), json!(ctx.globals.hotkey_name)));
        }
        ctx.output
            .secret_detail(
                Some(if hot {
                    "created hotkey"
                } else {
                    "created coldkey"
                }),
                &fields,
                &[("mnemonic", generated.mnemonic.expose_secret())],
            )
            .map_err(failure)?;
    }
    if !ctx.output.options.json_mode {
        ctx.output.message("once stored, wipe the mnemonic from this terminal: run `clear && printf '\\e[3J'` (or clear terminal scrollback)").map_err(failure)?;
    }
    Ok(())
}

/// Run a Keychain leaf with an injectable subprocess runner and platform.
/// Only public entry metadata reaches output; password pipes stay zeroizing.
pub async fn run_keychain_with<R: crate::process::ProcessRunner>(
    ctx: &mut AppContext,
    group: &ArgMatches,
    history: &mut PromptHistory,
    runner: R,
    macos: bool,
) -> Result<(), CliError> {
    use crate::macos_password::{KEYCHAIN_SERVICE, MacosPasswords};

    let (name, _) = group
        .subcommand()
        .ok_or_else(|| input("missing wallet keychain command"))?;
    let help = match name {
        "save" => "Wallet whose coldkey password to save.",
        "show" => "Wallet whose keychain entry to check.",
        "delete" => "Wallet whose keychain entry to remove.",
        _ => return Err(input("unknown wallet keychain command")),
    };
    ctx.confirm_wallet(
        &WalletConfirmation {
            help: help.into(),
            require_coldkey: false,
            allow_multisig: false,
            ..Default::default()
        },
        history,
    )?;
    if !macos {
        return Err(failure("macOS Keychain is only available on darwin"));
    }
    let mut passwords = MacosPasswords::with_platform(runner, macos);
    let wallet = &ctx.globals.wallet_name;
    let (title, fields) = match name {
        "save" => {
            let password = passwords
                .prompt_password(
                    &format!("Save {wallet} coldkey password"),
                    &format!(
                        "Enter the password for wallet {}. It will be stored in your macOS Keychain.",
                        rtao::primitives::pyfmt::string_repr(wallet),
                    ),
                )
                .await
                .map_err(failure)?;
            passwords
                .keychain_save(wallet, &password)
                .await
                .map_err(failure)?;
            (
                "saved keychain password",
                vec![
                    ("wallet".into(), json!(wallet)),
                    ("service".into(), json!(KEYCHAIN_SERVICE)),
                    ("account".into(), json!(wallet)),
                ],
            )
        }
        "show" => {
            let stored = passwords
                .keychain_load(wallet)
                .await
                .map_err(failure)?
                .is_some();
            (
                "keychain",
                vec![
                    ("wallet".into(), json!(wallet)),
                    ("service".into(), json!(KEYCHAIN_SERVICE)),
                    ("stored".into(), json!(stored)),
                ],
            )
        }
        "delete" => {
            let existed = passwords.keychain_delete(wallet).await.map_err(failure)?;
            (
                "deleted keychain password",
                vec![
                    ("wallet".into(), json!(wallet)),
                    ("existed".into(), json!(existed)),
                ],
            )
        }
        _ => unreachable!("validated Keychain leaf"),
    };
    ctx.output
        .detail(Some(title), &fields, None)
        .map_err(failure)
}

async fn save_password_env(ctx: &AppContext) -> Result<(), CliError> {
    let wallet = ctx.wallet()?;
    let name = rtao::wallet::env_var_name(wallet.coldkey_file().path());
    if !name
        .bytes()
        .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
    {
        return Err(input("wallet path does not produce a valid shell variable name")
            .with_help("choose a wallet path whose encoded environment name contains only ASCII letters, digits and underscores"));
    }
    let password = rtao::wallet::resolve_wallet_password(&wallet, &ctx.passwords)
        .await
        .map_err(failure)?
        .filter(|password| !password.expose_secret().is_empty())
        .ok_or_else(|| input("a coldkey password is required"))?;
    let encoded = Zeroizing::new(
        rtao::core::keyfile_env::encode_password_for_environment(&name, password.expose_secret())
            .map_err(failure)?,
    );
    let command_length = "export "
        .len()
        .checked_add(name.len())
        .and_then(|length| length.checked_add(1))
        .and_then(|length| length.checked_add(encoded.len()))
        .ok_or_else(|| failure("password export is too large"))?;
    let mut command = Zeroizing::new(String::with_capacity(command_length));
    command.push_str("export ");
    command.push_str(&name);
    command.push('=');
    command.push_str(&encoded);
    if ctx.output.options().json_mode {
        ctx.output
            .secret_detail(
                None,
                &[("name".into(), json!(name))],
                &[("value", encoded.as_str()), ("export", command.as_str())],
            )
            .map_err(failure)
    } else {
        ctx.output.secret_line(&[command.as_str()]).map_err(failure)
    }
}

fn show(ctx: &mut AppContext, history: &mut PromptHistory) -> Result<(), CliError> {
    ctx.confirm_wallet(
        &WalletConfirmation {
            help: "Wallet to show.".into(),
            require_coldkey: false,
            ..Default::default()
        },
        history,
    )?;
    if let Some(entry) = ctx.books.multisig(&ctx.globals.wallet_name) {
        let address = ctx.resolve_address("coldkey_ss58", None)?;
        let mut fields = vec![
            ("multisig_address".into(), json!(address)),
            ("threshold".into(), entry["threshold"].clone()),
            ("signatories".into(), entry["signatories"].clone()),
        ];
        if entry
            .get("note")
            .is_some_and(|note| note.as_str().is_some_and(|note| !note.is_empty()))
        {
            fields.push(("note".into(), entry["note"].clone()));
        }
        return ctx
            .output
            .detail(Some(&ctx.globals.wallet_name), &fields, None)
            .map_err(failure);
    }
    let wallet = ctx.wallet()?;
    let mut fields = Vec::new();
    let cold = wallet.public_info(SignerRole::Coldkey);
    if let Ok(info) = &cold {
        fields.push(("coldkey_ss58".into(), json!(info.account)));
        fields.push((
            "coldkey_crypto_type".into(),
            json!(crypto_name(info.crypto_type)),
        ));
    } else {
        fields.push((
            "coldkey".into(),
            json!("unavailable (no readable public companion)"),
        ));
    }
    let hot = wallet.public_info(SignerRole::Hotkey);
    fields.push(("hotkey".into(), json!(ctx.globals.hotkey_name)));
    if let Ok(info) = &hot {
        fields.push(("hotkey_ss58".into(), json!(info.account)));
        fields.push((
            "hotkey_crypto_type".into(),
            json!(crypto_name(info.crypto_type)),
        ));
    } else if let Some((_, value)) = fields.iter_mut().find(|(name, _)| name == "hotkey") {
        *value = json!(format!(
            "{} — unavailable (no readable public key)",
            ctx.globals.hotkey_name
        ));
    }
    if cold.is_err() && hot.is_err() {
        return Err(failure(format!(
            "wallet {} has no readable keys",
            rtao::primitives::pyfmt::string_repr(&ctx.globals.wallet_name)
        )));
    }
    fields.push((
        "path".into(),
        json!(ctx.globals.wallet_path.to_string_lossy()),
    ));
    ctx.output
        .detail(Some(&ctx.globals.wallet_name), &fields, None)
        .map_err(failure)
}

// Unicode 16 folding has a stable mapping for Unicode 15 assigned characters.
// Leave newer characters unchanged, matching the Python 3.12 oracle. Neither
// dependency may silently advance its data version.
const _: () = {
    assert!(
        caseless::UNICODE_VERSION.0 == 16
            && caseless::UNICODE_VERSION.1 == 0
            && caseless::UNICODE_VERSION.2 == 0
    );
    assert!(
        unicode_general_category::UNICODE_VERSION.0 == 15
            && unicode_general_category::UNICODE_VERSION.1 == 0
            && unicode_general_category::UNICODE_VERSION.2 == 0
    );
};
fn casefold(value: &str) -> String {
    let mut folded = String::new();
    for ch in value.chars() {
        if unicode_general_category::get_general_category(ch)
            == unicode_general_category::GeneralCategory::Unassigned
        {
            folded.push(ch);
        } else {
            folded.push_str(&caseless::default_case_fold_str(&ch.to_string()));
        }
    }
    folded
}
/// Python's natural wallet-name order, including arbitrary-length Unicode digits.
pub fn natural_cmp(left: &str, right: &str) -> Ordering {
    let chunks = |value: &str| {
        let mut result = Vec::<(bool, String)>::new();
        for ch in value.chars() {
            let digit = unicode_general_category::get_general_category(ch)
                == unicode_general_category::GeneralCategory::DecimalNumber;
            if let Some((previous, text)) = result.last_mut()
                && *previous == digit
            {
                text.push(ch);
            } else {
                result.push((digit, ch.to_string()));
            }
        }
        result
    };
    let left = chunks(left);
    let right = chunks(right);
    for ((ld, left), (rd, right)) in left.iter().zip(&right) {
        let order = match (*ld, *rd) {
            (true, true) => rtao::primitives::pyfmt::parse_integer(left)
                .cmp(&rtao::primitives::pyfmt::parse_integer(right)),
            (false, false) => casefold(left).cmp(&casefold(right)),
            (true, false) => Ordering::Less,
            (false, true) => Ordering::Greater,
        };
        if order != Ordering::Equal {
            return order;
        }
    }
    left.len().cmp(&right.len())
}

#[derive(Default)]
struct PublicToken {
    text: Option<String>,
    integer: Option<i128>,
}
impl<'de> serde::Deserialize<'de> for PublicToken {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct Visitor;
        impl<'de> serde::de::Visitor<'de> for Visitor {
            type Value = PublicToken;
            fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str("a public keyfile field")
            }
            fn visit_str<E: serde::de::Error>(self, value: &str) -> Result<Self::Value, E> {
                Ok(PublicToken {
                    text: Some(value.into()),
                    integer: None,
                })
            }
            fn visit_string<E: serde::de::Error>(self, value: String) -> Result<Self::Value, E> {
                Ok(PublicToken {
                    text: Some(value),
                    integer: None,
                })
            }
            fn visit_i64<E: serde::de::Error>(self, value: i64) -> Result<Self::Value, E> {
                Ok(PublicToken {
                    text: None,
                    integer: Some(value.into()),
                })
            }
            fn visit_u64<E: serde::de::Error>(self, value: u64) -> Result<Self::Value, E> {
                Ok(PublicToken {
                    text: None,
                    integer: Some(value.into()),
                })
            }
            fn visit_bool<E: serde::de::Error>(self, value: bool) -> Result<Self::Value, E> {
                Ok(PublicToken {
                    text: None,
                    integer: Some(if value { 1 } else { 0 }),
                })
            }
            fn visit_f64<E: serde::de::Error>(self, _: f64) -> Result<Self::Value, E> {
                Ok(PublicToken::default())
            }
            fn visit_none<E: serde::de::Error>(self) -> Result<Self::Value, E> {
                Ok(PublicToken::default())
            }
            fn visit_unit<E: serde::de::Error>(self) -> Result<Self::Value, E> {
                Ok(PublicToken::default())
            }
            fn visit_seq<A: serde::de::SeqAccess<'de>>(
                self,
                mut seq: A,
            ) -> Result<Self::Value, A::Error> {
                while seq.next_element::<serde::de::IgnoredAny>()?.is_some() {}
                Ok(PublicToken::default())
            }
            fn visit_map<A: serde::de::MapAccess<'de>>(
                self,
                mut map: A,
            ) -> Result<Self::Value, A::Error> {
                while map
                    .next_entry::<serde::de::IgnoredAny, serde::de::IgnoredAny>()?
                    .is_some()
                {}
                Ok(PublicToken::default())
            }
        }
        deserializer.deserialize_any(Visitor)
    }
}
#[derive(Default)]
struct PublicFile {
    ss58: Option<String>,
    crypto: Option<i128>,
}
impl<'de> serde::Deserialize<'de> for PublicFile {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct Visitor;
        impl<'de> serde::de::Visitor<'de> for Visitor {
            type Value = PublicFile;
            fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str("a public keyfile object")
            }
            fn visit_map<A: serde::de::MapAccess<'de>>(
                self,
                mut map: A,
            ) -> Result<Self::Value, A::Error> {
                let mut ss58 = None;
                let mut primary = None;
                let mut alias = None;
                while let Some(key) = map.next_key::<String>()? {
                    match key.as_str() {
                        "ss58Address" => ss58 = map.next_value::<PublicToken>()?.text,
                        "cryptoType" => primary = Some(map.next_value::<PublicToken>()?.integer),
                        "crypto_type" => alias = map.next_value::<PublicToken>()?.integer,
                        _ => {
                            let _: serde::de::IgnoredAny = map.next_value()?;
                        }
                    }
                }
                Ok(PublicFile {
                    ss58,
                    crypto: primary.unwrap_or(alias),
                })
            }
        }
        deserializer.deserialize_map(Visitor)
    }
}
fn public_fields(path: &Path) -> PublicFile {
    let result = (|| {
        let bytes = rtao::wallet::read_keyfile_data(path).ok()?;
        serde_json::from_slice::<PublicFile>(&bytes).ok()
    })();
    result.unwrap_or_default()
}
fn stored_crypto(value: Option<i128>) -> String {
    match value {
        None | Some(1) => "sr25519".into(),
        Some(0) => "ed25519".into(),
        Some(value) => format!("unknown({value})"),
    }
}
/// Read only public fields from local files, retaining missing/encrypted entries.
/// Lock artifacts and public hotkey companions are excluded as in Python.
pub async fn list_records(path: &Path) -> Result<Vec<Value>, CliError> {
    let root = WalletRef::new("default", "default", path)
        .map_err(failure)?
        .path()
        .to_owned();
    rtao::core::offload(move|| {
        let Ok(entries)=fs::read_dir(&root) else{return Ok(Vec::new());};
        let mut wallets=entries.filter_map(Result::ok).filter(|entry|entry.path().is_dir()).collect::<Vec<_>>();
        wallets.sort_by(|left,right|natural_cmp(&left.file_name().to_string_lossy(),&right.file_name().to_string_lossy()));
        let mut records=Vec::new();
        for entry in wallets {
            let name=entry.file_name().to_string_lossy().into_owned();
            let info=public_fields(&entry.path().join("coldkeypub.txt"));
            let mut hotkeys=Vec::new();
            if let Ok(entries)=fs::read_dir(entry.path().join("hotkeys")) {
                let mut entries=entries.filter_map(Result::ok).filter(|entry|entry.path().is_file()).collect::<Vec<_>>();
                entries.sort_by(|left,right|natural_cmp(&left.file_name().to_string_lossy(),&right.file_name().to_string_lossy()));
                for entry in entries {
                    let name=entry.file_name().to_string_lossy().into_owned();
                    if name.ends_with(".lock")||name.ends_with("pub.txt"){continue;}
                    let info=public_fields(&entry.path());
                    hotkeys.push(json!({"name":name,"ss58":info.ss58,"crypto_type":stored_crypto(info.crypto)}));
                }
            }
            records.push(json!({"coldkey":name,"ss58":info.ss58,"crypto_type":stored_crypto(info.crypto),"hotkeys":hotkeys}));
        }
        Ok::<_,CliError>(records)
    }).await.map_err(|_|failure("wallet listing task failed"))?
}
async fn list(ctx: &AppContext) -> Result<(), CliError> {
    let records = list_records(&ctx.globals.wallet_path).await?;
    let lookup = FsAccountLookup::new(ctx.globals.wallet_path.clone()).map_err(failure)?;
    let resolver = NameResolver::new(
        &ctx.books,
        &lookup,
        &ctx.globals.wallet_name,
        &ctx.globals.hotkey_name,
    );
    let mut multisigs = Vec::new();
    for entry in &ctx.books.multisigs {
        let Some(name) = entry["name"].as_str().filter(|name| !name.is_empty()) else {
            continue;
        };
        let refs = entry["signatories"].as_array().cloned().unwrap_or_default();
        let signatories = refs
            .iter()
            .map(|reference| {
                let address = reference
                    .as_str()
                    .and_then(|name| resolver.resolve_address_ref("coldkey_ss58", name).ok())
                    .map(|entry| entry.address);
                json!({"name":reference,"ss58":address})
            })
            .collect::<Vec<_>>();
        multisigs.push(json!({"name":name,"kind":"multisig","threshold":entry["threshold"],"signatory_count":refs.len(),"ss58":resolver.saved_multisig_address(name).ok().flatten(),"signatories":signatories,"note":entry.get("note").cloned().unwrap_or(json!(""))}));
    }
    multisigs.sort_by_key(|entry| entry["name"].as_str().unwrap_or("").to_lowercase());
    let payload = json!({"path":ctx.globals.wallet_path.to_string_lossy(),"coldkeys":records,"multisigs":multisigs,"addresses":ctx.books.addresses,"proxies":ctx.books.proxies});
    if ctx.output.options.json_mode {
        return ctx.output.value(&payload).map_err(failure);
    }
    let mut nodes = Vec::new();
    for record in payload["coldkeys"].as_array().into_iter().flatten() {
        let branch = format!(
            "{}  {}  {}",
            record["coldkey"].as_str().unwrap_or(""),
            record["ss58"].as_str().unwrap_or("unavailable"),
            record["crypto_type"].as_str().unwrap_or("")
        );
        let leaves = record["hotkeys"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|hot| {
                format!(
                    "{}  {}  {}",
                    hot["name"].as_str().unwrap_or(""),
                    hot["ss58"].as_str().unwrap_or("unavailable"),
                    hot["crypto_type"].as_str().unwrap_or("")
                )
            })
            .collect();
        nodes.push((branch, leaves));
    }
    ctx.output
        .tree(&ctx.globals.wallet_path.to_string_lossy(), &nodes, None)
        .map_err(failure)?;
    for key in ["multisigs", "addresses", "proxies"] {
        if payload[key].as_array().is_some_and(|rows| !rows.is_empty()) {
            ctx.output.query(key, &payload[key]).map_err(failure)?;
        }
    }
    Ok(())
}

fn saved_accounts(ctx: &AppContext) -> Result<Vec<(String, AccountId)>, CliError> {
    let lookup = FsAccountLookup::new(ctx.globals.wallet_path.clone()).map_err(failure)?;
    let resolver = NameResolver::new(
        &ctx.books,
        &lookup,
        &ctx.globals.wallet_name,
        &ctx.globals.hotkey_name,
    );
    Ok(ctx
        .books
        .multisigs
        .iter()
        .filter_map(|entry| {
            let name = entry["name"].as_str().filter(|name| !name.is_empty())?;
            let address = resolver
                .saved_multisig_address(name)
                .ok()
                .flatten()?
                .parse()
                .ok()?;
            Some((name.into(), address))
        })
        .collect())
}

mod chain {
    use super::*;
    use crate::{books::is_bittensor_address, output::PythonBalance};
    use rtao::{
        client::{BlockRef, Snapshot, View, ViewExt},
        core::codec::Value as CoreValue,
        primitives::{DynBalance, NetUid, pyfmt, ratio},
        reads::reads::owner::{self, StakeValuation},
    };
    use std::collections::{BTreeMap, BTreeSet};

    include!("wallet_m8/views.rs");

    const BASIS: &str = "spot price; excludes slippage/fees of an actual unstake";
    const STAKE_TITLE: &str = "stake (per-subnet currency: TAO on netuid 0, alpha elsewhere)";
    const DUST_RAO: u64 = 1_000_000;

    fn sum(values: impl IntoIterator<Item = u64>, label: &str) -> Result<u64, CliError> {
        values.into_iter().try_fold(0u64, |total, value| {
            total
                .checked_add(value)
                .ok_or_else(|| input(format!("{label} exceeds native u64 rao")))
        })
    }

    fn tao(rao: u64) -> DynBalance {
        DynBalance::from_rao(rao, NetUid::ROOT)
    }

    fn display(value: &DynBalance) -> Value {
        json!(PythonBalance(value.clone()))
    }

    fn spot(value: &DynBalance, valuation: &StakeValuation) -> Result<DynBalance, CliError> {
        if value.netuid == NetUid::ROOT {
            return Ok(tao(value.rao));
        }
        let price = valuation
            .tao_per_alpha
            .get(&value.netuid)
            .copied()
            .unwrap_or(0.0);
        let amount = (value.rao as f64 * price).trunc();
        if !(0.0..18_446_744_073_709_551_616.0).contains(&amount) {
            return Err(input("spot valuation exceeds native u64 rao"));
        }
        Ok(tao(amount as u64))
    }

    // Python formats Decimal(rao) / 10^9 here, with decimal half-even rounding.
    // Formatting the binary64 amount instead would change millitao ties.
    fn compact(value: &DynBalance) -> String {
        let mut milli = value.rao / 1_000_000;
        let remainder = value.rao % 1_000_000;
        if remainder > 500_000 || (remainder == 500_000 && !milli.is_multiple_of(2)) {
            milli += 1;
        }
        let digits = (milli / 1_000).to_string();
        let mut grouped = String::new();
        for (index, digit) in digits.chars().enumerate() {
            if index != 0 && (digits.len() - index).is_multiple_of(3) {
                grouped.push(',');
            }
            grouped.push(digit);
        }
        format!("{}{grouped}.{:03}", value.unit(), milli % 1_000)
    }

    struct BalanceRow {
        name: String,
        coldkey: AccountId,
        free: DynBalance,
        valuation: StakeValuation,
        beta: DynBalance,
        total: DynBalance,
    }

    impl BalanceRow {
        fn new(
            name: String,
            coldkey: AccountId,
            free: DynBalance,
            valuation: StakeValuation,
            beta: DynBalance,
        ) -> Result<Self, CliError> {
            let total = tao(sum(
                [free.rao, valuation.stake_value.rao, beta.rao],
                "total value",
            )?);
            Ok(Self {
                name,
                coldkey,
                free,
                valuation,
                beta,
                total,
            })
        }

        fn record(&self) -> Value {
            json!({
                "wallet": self.name,
                "coldkey": self.coldkey,
                "free": display(&self.free),
                "free_tao": self.free.amount(),
                "stake_positions": self.valuation.positions.len(),
                "stake_subnets": self.valuation.positions.iter().map(|p|p.netuid).collect::<BTreeSet<_>>().len(),
                "stake_value": display(&self.valuation.stake_value),
                "stake_value_tao": self.valuation.stake_value.amount(),
                "beta_value": display(&self.beta),
                "beta_value_tao": self.beta.amount(),
                "total_value": display(&self.total),
                "total_value_tao": self.total.amount(),
                "stake_value_basis": BASIS,
                "block": self.valuation.block,
            })
        }

        fn human_fields(
            &self,
            beta: (f64, usize),
            locked: Option<(&DynBalance, usize)>,
        ) -> Vec<(String, Value)> {
            let subnets = self
                .valuation
                .positions
                .iter()
                .map(|p| p.netuid)
                .collect::<BTreeSet<_>>()
                .len();
            let beta_text = if beta.1 == 0 {
                "—".into()
            } else {
                format!(
                    "{} β · {} validator{}",
                    pyfmt::fmt_comma_9f(beta.0),
                    beta.1,
                    if beta.1 == 1 { "" } else { "s" }
                )
            };
            let mut fields = vec![
                (
                    "wallet".into(),
                    json!(format!("{} ({})", self.name, self.coldkey)),
                ),
                ("free".into(), json!(compact(&self.free))),
                (
                    "alpha".into(),
                    json!(format!(
                        "{} positions · {subnets} subnets",
                        self.valuation.positions.len()
                    )),
                ),
                ("beta".into(), json!(beta_text)),
                (
                    "beta_value".into(),
                    json!(format!(
                        "{}  (claimable root dividends)",
                        compact(&self.beta)
                    )),
                ),
                (
                    "alpha_value".into(),
                    json!(format!(
                        "{}  (spot, excl. slippage/fees)",
                        compact(&self.valuation.stake_value)
                    )),
                ),
            ];
            if let Some((value, count)) = locked.filter(|(_, count)| *count != 0) {
                fields.push((
                    "locked_value".into(),
                    json!(format!(
                        "{}  ({count} subnets; part of alpha_value)",
                        compact(value)
                    )),
                ));
            }
            fields.push(("total_value".into(), json!(compact(&self.total))));
            fields.push(("block".into(), json!(self.valuation.block)));
            fields
        }
    }

    async fn snapshot(client: &Client, matches: &ArgMatches) -> Result<Snapshot, CliError> {
        match matches.try_get_one::<u64>("__block").ok().flatten() {
            Some(block) => client.at(BlockRef::Number(*block)).await.map_err(failure),
            None => client.pin().await.map_err(failure),
        }
    }

    async fn positions(
        view: &Snapshot,
        coldkey: AccountId,
    ) -> Result<Vec<(AccountId, u64, u64)>, CliError> {
        // Resolve by name, matching Python and supporting compatible older Beta APIs.
        let value = view
            .call_dyn(
                "BetaBasketRuntimeApi",
                "get_root_basket_positions",
                &[CoreValue::Str(coldkey.to_string())],
            )
            .await
            .map_err(failure)?
            .to_json();
        if value.is_null() {
            return Ok(Vec::new());
        }
        serde_json::from_value(value)
            .map_err(|error| failure(format!("get_root_basket_positions changed shape: {error}")))
    }

    async fn locked_value(
        view: &Snapshot,
        coldkey: AccountId,
        valuation: &StakeValuation,
    ) -> Result<(DynBalance, usize), CliError> {
        let netuids: Vec<_> = valuation
            .positions
            .iter()
            .map(|p| p.netuid)
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect();
        if netuids.is_empty() {
            return Ok((tao(0), 0));
        }
        let value = view.call(
            &rtao_runtime::runtime_apis::stake_info_runtime_api::GET_STAKE_AVAILABILITY_FOR_COLDKEYS,
            (vec![coldkey], Some(netuids)),
        ).await.map_err(failure)?.to_json();
        if value.is_null() {
            return Ok((tao(0), 0));
        }
        let accounts = value
            .as_object()
            .ok_or_else(|| failure("stake availability is not an account map"))?;
        let Some(entries) = accounts.get(&coldkey.to_string()) else {
            return Ok((tao(0), 0));
        };
        if entries.is_null() {
            return Ok((tao(0), 0));
        }
        let entries = entries
            .as_object()
            .ok_or_else(|| failure("stake availability is not a subnet map"))?;
        let mut amounts = Vec::new();
        for (netuid, record) in entries {
            let netuid = NetUid(netuid.parse::<u16>().map_err(failure)?);
            let rao = match record.get("locked") {
                None | Some(Value::Null) => 0,
                Some(value) => value
                    .as_u64()
                    .ok_or_else(|| failure("locked stake is not native u64 rao"))?,
            };
            if rao != 0 {
                amounts.push(spot(&DynBalance::from_rao(rao, netuid), valuation)?.rao);
            }
        }
        Ok((
            tao(sum(amounts.iter().copied(), "locked value")?),
            amounts.len(),
        ))
    }

    pub(super) async fn balance(
        ctx: &mut AppContext,
        client: &Client,
        matches: &ArgMatches,
        _history: &mut PromptHistory,
    ) -> Result<(), CliError> {
        if matches.get_flag("all") {
            return all_balances(ctx, client, matches).await;
        }
        let address = matches.get_one::<String>("address").map(String::as_str);
        let coldkey = ctx
            .resolve_address("coldkey_ss58", address)?
            .ok_or_else(|| input("missing coldkey"))?;
        let label = address
            .filter(|address| !is_bittensor_address(address))
            .unwrap_or(&ctx.globals.wallet_name)
            .to_owned();
        let view = snapshot(client, matches).await?;
        let valuation = owner::stake_value_for_coldkey(&view, coldkey)
            .await
            .map_err(failure)?;
        let (free, beta) = tokio::try_join!(
            owner::balance(&view, coldkey),
            owner::root_basket_owed(&view, coldkey)
        )
        .map_err(failure)?;
        let (locked, positions) = tokio::try_join!(
            locked_value(&view, coldkey, &valuation),
            positions(&view, coldkey)
        )?;
        let beta_tokens =
            pyfmt::rao_to_f64(sum(positions.iter().map(|p| p.1), "beta token holdings")?);
        let row = BalanceRow::new(label, coldkey, free, valuation, beta)?;
        let mut record = row.record();
        record["locked_value"] = display(&locked.0);
        record["locked_value_tao"] = json!(locked.0.amount());
        record["locked_subnets"] = json!(locked.1);
        record["beta_tokens"] = json!(beta_tokens);
        record["beta_validators"] = json!(positions.len());
        ctx.output
            .detail(
                None,
                &row.human_fields((beta_tokens, positions.len()), Some((&locked.0, locked.1))),
                Some(&record),
            )
            .map_err(failure)
    }

    async fn all_balances(
        ctx: &mut AppContext,
        client: &Client,
        matches: &ArgMatches,
    ) -> Result<(), CliError> {
        let sort = matches
            .get_one::<String>("sort")
            .map_or("total", String::as_str);
        if ![
            "name",
            "free",
            "alpha",
            "beta",
            "total",
            "stake-value",
            "total-value",
        ]
        .contains(&sort)
        {
            return Err(input(format!(
                "unknown sort key {sort:?}; use: name, free, alpha, beta, or total"
            )));
        }
        let mut accounts = Vec::new();
        for row in super::list_records(&ctx.globals.wallet_path).await? {
            if let Some(address) = row
                .get("ss58")
                .and_then(Value::as_str)
                .filter(|address| !address.is_empty())
            {
                let name = row
                    .get("coldkey")
                    .and_then(Value::as_str)
                    .ok_or_else(|| failure("wallet listing has no coldkey name"))?;
                accounts.push((
                    name.to_owned(),
                    address.parse::<AccountId>().map_err(failure)?,
                ));
            }
        }
        let saved = super::saved_accounts(ctx)?;
        let saved_by_name: BTreeMap<_, _> = saved.iter().cloned().collect();
        accounts.extend(saved);
        if accounts.is_empty() {
            return Err(failure(format!(
                "no wallets found in {}",
                ctx.globals.wallet_path.display()
            )));
        }
        let addresses: Vec<_> = accounts
            .iter()
            .map(|(_, address)| *address)
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect();
        let view = snapshot(client, matches).await?;
        let (valuations, balances) = tokio::try_join!(
            owner::stake_value_for_coldkeys(&view, addresses.clone()),
            owner::balances(&view, addresses.clone())
        )
        .map_err(failure)?;
        let mut owed = BTreeMap::new();
        for address in addresses {
            owed.insert(
                address,
                owner::root_basket_owed(&view, address)
                    .await
                    .map_err(failure)?,
            );
        }
        let mut rows = accounts
            .into_iter()
            .map(|(name, address)| {
                let valuation = valuations
                    .get(&address)
                    .cloned()
                    .ok_or_else(|| failure("stake valuation omitted a requested coldkey"))?;
                let free = balances
                    .get(&address)
                    .cloned()
                    .ok_or_else(|| failure("balance read omitted a requested coldkey"))?;
                let beta = owed
                    .get(&address)
                    .cloned()
                    .ok_or_else(|| failure("root-dividend read omitted a requested coldkey"))?;
                BalanceRow::new(name, address, free, valuation, beta)
            })
            .collect::<Result<Vec<_>, CliError>>()?;
        rows.sort_by(|left, right| {
            if sort == "name" {
                return super::natural_cmp(&left.name, &right.name);
            }
            let amount = |row: &BalanceRow| match sort {
                "free" => row.free.amount(),
                "alpha" | "stake-value" => row.valuation.stake_value.amount(),
                "beta" => row.beta.amount(),
                _ => row.total.amount(),
            };
            amount(right).total_cmp(&amount(left))
        });
        let unique: BTreeMap<_, _> = rows
            .iter()
            .map(|row| (row.coldkey, row.total.rao))
            .collect();
        let grand_total = tao(sum(unique.values().copied(), "wallet grand total")?);
        let duplicate_count = rows.len() - unique.len();
        let records: Vec<_> = rows
            .iter()
            .map(|row| {
                let mut record = row.record();
                record["kind"] = json!(if saved_by_name.get(&row.name) == Some(&row.coldkey) {
                    "multisig"
                } else {
                    "wallet"
                });
                record
            })
            .collect();
        let mut hidden = 0;
        let table_rows = rows
            .iter()
            .filter_map(|row| {
                if !matches.get_flag("empty") && row.total.rao == 0 {
                    hidden += 1;
                    return None;
                }
                let amount = |value: &DynBalance| {
                    if value.rao == 0 {
                        "—".into()
                    } else {
                        compact(value)
                    }
                };
                let suffix = if saved_by_name.get(&row.name) == Some(&row.coldkey) {
                    " (multisig)"
                } else {
                    ""
                };
                Some(vec![
                    json!(format!("{}{suffix}", row.name)),
                    json!(amount(&row.free)),
                    json!(amount(&row.valuation.stake_value)),
                    json!(amount(&row.beta)),
                    json!(amount(&row.total)),
                    json!(row.coldkey),
                ])
            })
            .collect();
        let mut table = TableSpec::new(
            "wallet balances (alpha stake at spot; beta = claimable root dividends)",
            [
                "name",
                "free (TAO)",
                "alpha (TAO)",
                "beta (TAO)",
                "total (TAO)",
                "coldkey",
            ]
            .into_iter()
            .map(str::to_owned)
            .collect(),
            table_rows,
        );
        table.records = Some(json!(records));
        table.right_align.extend([1, 2, 3, 4]);
        let duplicate_note = if duplicate_count == 0 {
            String::new()
        } else {
            format!("; {duplicate_count} duplicate-coldkey wallets counted once")
        };
        let footer = format!(
            "total {}  (alpha at spot; beta realizable{duplicate_note})",
            compact(&grand_total)
        );
        ctx.output.columns(&table).map_err(failure)?;
        if !ctx.output.options.json_mode {
            ctx.output.value(&footer).map_err(failure)?;
        }
        if hidden != 0 {
            ctx.output
                .message(&format!(
                    "{hidden} empty wallets hidden — pass --empty to show"
                ))
                .map_err(failure)?;
        }
        Ok(())
    }

    fn yield_record(accrued: &DynBalance, staked: &DynBalance) -> Value {
        json!({
            "accrued_basket_yield": display(accrued), "accrued_basket_yield_tao": accrued.amount(),
            "root_staked": display(staked), "root_staked_tao": staked.amount(),
            "personal_yield": ratio::integer_division(i128::from(accrued.rao), staked.rao), "auto_claim": false,
        })
    }

    fn extend_yield(record: &mut Value, yields: &Value) -> Result<(), CliError> {
        let fields = yields
            .as_object()
            .ok_or_else(|| failure("root yield is not a record"))?;
        record
            .as_object_mut()
            .ok_or_else(|| failure("wallet output is not a record"))?
            .extend(fields.clone());
        Ok(())
    }

    pub(super) async fn inspect(
        ctx: &mut AppContext,
        client: &Client,
        matches: &ArgMatches,
        history: &mut PromptHistory,
    ) -> Result<(), CliError> {
        let explicit = matches
            .get_one::<String>("coldkey_ss58")
            .map(String::as_str);
        let selected = if explicit.is_none() {
            ctx.confirm_wallet(
                &WalletConfirmation {
                    help: "Wallet to inspect.".into(),
                    ..Default::default()
                },
                history,
            )?
        } else {
            None
        };
        let coldkey = match selected {
            Some(address) => address,
            None => ctx
                .resolve_address("coldkey_ss58", explicit)?
                .ok_or_else(|| input("missing coldkey"))?,
        };
        let view = snapshot(client, matches).await?;
        let (valuation, delegated, identity, beta) = tokio::try_join!(
            owner::stake_value_for_coldkey(&view, coldkey),
            owner::delegated(&view, coldkey),
            owner::identity(&view, coldkey),
            owner::root_basket_owed(&view, coldkey),
        )
        .map_err(failure)?;
        let free = owner::balance(&view, coldkey).await.map_err(failure)?;
        let raw_baskets = positions(&view, coldkey).await?;
        sum(
            raw_baskets.iter().map(|position| position.1),
            "beta token holdings",
        )?;
        let root = tao(sum(
            valuation
                .positions
                .iter()
                .filter(|p| p.netuid == NetUid::ROOT)
                .map(|p| p.stake.rao),
            "root principal",
        )?);
        let yields = yield_record(&beta, &root);
        let mut baskets = Vec::new();
        for (hotkey, beta_raw, payout) in &raw_baskets {
            let summary = owner::validator_basket_summary(&view, *hotkey)
                .await
                .map_err(failure)?;
            baskets.push(json!({
                "hotkey": hotkey, "beta_raw": beta_raw, "beta": pyfmt::rao_to_f64(*beta_raw),
                "claimable": display(&tao(*payout)), "claimable_tao": pyfmt::rao_to_f64(*payout),
                "beta_price_tao": summary.beta_price_tao,
                "fund_share": ratio::integer_division(i128::from(*beta_raw), summary.beta_total_raw).unwrap_or(0.0),
                "fund_nav": display(&summary.nav_tao), "lifetime_return": summary.lifetime_return,
            }));
        }
        baskets.sort_by(|left, right| {
            let amount = |value: &Value| {
                value
                    .get("claimable_tao")
                    .and_then(Value::as_f64)
                    .unwrap_or(0.0)
            };
            amount(right).total_cmp(&amount(left))
        });
        let beta_tokens: f64 = baskets
            .iter()
            .filter_map(|row| row.get("beta").and_then(Value::as_f64))
            .sum();
        let mut stakes = Vec::new();
        for position in &valuation.positions {
            let value = spot(&position.stake, &valuation)?;
            let mut record = json!({
                "netuid": position.netuid, "hotkey": position.hotkey, "stake": display(&position.stake),
                "stake_amount": position.stake.amount(), "stake_unit": stake_unit(position.netuid),
                "value": display(&value), "value_tao": value.amount(), "registered": position.is_registered,
            });
            extend_yield(&mut record, &yields)?;
            stakes.push(record);
        }
        let delegations = delegated.iter().map(|position| {
            let value = spot(&position.stake, &valuation)?;
            Ok(json!({
                "netuid": position.netuid, "delegate_hotkey": position.delegate.hotkey, "take": position.delegate.take,
                "stake": display(&position.stake), "stake_amount": position.stake.amount(), "stake_unit": stake_unit(position.netuid),
                "value": display(&value), "value_tao": value.amount(),
            }))
        }).collect::<Result<Vec<Value>, CliError>>()?;
        let row = BalanceRow::new(
            ctx.globals.wallet_name.clone(),
            coldkey,
            free,
            valuation,
            beta,
        )?;
        let mut balance = row.record();
        balance["beta_tokens"] = json!(beta_tokens);
        balance["beta_validators"] = json!(baskets.len());
        extend_yield(&mut balance, &yields)?;
        let mut data = json!({"balance": balance,"stakes": stakes,"delegations": delegations,"baskets": baskets,"identity": identity});
        extend_yield(&mut data, &yields)?;
        // Python resolves labels before choosing JSON or human output, but the
        // labels are presentation only and never become machine-record fields.
        let names = local_names(ctx).await?;
        let hotkeys = row
            .valuation
            .positions
            .iter()
            .map(|p| p.hotkey)
            .chain(delegated.iter().map(|d| d.delegate.hotkey))
            .chain(raw_baskets.iter().map(|p| p.0))
            .filter(|hotkey| !names.contains_key(hotkey))
            .collect();
        let identities = owner::hotkey_identities(&view, hotkeys)
            .await
            .map_err(failure)?;
        if ctx.output.options.json_mode {
            return ctx.output.value(&data).map_err(failure);
        }
        ctx.output
            .update_token_symbols(
                client
                    .token_symbols()
                    .await
                    .into_iter()
                    .map(|(netuid, symbol)| (netuid.get(), rtao::primitives::Symbol::new(symbol))),
            )
            .map_err(failure)?;
        ctx.output
            .detail(
                None,
                &row.human_fields((beta_tokens, raw_baskets.len()), None),
                None,
            )
            .map_err(failure)?;
        render_stakes(
            &ctx.output,
            &row,
            matches.get_flag("dust"),
            &names,
            &identities,
        )?;
        if !baskets.is_empty() {
            render_baskets(&ctx.output, &baskets, &names, &identities)?;
        }
        match identity {
            Some(Value::Object(fields)) if !fields.is_empty() => ctx
                .output
                .detail(
                    Some("identity"),
                    &fields.into_iter().collect::<Vec<_>>(),
                    None,
                )
                .map_err(failure)?,
            _ => ctx
                .output
                .message("no on-chain identity")
                .map_err(failure)?,
        }
        Ok(())
    }

    fn stake_unit(netuid: NetUid) -> String {
        if netuid == NetUid::ROOT {
            "TAO".into()
        } else {
            format!("alpha (netuid {netuid})")
        }
    }

    async fn local_names(ctx: &AppContext) -> Result<BTreeMap<AccountId, String>, CliError> {
        let mut names = BTreeMap::new();
        for row in super::list_records(&ctx.globals.wallet_path).await? {
            if let Some(hotkeys) = row.get("hotkeys").and_then(Value::as_array) {
                for hotkey in hotkeys {
                    if let (Some(address), Some(name)) = (
                        hotkey.get("ss58").and_then(Value::as_str),
                        hotkey.get("name").and_then(Value::as_str),
                    ) && let Ok(address) = address.parse()
                    {
                        names.entry(address).or_insert_with(|| name.into());
                    }
                }
            }
        }
        for entry in &ctx.books.addresses {
            if let (Some(address), Some(name)) = (
                entry.get("address").and_then(Value::as_str),
                entry.get("name").and_then(Value::as_str),
            ) && !name.is_empty()
                && let Ok(address) = address.parse()
            {
                names.entry(address).or_insert_with(|| name.into());
            }
        }
        Ok(names)
    }

    fn label(
        address: &AccountId,
        names: &BTreeMap<AccountId, String>,
        identities: &BTreeMap<AccountId, Value>,
    ) -> String {
        names
            .get(address)
            .cloned()
            .or_else(|| {
                identities
                    .get(address)
                    .and_then(|identity| identity.get("name"))
                    .and_then(Value::as_str)
                    .filter(|name| !name.is_empty())
                    .map(str::to_owned)
            })
            .unwrap_or_else(|| "—".into())
    }

    fn render_stakes(
        output: &Output,
        row: &BalanceRow,
        dust: bool,
        names: &BTreeMap<AccountId, String>,
        identities: &BTreeMap<AccountId, Value>,
    ) -> Result<(), CliError> {
        let mut groups: BTreeMap<NetUid, Vec<_>> = BTreeMap::new();
        for position in &row.valuation.positions {
            groups.entry(position.netuid).or_default().push(position);
        }
        let mut visible = Vec::new();
        let mut hidden_subnets = 0;
        let mut hidden_positions = 0;
        let mut hidden_value = 0.0;
        for (netuid, mut positions) in groups {
            let Some(first) = positions.first() else {
                continue;
            };
            let mut total = first.stake.clone();
            total.rao = sum(positions.iter().map(|p| p.stake.rao), "subnet stake")?;
            let value = spot(&total, &row.valuation)?;
            if !dust && value.rao < DUST_RAO {
                hidden_subnets += 1;
                hidden_value += value.amount();
                continue;
            }
            positions.sort_by_key(|position| std::cmp::Reverse(position.stake.rao));
            for position in positions {
                let value = spot(&position.stake, &row.valuation)?;
                if !dust && value.rao < DUST_RAO {
                    hidden_positions += 1;
                    hidden_value += value.amount();
                    continue;
                }
                visible.push((netuid, position, value));
            }
        }
        visible.sort_by(|left, right| right.2.amount().total_cmp(&left.2.amount()));
        let rows = visible
            .into_iter()
            .map(|(netuid, position, value)| {
                let address = position.hotkey.to_string();
                vec![
                    json!(output.subnet_text(netuid, true)),
                    json!(
                        position
                            .stake
                            .to_python_string()
                            .trim_start_matches(position.stake.unit().as_ref())
                            .trim_end_matches(position.stake.unit().as_ref())
                    ),
                    json!(value.to_python_string().trim_start_matches('τ')),
                    json!(label(&position.hotkey, names, identities)),
                    json!(format!("{}…", address.chars().take(8).collect::<String>())),
                ]
            })
            .collect();
        let mut table = TableSpec::new(
            STAKE_TITLE,
            ["netuid", "stake (α)", "value (τ)", "validator", "hotkey"]
                .into_iter()
                .map(str::to_owned)
                .collect(),
            rows,
        );
        table.right_align.extend([1, 2]);
        let root = tao(sum(
            row.valuation
                .positions
                .iter()
                .filter(|p| p.netuid == NetUid::ROOT)
                .map(|p| p.stake.rao),
            "root principal",
        )?);
        let ratio = ratio::integer_division(i128::from(row.beta.rao), root.rao)
            .map(|value| format!("  ({:.2}% of root stake)", value * 100.0))
            .unwrap_or_default();
        output.columns(&table).map_err(failure)?;
        if !table.rows.is_empty() {
            output
                .value(&format!(
                    "total {}  (spot, excl. slippage/fees)",
                    row.valuation.stake_value.to_python_string()
                ))
                .map_err(failure)?;
        }
        if row.beta.rao != 0 || root.rao != 0 {
            output
                .value(&format!(
                    "accrued basket yield  {}{ratio}\nauto-claim is off → rtao root claim",
                    row.beta.to_python_string()
                ))
                .map_err(failure)?;
        }
        if hidden_subnets != 0 || hidden_positions != 0 {
            let mut parts = Vec::new();
            if hidden_subnets != 0 {
                parts.push(format!(
                    "{hidden_subnets} dust subnet{}",
                    if hidden_subnets == 1 { "" } else { "s" }
                ));
            }
            if hidden_positions != 0 {
                parts.push(format!(
                    "{hidden_positions} dust position{}",
                    if hidden_positions == 1 { "" } else { "s" }
                ));
            }
            output
                .message(&format!(
                    "{} hidden (each < τ0.001, ≈ τ{hidden_value:.6} total) — pass --dust to show",
                    parts.join(" and ")
                ))
                .map_err(failure)?;
        }
        Ok(())
    }

    fn render_baskets(
        output: &Output,
        baskets: &[Value],
        names: &BTreeMap<AccountId, String>,
        identities: &BTreeMap<AccountId, Value>,
    ) -> Result<(), CliError> {
        let rows = baskets
            .iter()
            .map(|record| {
                let hotkey = record
                    .get("hotkey")
                    .and_then(Value::as_str)
                    .ok_or_else(|| failure("basket has no hotkey"))?;
                let address = hotkey.parse::<AccountId>().map_err(failure)?;
                let number = |key| record.get(key).and_then(Value::as_f64).unwrap_or(0.0);
                Ok(vec![
                    json!(label(&address, names, identities)),
                    json!(pyfmt::fmt_comma_9f(number("beta"))),
                    json!(format!("{:.6}", number("beta_price_tao"))),
                    record["claimable"].clone(),
                    json!(format!("{:.4}%", number("fund_share") * 100.0)),
                    json!(
                        record
                            .get("lifetime_return")
                            .and_then(Value::as_f64)
                            .map(|value| format!("{value:.4}x"))
                            .unwrap_or_else(|| "—".into())
                    ),
                    json!(hotkey),
                ])
            })
            .collect::<Result<Vec<_>, CliError>>()?;
        let mut table = TableSpec::new(
            "beta baskets (root dividends escrowed per validator)",
            [
                "validator",
                "beta (β)",
                "τ/β",
                "claimable (τ)",
                "fund share",
                "lifetime",
                "hotkey",
            ]
            .into_iter()
            .map(str::to_owned)
            .collect(),
            rows,
        );
        table.right_align.extend([1, 2, 3, 4, 5]);
        table.footer = Some("beta grows with accruals and shrinks with claims; τ/β is the fund's realizable price (par 1.0 at inception); claim with `rtao root claim`".into());
        output.columns(&table).map_err(failure)
    }
}
