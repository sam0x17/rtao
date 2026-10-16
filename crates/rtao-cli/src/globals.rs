// Derived from RaoFoundation/subtensor sdk/python/bittensor/cli/globals.py @ 850f2755bb05a3851f948489f0ab27b0645cebce (Apache-2.0); modified by rtao.
// Additional source at the same commit/license: sdk/python/bittensor/cli/main.py.
//! Shared command tiers and flag/environment/config precedence.

use clap::{Arg, ArgAction, ArgMatches, Command};
use serde_json::{Map, Value};
use std::path::{Path, PathBuf};

/// The global surface a command can act on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tier {
    /// Network, wallet identity, and output options.
    Read,
    /// Read options and coldkey password sources.
    Unlock,
    /// Read options and extension bridge choices.
    Extension,
    /// All mutation, signing, and password options.
    Tx,
}

/// Resolved CLI settings. Password values are never stored here.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Globals {
    /// Network name or WebSocket URL.
    pub network: String,
    /// Coldkey wallet name.
    pub wallet_name: String,
    /// Hotkey name in the wallet.
    pub hotkey_name: String,
    /// Wallet directory.
    pub wallet_path: PathBuf,
    /// Whether the wallet was supplied by a flag or environment.
    pub wallet_given: bool,
    /// Whether the hotkey was supplied by a flag or environment.
    pub hotkey_given: bool,
    /// Emit value-equal Python JSON.
    pub json: bool,
    /// Suppress informational human output.
    pub quiet: bool,
    /// Number of verbose flags.
    pub verbosity: u8,
    /// Skip mutation confirmation.
    pub assume_yes: bool,
    /// Plan without submitting.
    pub dry_run: bool,
    /// Explicit tri-state shield override; config stays a separate fallback.
    pub mev_shield: Option<bool>,
    /// Proxy reference; `self` bypasses configured dispatch.
    pub proxy_for: Option<String>,
    /// Exact proxy permission category.
    pub force_proxy_type: Option<String>,
    /// Ordered member references for multisig approvals.
    pub signatories: Vec<String>,
    /// Password file, never its contents.
    pub wallet_password_file: Option<PathBuf>,
    /// Enable an explicit macOS password dialog.
    pub macos_password: bool,
    /// Enable explicit macOS Keychain access.
    pub keychain_password: bool,
    /// Wallet, Ledger, Vault or extension signing backend.
    pub signer_backend: Option<String>,
    /// External signer account reference.
    pub signer_address: Option<String>,
    /// Ledger derivation account.
    pub ledger_account: u32,
    /// Ledger derivation address index.
    pub ledger_index: u32,
    /// Extension account source filter.
    pub extension_source: Option<String>,
    /// Extension bridge page browser.
    pub extension_browser: Option<String>,
    /// Extension bridge WebSocket URL.
    pub extension_bridge_url: Option<String>,
    /// Comma-separated fallback endpoint override, or `none`.
    pub fallback_endpoints: Option<String>,
    /// Comma-separated archive endpoint override, or `none`.
    pub archive_endpoints: Option<String>,
    /// Cycle the endpoint pool indefinitely on connection failures.
    pub retry_forever: bool,
}

fn option(id: &'static str, long: &'static str, help: &'static str) -> Arg {
    Arg::new(id)
        .long(long)
        .help(help)
        .value_parser(clap::builder::StringValueParser::new())
}
fn flag(id: &'static str, long: &'static str, help: &'static str) -> Arg {
    Arg::new(id)
        .long(long)
        .help(help)
        .action(ArgAction::SetTrue)
}
fn common() -> Vec<Arg> {
    vec![
        option(
            "network",
            "network",
            "Network name (finney/test/local) or ws:// endpoint.",
        )
        .short('n'),
        option(
            "fallback_endpoints",
            "fallback-endpoints",
            "Comma-separated fallback endpoints ('none' disables).",
        ),
        option(
            "archive_endpoints",
            "archive-endpoints",
            "Comma-separated archive endpoints ('none' disables).",
        ),
        flag(
            "retry_forever",
            "retry-forever",
            "Keep cycling the endpoint pool on connection failures.",
        ),
        option("wallet", "wallet", "Coldkey wallet name.").short('w'),
        option(
            "wallet_hotkey",
            "wallet-hotkey",
            "Hotkey name within the wallet.",
        )
        .short('H'),
        option("wallet_path", "wallet-path", "Wallet directory."),
        flag("json", "json", "Machine-readable JSON output."),
        flag("quiet", "quiet", "Suppress informational output.").short('q'),
        Arg::new("verbosity")
            .long("verbose")
            .short('v')
            .action(ArgAction::Count)
            .help("Diagnostic logging on stderr: -v info, -vv debug, -vvv raw frames."),
    ]
}
fn unlock() -> Vec<Arg> {
    vec![
        option(
            "wallet_password_file",
            "wallet-password-file",
            "File containing the coldkey password.",
        ),
        flag(
            "macos_password",
            "macos-password",
            "Unlock via the native macOS password dialog.",
        ),
        flag(
            "keychain_password",
            "keychain-password",
            "Unlock from the macOS Keychain.",
        ),
    ]
}
fn bridge() -> Vec<Arg> {
    vec![
        option(
            "extension_browser",
            "extension-browser",
            "Browser for the extension bridge page.",
        ),
        option(
            "extension_bridge_url",
            "extension-bridge",
            "Extension bridge WebSocket URL.",
        ),
    ]
}
fn tx() -> Vec<Arg> {
    vec![
        flag("assume_yes", "yes", "Skip confirmation prompts.")
            .short('y')
            .alias("no-prompt"),
        flag("dry_run", "dry-run", "Preview without submitting."),
        flag("mev_shield", "mev-shield", "Encrypt via the MEV shield.")
            .overrides_with_all(["mev_shield", "no_mev_shield"]),
        flag("no_mev_shield", "no-mev-shield", "Submit in the clear.")
            .overrides_with_all(["mev_shield", "no_mev_shield"]),
        option(
            "proxy_for",
            "proxy-for",
            "Dispatch as this account through Proxy.proxy; 'self' signs directly.",
        ),
        option(
            "force_proxy_type",
            "force-proxy-type",
            "Require this proxy permission category.",
        )
        .value_parser(rtao_intents::proxy::PROXY_TYPES),
        option(
            "signatory",
            "signatory",
            "Multisig signing member; repeat or supply comma/space separated names.",
        )
        .action(ArgAction::Append),
        option(
            "signer_backend",
            "signer",
            "Signing backend: wallet, extension, ledger, or vault.",
        ),
        flag("ledger", "ledger", "Shorthand for --signer ledger."),
        Arg::new("ledger_account")
            .long("ledger-account")
            .value_parser(clap::value_parser!(u32)),
        Arg::new("ledger_index")
            .long("ledger-index")
            .value_parser(clap::value_parser!(u32)),
        option(
            "signer_address",
            "signer-address",
            "External signer ss58 or address-book name.",
        ),
        option(
            "extension_source",
            "extension-source",
            "Extension account source filter.",
        ),
    ]
}

/// Attach one tier, retaining any flags already owned by the command.
///
/// Ownership by argument identifier mirrors Python's signature-based skip;
/// raw `call` owns `signer_backend`, and generated fields may own proxy options.
pub fn with_globals(mut command: Command, tier: Tier) -> Command {
    let owned: Vec<_> = command
        .get_arguments()
        .map(|arg| arg.get_id().as_str().to_owned())
        .collect();
    let mut args = common();
    match tier {
        Tier::Read => {}
        Tier::Unlock => args.extend(unlock()),
        Tier::Extension => args.extend(bridge()),
        Tier::Tx => {
            args.extend(tx());
            args.extend(unlock());
            args.extend(bridge());
        }
    }
    for arg in args {
        if !owned.iter().any(|id| id == arg.get_id().as_str()) {
            // A default marks env-aware options as belonging to this leaf even
            // when absent. ArgMatches cannot otherwise reveal absent options
            // in release builds; read-only tiers must not apply tx envvars.
            let arg = if matches!(
                arg.get_id().as_str(),
                "signer_address" | "extension_browser" | "extension_bridge_url"
            ) {
                arg.default_value("")
            } else {
                arg
            };
            command = command.arg(arg);
        }
    }
    command
}

/// Attach Python's root defaults; per-command tiers supply the overrides.
pub fn root_globals(command: Command) -> Command {
    // Python root exposes read, flow and external backend flags; coldkey unlock,
    // proxy and derivation options belong to tx/unlock commands.
    let mut args = common();
    args.extend(tx().into_iter().filter(|arg| {
        matches!(
            arg.get_id().as_str(),
            "assume_yes"
                | "dry_run"
                | "mev_shield"
                | "no_mev_shield"
                | "signer_backend"
                | "signer_address"
                | "extension_source"
        )
    }));
    args.extend(bridge());
    command.args(args)
}

fn string(matches: &ArgMatches, id: &str) -> Option<String> {
    matches.try_get_one::<String>(id).ok().flatten().cloned()
}
fn yes(matches: &ArgMatches, id: &str) -> bool {
    matches
        .try_get_one::<bool>(id)
        .ok()
        .flatten()
        .copied()
        .unwrap_or(false)
}

/// Normalize a backend selector as Python's `str.strip().lower()` does.
/// Callers retain the original spelling for stored input and diagnostics.
pub(crate) fn normalize_signer_backend(value: &str) -> String {
    crate::prompt::python_strip(value).to_lowercase()
}

/// Python's `str` conversion for config defaults, preserving plain strings.
pub(crate) fn config_string(value: &Value) -> String {
    use rtao::primitives::pyfmt;
    let repr = |value: &Value| match value {
        Value::String(value) => pyfmt::string_repr(value),
        value => config_string(value),
    };
    match value {
        Value::String(value) => value.clone(),
        Value::Null => "None".into(),
        Value::Bool(true) => "True".into(),
        Value::Bool(false) => "False".into(),
        Value::Number(value) => {
            let raw = value.to_string();
            if raw.contains(['.', 'e', 'E']) {
                raw.parse::<f64>().map(pyfmt::float_repr).unwrap_or(raw)
            } else {
                raw
            }
        }
        Value::Array(values) => format!(
            "[{}]",
            values.iter().map(repr).collect::<Vec<_>>().join(", ")
        ),
        Value::Object(values) => format!(
            "{{{}}}",
            values
                .iter()
                .map(|(key, value)| format!("{}: {}", pyfmt::string_repr(key), repr(value)))
                .collect::<Vec<_>>()
                .join(", ")
        ),
    }
}
fn env_bool(value: &str) -> Result<bool, String> {
    match value.trim().to_ascii_lowercase().as_str() {
        "1" | "true" | "t" | "yes" | "y" | "on" => Ok(true),
        "0" | "false" | "f" | "no" | "n" | "off" | "" => Ok(false),
        _ => Err(format!("invalid boolean environment value {value:?}")),
    }
}
fn truthy(value: &Value) -> bool {
    match value {
        Value::Null => false,
        Value::Bool(v) => *v,
        Value::String(v) => !v.is_empty(),
        Value::Array(v) => !v.is_empty(),
        Value::Object(v) => !v.is_empty(),
        Value::Number(v) => v.as_f64() != Some(0.0),
    }
}

impl Globals {
    /// Resolve root flag > nonempty environment > config > builtin default.
    /// Signer validation follows leaf overrides in [`Self::apply_with_env`].
    pub fn resolve(
        root: &ArgMatches,
        config: &Map<String, Value>,
        home: &Path,
        env: impl Fn(&str) -> Option<String>,
    ) -> Result<Self, String> {
        let resolve = |id: &str, variable: Option<&str>, fallback: Option<String>| {
            string(root, id)
                .or_else(|| variable.and_then(&env).filter(|v| !v.is_empty()))
                .or_else(|| config.get(id).map(config_string))
                .or(fallback)
        };
        Ok(Self {
            network: resolve(
                "network",
                Some("BT_NETWORK"),
                Some(rtao_catalog::py::DEFAULT_NETWORK.into()),
            )
            .ok_or("network is missing")?,
            wallet_name: resolve("wallet", Some("BT_WALLET"), Some("default".into()))
                .ok_or("wallet is missing")?,
            hotkey_name: resolve(
                "wallet_hotkey",
                Some("BT_WALLET_HOTKEY"),
                Some("default".into()),
            )
            .ok_or("hotkey is missing")?,
            wallet_path: resolve(
                "wallet_path",
                Some("BT_WALLET_PATH"),
                Some(
                    home.join(".bittensor/wallets")
                        .to_string_lossy()
                        .into_owned(),
                ),
            )
            .ok_or("wallet path is missing")?
            .into(),
            wallet_given: string(root, "wallet").is_some()
                || env("BT_WALLET").is_some_and(|v| !v.is_empty()),
            hotkey_given: string(root, "wallet_hotkey").is_some()
                || env("BT_WALLET_HOTKEY").is_some_and(|v| !v.is_empty()),
            json: yes(root, "json") || config.get("json").is_some_and(truthy),
            quiet: yes(root, "quiet") || config.get("quiet").is_some_and(truthy),
            verbosity: root
                .try_get_one::<u8>("verbosity")
                .ok()
                .flatten()
                .copied()
                .unwrap_or(0),
            assume_yes: yes(root, "assume_yes"),
            dry_run: yes(root, "dry_run"),
            mev_shield: if yes(root, "mev_shield") {
                Some(true)
            } else if yes(root, "no_mev_shield") {
                Some(false)
            } else {
                None
            },
            proxy_for: None,
            force_proxy_type: None,
            signatories: Vec::new(),
            wallet_password_file: env("BT_WALLET_PASSWORD_FILE")
                .filter(|v| !v.is_empty())
                .map(PathBuf::from),
            macos_password: false,
            keychain_password: false,
            signer_backend: string(root, "signer_backend").filter(|backend| !backend.is_empty()),
            signer_address: string(root, "signer_address")
                .or_else(|| env("BT_SIGNER_ADDRESS").filter(|v| !v.is_empty())),
            ledger_account: 0,
            ledger_index: 0,
            extension_source: string(root, "extension_source"),
            extension_browser: resolve("extension_browser", Some("BT_EXTENSION_BROWSER"), None),
            extension_bridge_url: string(root, "extension_bridge_url")
                .or_else(|| env("BT_EXTENSION_BRIDGE").filter(|v| !v.is_empty())),
            fallback_endpoints: resolve("fallback_endpoints", Some("BT_FALLBACK_ENDPOINTS"), None),
            archive_endpoints: resolve("archive_endpoints", Some("BT_ARCHIVE_ENDPOINTS"), None),
            retry_forever: yes(root, "retry_forever")
                || match env("BT_RETRY_FOREVER").filter(|v| !v.is_empty()) {
                    Some(v) => env_bool(&v)?,
                    None => config.get("retry_forever").is_some_and(truthy),
                },
        })
    }

    /// Apply one command's explicitly supplied overrides, matching Python's
    /// truthy string/boolean behavior. Reserved identifiers stay command-owned.
    pub fn apply(&mut self, command: &ArgMatches, reserved: &[&str]) -> Result<(), String> {
        self.apply_with_env(command, reserved, |name| std::env::var(name).ok())
    }

    /// Apply leaf overrides with an injected environment. Leaf flag > leaf
    /// environment > resolved root settings; reserved identifiers stay owned
    /// by the command, including their environment variables.
    pub fn apply_with_env(
        &mut self,
        command: &ArgMatches,
        reserved: &[&str],
        env: impl Fn(&str) -> Option<String>,
    ) -> Result<(), String> {
        let value = |id: &str| {
            if reserved.contains(&id) {
                None
            } else {
                let value = string(command, id)?;
                if command.value_source(id) == Some(clap::parser::ValueSource::DefaultValue)
                    && let Some(variable) = match id {
                        "signer_address" => Some("BT_SIGNER_ADDRESS"),
                        "extension_browser" => Some("BT_EXTENSION_BROWSER"),
                        "extension_bridge_url" => Some("BT_EXTENSION_BRIDGE"),
                        _ => None,
                    }
                {
                    env(variable).filter(|v| !v.is_empty())
                } else {
                    (!value.is_empty()).then_some(value)
                }
            }
        };
        let enabled = |id: &str| !reserved.contains(&id) && yes(command, id);
        if let Some(v) = value("network") {
            self.network = v;
        }
        if let Some(v) = value("wallet") {
            self.wallet_name = v;
            self.wallet_given = true;
        }
        if let Some(v) = value("wallet_hotkey") {
            self.hotkey_name = v;
            self.hotkey_given = true;
        }
        if let Some(v) = value("wallet_path") {
            self.wallet_path = v.into();
        }
        if let Some(v) = value("fallback_endpoints") {
            self.fallback_endpoints = Some(v);
        }
        if let Some(v) = value("archive_endpoints") {
            self.archive_endpoints = Some(v);
        }
        self.retry_forever |= enabled("retry_forever");
        self.json |= enabled("json");
        self.quiet |= enabled("quiet");
        self.assume_yes |= enabled("assume_yes");
        self.dry_run |= enabled("dry_run");
        if enabled("mev_shield") {
            self.mev_shield = Some(true);
        } else if enabled("no_mev_shield") {
            self.mev_shield = Some(false);
        }
        if !reserved.contains(&"proxy_for")
            && let Some(v) = string(command, "proxy_for")
        {
            self.proxy_for = Some(v);
        }
        if !reserved.contains(&"force_proxy_type")
            && let Some(v) = string(command, "force_proxy_type")
        {
            self.force_proxy_type = Some(v);
        }
        if !reserved.contains(&"signatory")
            && let Ok(Some(values)) = command.try_get_many::<String>("signatory")
        {
            self.signatories = values
                .flat_map(|v| {
                    crate::books::split_signatory_refs(v)
                        .into_iter()
                        .map(str::to_owned)
                })
                .collect();
            if self.signatories.len() == 1
                && let Some((name, backend)) = self.signatories[0].clone().split_once('=')
            {
                let backend = normalize_signer_backend(backend);
                if !["wallet", "vault", "ledger", "extension"].contains(&backend.as_str()) {
                    return Err(format!(
                        "unknown backend {backend:?} in --signatory; use wallet, vault, ledger, or extension"
                    ));
                }
                self.signatories[0] = name.into();
                if backend != "wallet" {
                    self.signer_backend = Some(backend);
                }
            }
        }
        let verbosity = command
            .try_get_one::<u8>("verbosity")
            .ok()
            .flatten()
            .copied()
            .unwrap_or(0);
        if verbosity > 0 && !reserved.contains(&"verbosity") {
            self.verbosity = verbosity;
        }
        if let Some(v) = value("wallet_password_file") {
            self.wallet_password_file = Some(v.into());
        }
        self.macos_password |= enabled("macos_password");
        self.keychain_password |= enabled("keychain_password");
        if let Some(v) = value("signer_backend") {
            self.signer_backend = Some(v);
        }
        if enabled("ledger") {
            self.signer_backend = Some("ledger".into());
        }
        if !reserved.contains(&"ledger_account")
            && let Ok(Some(v)) = command.try_get_one::<u32>("ledger_account")
        {
            self.ledger_account = *v;
        }
        if !reserved.contains(&"ledger_index")
            && let Ok(Some(v)) = command.try_get_one::<u32>("ledger_index")
        {
            self.ledger_index = *v;
        }
        if let Some(v) = value("signer_address") {
            self.signer_address = Some(v);
        }
        if let Some(v) = value("extension_source") {
            self.extension_source = Some(v);
        }
        if let Some(v) = value("extension_browser") {
            self.extension_browser = Some(v);
        }
        if let Some(v) = value("extension_bridge_url") {
            self.extension_bridge_url = Some(v);
        }
        // A command owning --signer may choose an external backend after
        // globals are applied. Raw call's wallet/coldkey/hotkey spellings keep
        // the root backend; its external spellings override it. Validate that
        // final selection without taking ownership of the reserved argument.
        let owned_backend = reserved
            .contains(&"signer_backend")
            .then(|| string(command, "signer_backend"))
            .flatten()
            .filter(|backend| ["ledger", "vault", "extension"].contains(&backend.as_str()));
        self.validate_signer(owned_backend.as_deref())
    }

    fn validate_signer(&self, override_backend: Option<&str>) -> Result<(), String> {
        if let Some(backend) = override_backend.or(self.signer_backend.as_deref())
            && !["wallet", "ledger", "vault", "extension"]
                .contains(&normalize_signer_backend(backend).as_str())
        {
            return Err(format!(
                "unknown signing backend {backend:?}; use wallet, extension, ledger, or vault"
            ));
        }
        Ok(())
    }

    /// Whether the private key lives outside local wallet files.
    pub fn uses_external_signer(&self) -> bool {
        self.signer_backend.as_deref().is_some_and(|v| {
            ["ledger", "vault", "extension"].contains(&normalize_signer_backend(v).as_str())
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::application::command_tree;
    use serde_json::json;
    fn parse(argv: &[&str]) -> Result<ArgMatches, clap::Error> {
        command_tree([
            with_globals(Command::new("submit"), Tier::Tx),
            with_globals(Command::new("read"), Tier::Read),
        ])
        .try_get_matches_from(argv)
    }
    #[test]
    fn flag_env_config_default_precedence_and_wallet_provenance()
    -> Result<(), Box<dyn std::error::Error>> {
        let config = json!({"network":"local","wallet":"configured","wallet_hotkey":"config-hot","json":true,"quiet":false,"mev_shield":true});
        let config = config.as_object().ok_or("map")?;
        let parsed = parse(&[
            "rtao",
            "-n",
            "test",
            "submit",
            "-w",
            "member",
            "--no-mev-shield",
        ])?;
        let mut globals =
            Globals::resolve(
                &parsed,
                config,
                Path::new("/isolated/home"),
                |name| match name {
                    "BT_NETWORK" => Some("finney".into()),
                    "BT_WALLET" => Some("env-member".into()),
                    _ => None,
                },
            )?;
        assert_eq!(globals.network, "test");
        assert_eq!(globals.wallet_name, "env-member");
        assert!(globals.wallet_given);
        assert!(!globals.hotkey_given);
        assert!(globals.json);
        assert_eq!(globals.mev_shield, None);
        globals.apply_with_env(parsed.subcommand().ok_or("subcommand")?.1, &[], |_| None)?;
        assert_eq!(globals.wallet_name, "member");
        assert_eq!(globals.mev_shield, Some(false));
        let default = Globals::resolve(
            &parse(&["rtao", "read"])?,
            &Map::new(),
            Path::new("/injected"),
            |_| None,
        )?;
        assert_eq!(
            default.wallet_path,
            Path::new("/injected/.bittensor/wallets")
        );
        assert_eq!(default.network, "finney");
        Ok(())
    }
    #[test]
    fn command_tiers_and_owned_globals_are_functional() -> Result<(), Box<dyn std::error::Error>> {
        assert!(parse(&["rtao", "read", "--yes"]).is_err());
        assert!(parse(&["rtao", "read", "--keychain-password"]).is_err());
        let call = with_globals(
            Command::new("call").arg(option("signer_backend", "signer", "Owned role").alias("key")),
            Tier::Tx,
        );
        let parsed = command_tree([call])
            .try_get_matches_from(["rtao", "call", "--key", "hotkey", "--ledger"])?;
        let mut globals = Globals::resolve(&parsed, &Map::new(), Path::new("/injected"), |_| None)?;
        globals.apply_with_env(
            parsed.subcommand().ok_or("call")?.1,
            &["signer_backend"],
            |_| None,
        )?;
        assert_eq!(globals.signer_backend.as_deref(), Some("ledger"));
        Ok(())
    }
    #[test]
    fn signatory_order_backend_and_count_override_match_python()
    -> Result<(), Box<dyn std::error::Error>> {
        let parsed = parse(&[
            "rtao",
            "-vvv",
            "submit",
            "-v",
            "--signatory",
            "a,b c",
            "--signatory",
            "d",
            "--ledger-index",
            "0",
        ])?;
        let mut globals = Globals::resolve(&parsed, &Map::new(), Path::new("/injected"), |_| None)?;
        globals.apply_with_env(parsed.subcommand().ok_or("submit")?.1, &[], |_| None)?;
        assert_eq!(globals.signatories, ["a", "b", "c", "d"]);
        assert_eq!(globals.verbosity, 1);
        let parsed = parse(&[
            "rtao",
            "submit",
            "--signatory",
            "alice=VAULT",
            "--signer",
            "extension",
        ])?;
        globals.apply_with_env(parsed.subcommand().ok_or("submit")?.1, &[], |_| None)?;
        assert_eq!(globals.signatories, ["alice"]);
        assert_eq!(globals.signer_backend.as_deref(), Some("extension"));
        for (flags, expected) in [
            (["--mev-shield", "--no-mev-shield"], false),
            (["--no-mev-shield", "--mev-shield"], true),
        ] {
            let parsed = parse(&["rtao", "submit", flags[0], flags[1]])?;
            globals.apply_with_env(parsed.subcommand().ok_or("submit")?.1, &[], |_| None)?;
            assert_eq!(globals.mev_shield, Some(expected));
        }
        Ok(())
    }

    #[test]
    fn external_leaf_env_overrides_root_flags_only_for_its_tier()
    -> Result<(), Box<dyn std::error::Error>> {
        let environment = |name: &str| match name {
            "BT_SIGNER_ADDRESS" => Some("env-account".into()),
            "BT_EXTENSION_BROWSER" => Some("env-browser".into()),
            "BT_EXTENSION_BRIDGE" => Some("ws://env-bridge".into()),
            _ => None,
        };
        for leaf in ["submit", "read"] {
            let parsed = parse(&[
                "rtao",
                "--signer-address",
                "root-account",
                "--extension-browser",
                "root-browser",
                "--extension-bridge",
                "ws://root-bridge",
                leaf,
            ])?;
            let mut globals =
                Globals::resolve(&parsed, &Map::new(), Path::new("/injected"), environment)?;
            assert_eq!(globals.signer_address.as_deref(), Some("root-account"));
            globals.apply_with_env(parsed.subcommand().ok_or("leaf")?.1, &[], environment)?;
            let expected = if leaf == "submit" {
                ("env-account", "env-browser", "ws://env-bridge")
            } else {
                ("root-account", "root-browser", "ws://root-bridge")
            };
            assert_eq!(globals.signer_address.as_deref(), Some(expected.0));
            assert_eq!(globals.extension_browser.as_deref(), Some(expected.1));
            assert_eq!(globals.extension_bridge_url.as_deref(), Some(expected.2));
        }
        let parsed = parse(&[
            "rtao",
            "--signer-address",
            "root-account",
            "--extension-browser",
            "root-browser",
            "submit",
            "--signer-address",
            "leaf-account",
            "--extension-browser",
            "",
            "--extension-bridge",
            "ws://leaf-bridge",
        ])?;
        let mut globals =
            Globals::resolve(&parsed, &Map::new(), Path::new("/injected"), environment)?;
        globals.apply_with_env(parsed.subcommand().ok_or("leaf")?.1, &[], environment)?;
        assert_eq!(globals.signer_address.as_deref(), Some("leaf-account"));
        assert_eq!(globals.extension_browser.as_deref(), Some("root-browser"));
        assert_eq!(
            globals.extension_bridge_url.as_deref(),
            Some("ws://leaf-bridge")
        );
        let mut globals =
            Globals::resolve(&parsed, &Map::new(), Path::new("/injected"), environment)?;
        globals.apply_with_env(
            parsed.subcommand().ok_or("leaf")?.1,
            &[
                "signer_address",
                "extension_browser",
                "extension_bridge_url",
            ],
            environment,
        )?;
        assert_eq!(globals.signer_address.as_deref(), Some("root-account"));
        assert_eq!(globals.extension_browser.as_deref(), Some("root-browser"));
        assert_eq!(
            globals.extension_bridge_url.as_deref(),
            Some("ws://env-bridge")
        );
        Ok(())
    }

    #[test]
    fn configured_signer_address_does_not_acquire_flag_or_env_provenance()
    -> Result<(), Box<dyn std::error::Error>> {
        let config = json!({"signer_address": "config-account"});
        let config = config.as_object().ok_or("config")?;
        let parsed = parse(&["rtao", "submit"])?;
        let mut globals = Globals::resolve(&parsed, config, Path::new("/injected"), |_| None)?;
        globals.apply_with_env(parsed.subcommand().ok_or("submit")?.1, &[], |_| None)?;
        assert_eq!(globals.signer_address, None);
        assert_eq!(config.get("signer_address"), Some(&json!("config-account")));
        let globals = Globals::resolve(&parsed, config, Path::new("/injected"), |name| {
            (name == "BT_SIGNER_ADDRESS").then(|| "env-account".into())
        })?;
        assert_eq!(globals.signer_address.as_deref(), Some("env-account"));
        Ok(())
    }

    #[test]
    fn explicit_empty_proxy_target_and_python_signatory_whitespace_survive()
    -> Result<(), Box<dyn std::error::Error>> {
        let parsed = parse(&[
            "rtao",
            "submit",
            "--proxy-for",
            "",
            "--force-proxy-type",
            "Any",
            "--signatory",
            "a\u{1c}b\u{1d}c\u{1e}d\u{1f}e\u{85}f\u{a0}g,h",
        ])?;
        let mut globals = Globals::resolve(&parsed, &Map::new(), Path::new("/injected"), |_| None)?;
        globals.apply_with_env(parsed.subcommand().ok_or("submit")?.1, &[], |_| None)?;
        assert_eq!(globals.proxy_for.as_deref(), Some(""));
        assert_eq!(globals.force_proxy_type.as_deref(), Some("Any"));
        assert_eq!(
            globals.signatories,
            ["a", "b", "c", "d", "e", "f", "g", "h"]
        );
        let mut reserved =
            Globals::resolve(&parsed, &Map::new(), Path::new("/injected"), |_| None)?;
        reserved.apply_with_env(
            parsed.subcommand().ok_or("submit")?.1,
            &["proxy_for", "force_proxy_type", "signatory"],
            |_| None,
        )?;
        assert_eq!(reserved.proxy_for, None);
        assert_eq!(reserved.force_proxy_type, None);
        assert!(reserved.signatories.is_empty());
        Ok(())
    }

    #[test]
    fn config_strings_use_python_str_and_nested_repr() -> Result<(), Box<dyn std::error::Error>> {
        for (raw, expected) in [
            ("1e3", "1000.0"),
            ("1e-7", "1e-07"),
            ("-0.0", "-0.0"),
            (
                "123456789012345678901234567890",
                "123456789012345678901234567890",
            ),
            (
                r#"[true,null,1e3,"a'b", "line\nnext"]"#,
                "[True, None, 1000.0, \"a'b\", 'line\\nnext']",
            ),
            (r#"{"key":[false,"value"]}"#, "{'key': [False, 'value']}"),
        ] {
            assert_eq!(config_string(&serde_json::from_str(raw)?), expected);
        }
        assert_eq!(config_string(&json!("plain\nstring")), "plain\nstring");
        let parsed = parse(&["rtao", "read"])?;
        let config = json!({"network": ["test", true]});
        let globals = Globals::resolve(
            &parsed,
            config.as_object().ok_or("config")?,
            Path::new("/injected"),
            |_| None,
        )?;
        assert_eq!(globals.network, "['test', True]");
        Ok(())
    }

    #[test]
    fn final_generated_signer_override_wins_and_invalid_final_choices_are_rejected()
    -> Result<(), Box<dyn std::error::Error>> {
        let command = command_tree([Command::new("tx").subcommand(with_globals(
            Command::new("transfer").arg(
                Arg::new("amount")
                    .long("amount")
                    .required(true)
                    .value_parser(clap::value_parser!(u64)),
            ),
            Tier::Tx,
        ))]);
        for backend in ["wallet", "vault", "ledger", "extension"] {
            let parsed = command.clone().try_get_matches_from([
                "rtao", "--signer", "typo", "tx", "transfer", "--amount", "1", "--signer", backend,
            ])?;
            let mut globals =
                Globals::resolve(&parsed, &Map::new(), Path::new("/injected"), |_| None)?;
            assert_eq!(globals.signer_backend.as_deref(), Some("typo"));
            let leaf = parsed
                .subcommand()
                .ok_or("tx")?
                .1
                .subcommand()
                .ok_or("transfer")?
                .1;
            globals.apply_with_env(leaf, &[], |_| None)?;
            assert_eq!(globals.signer_backend.as_deref(), Some(backend));
            assert_eq!(globals.uses_external_signer(), backend != "wallet");
        }
        for argv in [
            vec![
                "rtao", "--signer", "typo", "tx", "transfer", "--amount", "1",
            ],
            vec![
                "rtao", "--signer", "wallet", "tx", "transfer", "--amount", "1", "--signer", "typo",
            ],
            vec![
                "rtao", "--signer", "typo", "tx", "transfer", "--amount", "1", "--signer", "",
            ],
        ] {
            let parsed = command.clone().try_get_matches_from(argv)?;
            let mut globals =
                Globals::resolve(&parsed, &Map::new(), Path::new("/injected"), |_| None)?;
            let leaf = parsed
                .subcommand()
                .ok_or("tx")?
                .1
                .subcommand()
                .ok_or("transfer")?
                .1;
            assert!(globals.apply_with_env(leaf, &[], |_| None).is_err());
        }
        let parsed = command
            .try_get_matches_from(["rtao", "--signer", "", "tx", "transfer", "--amount", "1"])?;
        let mut globals = Globals::resolve(&parsed, &Map::new(), Path::new("/injected"), |_| None)?;
        let leaf = parsed
            .subcommand()
            .ok_or("tx")?
            .1
            .subcommand()
            .ok_or("transfer")?
            .1;
        globals.apply_with_env(leaf, &[], |_| None)?;
        assert_eq!(globals.signer_backend, None);
        assert!(!globals.uses_external_signer());
        Ok(())
    }

    #[test]
    fn raw_owned_external_selector_overrides_invalid_root_without_global_ownership()
    -> Result<(), Box<dyn std::error::Error>> {
        let command = command_tree([crate::call::command()]);
        for backend in ["vault", "ledger", "extension"] {
            let parsed = command.clone().try_get_matches_from([
                "rtao",
                "--signer",
                "typo",
                "call",
                "System.remark",
                "--signer",
                backend,
            ])?;
            let mut globals =
                Globals::resolve(&parsed, &Map::new(), Path::new("/injected"), |_| None)?;
            let leaf = parsed.subcommand().ok_or("call")?.1;
            globals.apply_with_env(leaf, &["signer_backend"], |_| None)?;
            assert_eq!(globals.signer_backend.as_deref(), Some("typo"));
            assert_eq!(
                leaf.get_one::<String>("signer_backend").map(String::as_str),
                Some(backend)
            );
        }
        for role in ["wallet", "coldkey", "hotkey"] {
            let parsed = command.clone().try_get_matches_from([
                "rtao",
                "--signer",
                "typo",
                "call",
                "System.remark",
                "--signer",
                role,
            ])?;
            let mut globals =
                Globals::resolve(&parsed, &Map::new(), Path::new("/injected"), |_| None)?;
            assert!(
                globals
                    .apply_with_env(
                        parsed.subcommand().ok_or("call")?.1,
                        &["signer_backend"],
                        |_| None
                    )
                    .is_err()
            );
        }
        let parsed = command.clone().try_get_matches_from([
            "rtao",
            "--signer",
            "",
            "call",
            "System.remark",
        ])?;
        let mut globals = Globals::resolve(&parsed, &Map::new(), Path::new("/injected"), |_| None)?;
        globals.apply_with_env(
            parsed.subcommand().ok_or("call")?.1,
            &["signer_backend"],
            |_| None,
        )?;
        assert_eq!(globals.signer_backend, None);
        assert!(!globals.uses_external_signer());
        assert!(
            command
                .try_get_matches_from(["rtao", "call", "System.remark", "--signer", "typo"])
                .is_err()
        );
        Ok(())
    }
}
