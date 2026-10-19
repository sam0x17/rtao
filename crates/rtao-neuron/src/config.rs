//! Explicit neuron configuration with flag, environment, and btcli precedence.

use clap::{ArgMatches, ValueEnum, parser::ValueSource};
use rtao_client::config::{ConfigData, Paths, Store};
use rtao_primitives::NetUid;
use rtao_wallet::WalletRef;
use serde::Deserialize;
use serde_json::Value;
use std::{
    collections::BTreeMap,
    ffi::OsString,
    net::{IpAddr, SocketAddr},
    path::{Path, PathBuf},
    str::FromStr,
};
use url::Url;

/// Logging severity selected for the neuron's tracing subscriber.
#[derive(Clone, Copy, Debug, Deserialize, ValueEnum, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum LogLevel {
    /// Disable logging.
    Off,
    /// Report errors only.
    Error,
    /// Report warnings and errors.
    Warn,
    /// Report operational events, warnings, and errors.
    Info,
    /// Include diagnostic details.
    Debug,
    /// Include all tracing events.
    Trace,
}

/// Shared neuron command arguments and resolved runtime configuration.
///
/// A binary can flatten this `clap::Args` value into its own parser. To include
/// btcli configuration, pass the parser's matches to [`Self::resolve_matches`].
/// This preserves the source of each argument so clap defaults cannot mask a
/// stored value. Libraries never inspect process arguments.
#[derive(Clone, Debug, clap::Args, Deserialize, PartialEq)]
pub struct NeuronConfig {
    /// Named network or an explicit websocket endpoint.
    #[arg(
        short = 'n',
        long,
        env = "BT_NETWORK",
        alias = "subtensor.network",
        default_value = "finney"
    )]
    pub network: String,
    /// Override the named network's primary websocket endpoint.
    #[arg(long, env = "BT_CHAIN_ENDPOINT", alias = "subtensor.chain_endpoint")]
    pub chain_endpoint: Option<Url>,
    /// Wallet directory name, also accepting `wallet/hotkey` shorthand.
    #[arg(
        short = 'w',
        long,
        env = "BT_WALLET",
        alias = "wallet.name",
        default_value = "default"
    )]
    pub wallet: String,
    /// Hotkey file name in the selected wallet.
    #[arg(
        short = 'H',
        long = "wallet-hotkey",
        env = "BT_WALLET_HOTKEY",
        alias = "wallet.hotkey",
        default_value = "default"
    )]
    pub hotkey: String,
    /// Wallet root; absent means `~/.bittensor/wallets`.
    #[arg(long = "wallet-path", env = "BT_WALLET_PATH", alias = "wallet.path")]
    pub wallet_path: Option<PathBuf>,
    /// Subnet identity supplied by the binary or [`Self::load_for`].
    #[arg(long, value_parser = parse_netuid)]
    pub netuid: NetUid,
    /// Explicit fallback pool; the single value `none` pins the primary endpoint.
    #[arg(long, env = "BT_FALLBACK_ENDPOINTS", value_delimiter = ',')]
    pub fallback_endpoints: Option<Vec<String>>,
    /// Explicit archive pool, accepting the single value `none` to disable it.
    #[arg(long, env = "BT_ARCHIVE_ENDPOINTS", value_delimiter = ',')]
    pub archive_endpoints: Option<Vec<String>>,
    /// Separate ws(s) endpoint for nonces, fees, submissions and their
    /// receipts, for example a full node while reads use a pruned, archive or
    /// indexing endpoint. Absent sends everything to the network endpoint.
    #[arg(long = "submit-endpoint", env = "BT_SUBMIT_ENDPOINT")]
    #[serde(default)]
    pub submit_endpoint: Option<String>,
    /// Continue reconnecting after the ordinary retry budget is exhausted.
    #[arg(long, env = "BT_RETRY_FOREVER", action = clap::ArgAction::Set, num_args = 0..=1, default_missing_value = "true", default_value = "false", value_parser = parse_bool)]
    pub retry_forever: bool,
    /// Wait for finalized registration before starting neuron services.
    #[arg(long = "wait-for-registration", default_value_t = false)]
    #[serde(default)]
    pub wait_for_registration: bool,
    /// Axon listening port.
    #[arg(long = "axon.port", default_value_t = 8091)]
    pub axon_port: u16,
    /// Public axon address supplied by the operator.
    #[arg(long = "axon.external-ip", env = "BT_AXON_EXTERNAL_IP")]
    pub axon_external_ip: Option<IpAddr>,
    /// Allow publishing a loopback or private axon address, for a miner and
    /// validator on one host or LAN. Always allowed with `--network local`.
    #[arg(long = "axon.allow-private", env = "BT_AXON_ALLOW_PRIVATE", action = clap::ArgAction::Set, num_args = 0..=1, default_missing_value = "true", default_value = "false", value_parser = parse_bool)]
    #[serde(default)]
    pub axon_allow_private: bool,
    /// Override the default neuron state directory.
    #[arg(long, env = "RTAO_STATE_DIR")]
    pub state_dir: Option<PathBuf>,
    /// Optional metrics and health listener; absent disables the listener.
    #[arg(long, env = "RTAO_METRICS_ADDR")]
    pub metrics_addr: Option<SocketAddr>,
    /// Logging severity; the runtime applies its default when this is absent.
    #[arg(long, env = "BITTENSOR_LOG", alias = "logging.level", value_enum)]
    pub log: Option<LogLevel>,
}

/// A missing mandatory identity or invalid configuration layer.
#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    /// A library call has no explicit subnet identity.
    #[error(
        "netuid is required: use NeuronConfig::load_for(netuid), or pass --netuid and call NeuronConfig::resolve_matches"
    )]
    MissingNetuid,
    /// The user's home directory cannot be resolved.
    #[error("cannot resolve neuron configuration without a home directory")]
    MissingHome,
    /// A selected configuration value is invalid.
    #[error("invalid neuron configuration for {key}: {message}")]
    Invalid {
        /// Flag, environment variable, or btcli key involved.
        key: String,
        /// Why the selected value is invalid.
        message: String,
    },
    /// The shared btcli store could not be read.
    #[error(transparent)]
    Shared(#[from] rtao_client::config::ConfigError),
}

impl NeuronConfig {
    /// Load environment and shared btcli configuration without reading argv.
    ///
    /// Subnet identity is mandatory and has no implicit environment or file
    /// default. Use [`Self::load_for`] to provide it, or [`Self::resolve_matches`]
    /// to supply an application's parsed arguments.
    pub async fn load() -> Result<Self, ConfigError> {
        Err(ConfigError::MissingNetuid)
    }

    /// Load environment, shared btcli configuration, and defaults for a subnet.
    pub async fn load_for(netuid: NetUid) -> Result<Self, ConfigError> {
        Self::load_layers(None, Some(netuid)).await
    }

    /// Resolve explicit application flags before environment, btcli, and defaults.
    ///
    /// The caller parses argv. This method uses only the supplied matches and
    /// the current environment; filesystem reads run on the blocking pool.
    pub async fn resolve_matches(matches: &ArgMatches) -> Result<Self, ConfigError> {
        Self::load_layers(Some(matches), None).await
    }

    async fn load_layers(
        matches: Option<&ArgMatches>,
        netuid: Option<NetUid>,
    ) -> Result<Self, ConfigError> {
        #[allow(deprecated)]
        let home = std::env::home_dir()
            .filter(|path| !path.as_os_str().is_empty())
            .ok_or(ConfigError::MissingHome)?;
        let environment = std::env::vars_os()
            .filter_map(|(key, value)| key.into_string().ok().map(|key| (key, value)))
            .collect::<BTreeMap<_, _>>();
        Self::load_with(&home, &environment, matches, netuid).await
    }

    /// Read the shared btcli file with injected home, environment, and flags.
    ///
    /// This honors `BTCLI_CONFIG` and reuses the shared store's corrupt-file
    /// recovery. Reads and any corrupt-file preservation run off the reactor.
    pub async fn load_with(
        home: &Path,
        environment: &BTreeMap<String, OsString>,
        matches: Option<&ArgMatches>,
        netuid: Option<NetUid>,
    ) -> Result<Self, ConfigError> {
        let paths = Paths::new(home, |key| environment.get(key).cloned());
        let stored = Store::new(paths).load().await?;
        Self::resolve_layers(matches, environment, &stored, home, netuid)
    }

    /// Resolve injected layers without filesystem access or process arguments.
    ///
    /// Only `CommandLine` values in `matches` count as flags. Environment values
    /// come from the supplied map, so this method supports deterministic tests.
    /// The explicit subnet argument is used when `matches` has no `--netuid`.
    pub fn resolve_layers(
        matches: Option<&ArgMatches>,
        environment: &BTreeMap<String, OsString>,
        stored: &ConfigData,
        home: &Path,
        netuid: Option<NetUid>,
    ) -> Result<Self, ConfigError> {
        let netuid = flag::<NetUid>(matches, "netuid")
            .or(netuid)
            .ok_or(ConfigError::MissingNetuid)?;
        NetUid::new(netuid.get()).map_err(|error| invalid("netuid", error))?;
        let network = string_layer(
            matches,
            environment,
            stored,
            "network",
            "BT_NETWORK",
            "network",
            "finney",
        )?;
        let wallet = string_layer(
            matches,
            environment,
            stored,
            "wallet",
            "BT_WALLET",
            "wallet",
            "default",
        )?;
        let hotkey = string_layer(
            matches,
            environment,
            stored,
            "hotkey",
            "BT_WALLET_HOTKEY",
            "wallet_hotkey",
            "default",
        )?;
        let wallet_path = path_layer(
            matches,
            environment,
            stored,
            "wallet_path",
            "BT_WALLET_PATH",
            Some("wallet_path"),
            home,
        )?;
        // Normalize shorthand once so state and wallet paths use the same names.
        let reference = WalletRef::new(
            &wallet,
            &hotkey,
            wallet_path
                .clone()
                .unwrap_or_else(|| home.join(".bittensor/wallets")),
        )
        .map_err(|error| invalid("wallet", error))?;
        let chain_endpoint =
            parsed_layer(matches, environment, "chain_endpoint", "BT_CHAIN_ENDPOINT")?;
        if let Some(url) = &chain_endpoint {
            let url: &Url = url;
            if !matches!(url.scheme(), "ws" | "wss") || url.host_str().is_none() {
                return Err(invalid(
                    "chain_endpoint",
                    "expected a ws:// or wss:// endpoint with a host",
                ));
            }
        } else {
            rtao_client::Network::resolve_with(&network, |key| {
                environment
                    .get(key)
                    .and_then(|value| value.to_str())
                    .map(str::to_owned)
            })
            .map_err(|error| invalid("network", error))?;
        }
        let submit_endpoint: Option<String> = parsed_layer(
            matches,
            environment,
            "submit_endpoint",
            "BT_SUBMIT_ENDPOINT",
        )?;
        if let Some(endpoint) = &submit_endpoint {
            let valid = Url::parse(endpoint)
                .is_ok_and(|url| matches!(url.scheme(), "ws" | "wss") && url.host_str().is_some());
            if !valid {
                return Err(invalid(
                    "submit_endpoint",
                    "expected a ws:// or wss:// endpoint with a host",
                ));
            }
        }
        let axon_allow_private = match flag::<bool>(matches, "axon_allow_private") {
            Some(value) => value,
            None => environment_string(environment, "BT_AXON_ALLOW_PRIVATE")?
                .map(|value| {
                    parse_bool(&value).map_err(|error| invalid("BT_AXON_ALLOW_PRIVATE", error))
                })
                .transpose()?
                .unwrap_or(false),
        };
        let retry_forever = if let Some(value) = flag::<bool>(matches, "retry_forever") {
            value
        } else if let Some(value) = environment_string(environment, "BT_RETRY_FOREVER")? {
            parse_bool(&value).map_err(|error| invalid("BT_RETRY_FOREVER", error))?
        } else if let Some(value) = stored.get("retry_forever").filter(|value| !value.is_null()) {
            value
                .as_bool()
                .ok_or_else(|| invalid("retry_forever", "btcli value must be a boolean"))?
        } else {
            false
        };
        Ok(Self {
            network,
            chain_endpoint,
            wallet: reference.name().into(),
            hotkey: reference.hotkey_name().into(),
            wallet_path,
            netuid,
            fallback_endpoints: endpoints_layer(
                matches,
                environment,
                stored,
                "fallback_endpoints",
                "BT_FALLBACK_ENDPOINTS",
            )?,
            archive_endpoints: endpoints_layer(
                matches,
                environment,
                stored,
                "archive_endpoints",
                "BT_ARCHIVE_ENDPOINTS",
            )?,
            submit_endpoint,
            retry_forever,
            wait_for_registration: flag::<bool>(matches, "wait_for_registration").unwrap_or(false),
            axon_port: flag::<u16>(matches, "axon_port").unwrap_or(8091),
            axon_external_ip: parsed_layer(
                matches,
                environment,
                "axon_external_ip",
                "BT_AXON_EXTERNAL_IP",
            )?,
            axon_allow_private,
            state_dir: path_layer(
                matches,
                environment,
                stored,
                "state_dir",
                "RTAO_STATE_DIR",
                None,
                home,
            )?,
            metrics_addr: parsed_layer(matches, environment, "metrics_addr", "RTAO_METRICS_ADDR")?,
            log: if let Some(value) = flag::<LogLevel>(matches, "log") {
                Some(value)
            } else {
                environment_string(environment, "BITTENSOR_LOG")?
                    .map(|value| {
                        <LogLevel as ValueEnum>::from_str(&value, true)
                            .map_err(|error| invalid("BITTENSOR_LOG", error))
                    })
                    .transpose()?
            },
        })
    }

    /// Resolve the public wallet layout without reading or unlocking a key.
    pub fn wallet_reference(&self) -> Result<WalletRef, ConfigError> {
        let path = self
            .wallet_path
            .clone()
            .unwrap_or_else(|| PathBuf::from("~/.bittensor/wallets"));
        WalletRef::new(&self.wallet, &self.hotkey, path).map_err(|error| invalid("wallet", error))
    }

    /// Resolve this neuron's default persistent state directory.
    ///
    /// `name` identifies the application, for example `validator`. An explicit
    /// state directory overrides the entire default location.
    pub fn state_directory(&self, home: &Path, name: &str) -> Result<PathBuf, ConfigError> {
        if name.is_empty()
            || name == "."
            || name == ".."
            || Path::new(name).components().count() != 1
            || name.contains(std::path::MAIN_SEPARATOR)
        {
            return Err(invalid("name", "expected one non-empty path component"));
        }
        // Validate public fields again because callers may construct or mutate
        // configuration directly without using the resolution methods.
        let wallet = WalletRef::new(&self.wallet, &self.hotkey, home.join(".bittensor/wallets"))
            .map_err(|error| invalid("wallet", error))?;
        Ok(self
            .state_dir
            .clone()
            .map(|path| expand_home(path, home))
            .unwrap_or_else(|| {
                home.join(".bittensor/miners")
                    .join(wallet.name())
                    .join(wallet.hotkey_name())
                    .join(format!("netuid{}", self.netuid.get()))
                    .join(name)
            }))
    }
}

fn flag<T: Clone + Send + Sync + 'static>(matches: Option<&ArgMatches>, id: &str) -> Option<T> {
    let matches = matches?;
    if matches.try_contains_id(id).ok()?
        && matches.value_source(id) == Some(ValueSource::CommandLine)
    {
        matches.try_get_one::<T>(id).ok().flatten().cloned()
    } else {
        None
    }
}
fn flags(matches: Option<&ArgMatches>, id: &str) -> Option<Vec<String>> {
    let matches = matches?;
    if matches.try_contains_id(id).ok()?
        && matches.value_source(id) == Some(ValueSource::CommandLine)
    {
        matches
            .try_get_many::<String>(id)
            .ok()
            .flatten()
            .map(|values| values.cloned().collect())
    } else {
        None
    }
}
fn invalid(key: &str, message: impl std::fmt::Display) -> ConfigError {
    ConfigError::Invalid {
        key: key.into(),
        message: message.to_string(),
    }
}
fn environment_string(
    environment: &BTreeMap<String, OsString>,
    key: &str,
) -> Result<Option<String>, ConfigError> {
    environment
        .get(key)
        .filter(|value| !value.is_empty())
        .map(|value| {
            value
                .to_str()
                .map(str::to_owned)
                .ok_or_else(|| invalid(key, "value must be valid Unicode"))
        })
        .transpose()
}
fn string_layer(
    matches: Option<&ArgMatches>,
    environment: &BTreeMap<String, OsString>,
    stored: &ConfigData,
    id: &str,
    env: &str,
    key: &str,
    default: &str,
) -> Result<String, ConfigError> {
    let value = if let Some(value) = flag::<String>(matches, id) {
        value
    } else if let Some(value) = environment_string(environment, env)? {
        value
    } else if let Some(value) = stored.get(key).filter(|value| !value.is_null()) {
        value
            .as_str()
            .ok_or_else(|| invalid(key, "btcli value must be a string"))?
            .into()
    } else {
        default.into()
    };
    if value.is_empty() {
        return Err(invalid(key, "value must not be empty"));
    }
    Ok(value)
}
fn parsed_layer<T: Clone + Send + Sync + FromStr + 'static>(
    matches: Option<&ArgMatches>,
    environment: &BTreeMap<String, OsString>,
    id: &str,
    env: &str,
) -> Result<Option<T>, ConfigError>
where
    T::Err: std::fmt::Display,
{
    if let Some(value) = flag::<T>(matches, id) {
        return Ok(Some(value));
    }
    environment_string(environment, env)?
        .map(|value| value.parse().map_err(|error| invalid(env, error)))
        .transpose()
}
fn path_layer(
    matches: Option<&ArgMatches>,
    environment: &BTreeMap<String, OsString>,
    stored: &ConfigData,
    id: &str,
    env: &str,
    key: Option<&str>,
    home: &Path,
) -> Result<Option<PathBuf>, ConfigError> {
    let path = if let Some(value) = flag::<PathBuf>(matches, id) {
        Some(value)
    } else if let Some(value) = environment.get(env).filter(|value| !value.is_empty()) {
        Some(PathBuf::from(value))
    } else if let Some(key) = key {
        stored
            .get(key)
            .filter(|value| !value.is_null())
            .map(|value| {
                value
                    .as_str()
                    .map(PathBuf::from)
                    .ok_or_else(|| invalid(key, "btcli value must be a string path"))
            })
            .transpose()?
    } else {
        None
    };
    if path
        .as_ref()
        .is_some_and(|value| value.as_os_str().is_empty())
    {
        return Err(invalid(id, "path must not be empty"));
    }
    Ok(path.map(|path| expand_home(path, home)))
}
fn expand_home(path: PathBuf, home: &Path) -> PathBuf {
    path.strip_prefix("~")
        .map_or_else(|_| path.clone(), |suffix| home.join(suffix))
}
fn endpoints_layer(
    matches: Option<&ArgMatches>,
    environment: &BTreeMap<String, OsString>,
    stored: &ConfigData,
    id: &str,
    env: &str,
) -> Result<Option<Vec<String>>, ConfigError> {
    let values = if let Some(values) = flags(matches, id) {
        Some(values)
    } else if let Some(value) = environment_string(environment, env)? {
        Some(
            value
                .split(',')
                .map(|value| value.trim().to_owned())
                .collect(),
        )
    } else if let Some(value) = stored.get(id).filter(|value| !value.is_null()) {
        Some(match value {
            Value::Array(values) => values
                .iter()
                .map(|value| {
                    value
                        .as_str()
                        .map(str::to_owned)
                        .ok_or_else(|| invalid(id, "btcli endpoints must be strings"))
                })
                .collect::<Result<_, _>>()?,
            Value::String(value) => value
                .split(',')
                .map(|value| value.trim().to_owned())
                .collect(),
            _ => {
                return Err(invalid(
                    id,
                    "btcli value must be an endpoint list or comma-separated string",
                ));
            }
        })
    } else {
        None
    };
    if let Some(values) = &values {
        if values.len() == 1 && values[0].eq_ignore_ascii_case("none") {
            return Ok(Some(vec!["none".into()]));
        }
        for value in values {
            let endpoint: Url = value.parse().map_err(|error| invalid(id, error))?;
            if !matches!(endpoint.scheme(), "ws" | "wss") || endpoint.host_str().is_none() {
                return Err(invalid(
                    id,
                    "expected websocket endpoints or the single value 'none'",
                ));
            }
        }
    }
    Ok(values)
}
fn parse_netuid(value: &str) -> Result<NetUid, String> {
    NetUid::new(value.parse::<u16>().map_err(|error| error.to_string())?)
        .map_err(|error| error.to_string())
}
fn parse_bool(value: &str) -> Result<bool, String> {
    match value.trim().to_ascii_lowercase().as_str() {
        "true" | "1" | "yes" | "on" => Ok(true),
        "false" | "0" | "no" | "off" => Ok(false),
        _ => Err("expected true/false, 1/0, yes/no, or on/off".into()),
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::expect_used)]
    use super::*;
    use clap::{Args, Command};

    fn matches(arguments: &[&str]) -> ArgMatches {
        NeuronConfig::augment_args(Command::new("validator"))
            .try_get_matches_from(arguments)
            .expect("parse flags")
    }
    fn stored() -> ConfigData {
        serde_json::json!({"network":"test", "wallet":"stored", "wallet_hotkey":"stored-hot", "wallet_path":"~/stored-wallets", "fallback_endpoints":["wss://stored.example"], "retry_forever":true}).as_object().expect("config object").clone()
    }
    #[test]
    fn explicit_flags_override_environment_and_shared_btcli() {
        let flags = matches(&[
            "validator",
            "--netuid",
            "3",
            "--network",
            "local",
            "--wallet",
            "flag",
            "--wallet-hotkey",
            "flag-hot",
            "--retry-forever=false",
            "--fallback-endpoints",
            "none",
        ]);
        let env = BTreeMap::from([
            ("BT_NETWORK".into(), OsString::from("test")),
            ("BT_WALLET".into(), OsString::from("env")),
            ("BT_WALLET_HOTKEY".into(), OsString::from("env-hot")),
            ("BT_RETRY_FOREVER".into(), OsString::from("true")),
        ]);
        let cfg = NeuronConfig::resolve_layers(
            Some(&flags),
            &env,
            &stored(),
            Path::new("/isolated/home"),
            None,
        )
        .expect("resolve");
        assert_eq!(
            (
                cfg.network.as_str(),
                cfg.wallet.as_str(),
                cfg.hotkey.as_str(),
                cfg.netuid
            ),
            ("local", "flag", "flag-hot", NetUid(3))
        );
        assert!(!cfg.retry_forever);
        assert_eq!(cfg.fallback_endpoints, Some(vec!["none".into()]));
        assert_eq!(
            cfg.wallet_path,
            Some(PathBuf::from("/isolated/home/stored-wallets"))
        );
    }
    #[test]
    fn clap_defaults_do_not_mask_persisted_values_and_environment_wins() {
        let flags = matches(&["validator", "--netuid", "3"]);
        let cfg = NeuronConfig::resolve_layers(
            Some(&flags),
            &BTreeMap::new(),
            &stored(),
            Path::new("/isolated/home"),
            None,
        )
        .expect("stored");
        assert_eq!(
            (
                cfg.network.as_str(),
                cfg.wallet.as_str(),
                cfg.hotkey.as_str()
            ),
            ("test", "stored", "stored-hot")
        );
        assert!(cfg.retry_forever);
        let env = BTreeMap::from([
            ("BT_NETWORK".into(), OsString::from("local")),
            ("BT_WALLET".into(), OsString::from("env")),
            ("BT_RETRY_FOREVER".into(), OsString::from("0")),
        ]);
        let cfg = NeuronConfig::resolve_layers(
            Some(&flags),
            &env,
            &stored(),
            Path::new("/isolated/home"),
            None,
        )
        .expect("environment");
        assert_eq!(
            (cfg.network.as_str(), cfg.wallet.as_str()),
            ("local", "env")
        );
        assert!(!cfg.retry_forever);
    }
    #[test]
    fn dotted_aliases_paths_and_framework_environment_are_resolved() {
        let flags = matches(&[
            "validator",
            "--netuid",
            "7",
            "--subtensor.network",
            "local",
            "--wallet.name",
            "cold/hot",
            "--wallet.path",
            "~/wallets",
            "--logging.level",
            "debug",
        ]);
        let env = BTreeMap::from([
            ("RTAO_STATE_DIR".into(), OsString::from("~/state")),
            ("RTAO_METRICS_ADDR".into(), OsString::from("127.0.0.1:9090")),
            ("BT_AXON_EXTERNAL_IP".into(), OsString::from("::1")),
        ]);
        let cfg = NeuronConfig::resolve_layers(
            Some(&flags),
            &env,
            &ConfigData::new(),
            Path::new("/isolated/home"),
            None,
        )
        .expect("aliases");
        assert_eq!((cfg.wallet.as_str(), cfg.hotkey.as_str()), ("cold", "hot"));
        assert_eq!(
            cfg.wallet_reference().expect("public wallet").hotkey_path(),
            Path::new("/isolated/home/wallets/cold/hotkeys/hot")
        );
        assert_eq!(
            cfg.state_directory(Path::new("/isolated/home"), "validator")
                .expect("state override"),
            Path::new("/isolated/home/state")
        );
        assert_eq!(
            cfg.metrics_addr,
            Some("127.0.0.1:9090".parse().expect("socket"))
        );
        assert_eq!(cfg.axon_external_ip, Some("::1".parse().expect("ip")));
        assert_eq!(cfg.log, Some(LogLevel::Debug));
    }
    #[test]
    fn defaults_require_explicit_netuid_and_cannot_escape_default_state_layout() {
        let cfg = NeuronConfig::resolve_layers(
            None,
            &BTreeMap::new(),
            &ConfigData::new(),
            Path::new("/isolated/home"),
            Some(NetUid(1)),
        )
        .expect("defaults");
        assert_eq!(cfg.network, "finney");
        assert_eq!(cfg.axon_port, 8091);
        assert_eq!(
            cfg.state_directory(Path::new("/isolated/home"), "validator")
                .expect("default state"),
            Path::new("/isolated/home/.bittensor/miners/default/default/netuid1/validator")
        );
        assert!(
            cfg.state_directory(Path::new("/isolated/home"), "../wallets")
                .is_err()
        );
        assert!(matches!(
            NeuronConfig::resolve_layers(
                None,
                &BTreeMap::new(),
                &ConfigData::new(),
                Path::new("/isolated/home"),
                None
            ),
            Err(ConfigError::MissingNetuid)
        ));
        assert!(
            NeuronConfig::resolve_layers(
                None,
                &BTreeMap::new(),
                &ConfigData::new(),
                Path::new("/isolated/home"),
                Some(NetUid(NetUid::LIMIT))
            )
            .is_err()
        );
    }
    #[test]
    fn submit_endpoint_comes_from_flag_or_environment_and_reaches_the_client() {
        let resolve = |flags: &[&str], env: &[(&str, &str)]| {
            let env = env
                .iter()
                .map(|(key, value)| ((*key).to_owned(), OsString::from(*value)))
                .collect::<BTreeMap<_, _>>();
            NeuronConfig::resolve_layers(
                Some(&matches(flags)),
                &env,
                &ConfigData::new(),
                Path::new("/isolated/home"),
                None,
            )
            .expect("resolve")
        };
        let absent = resolve(&["validator", "--netuid", "1", "--network", "local"], &[]);
        assert_eq!(absent.submit_endpoint, None);
        #[cfg(any(feature = "validator", feature = "miner"))]
        assert!(
            crate::client_config(&absent)
                .expect("client config")
                .submit
                .is_none()
        );
        let env = resolve(
            &["validator", "--netuid", "1", "--network", "local"],
            &[("BT_SUBMIT_ENDPOINT", "ws://127.0.0.1:9955")],
        );
        assert_eq!(env.submit_endpoint.as_deref(), Some("ws://127.0.0.1:9955"));
        let flag = resolve(
            &[
                "validator",
                "--netuid",
                "1",
                "--network",
                "local",
                "--submit-endpoint",
                "wss://node.example:443",
            ],
            &[("BT_SUBMIT_ENDPOINT", "ws://127.0.0.1:9955")],
        );
        assert_eq!(
            flag.submit_endpoint.as_deref(),
            Some("wss://node.example:443")
        );
        #[cfg(any(feature = "validator", feature = "miner"))]
        {
            let chain = crate::client_config(&flag).expect("client config");
            let submit = chain.submit.expect("submit session");
            assert_eq!(submit.pool.urls.len(), 1);
            assert_eq!(submit.pool.urls[0].host_str(), Some("node.example"));
            // Reads keep the network's own endpoint.
            assert_ne!(chain.session.pool.urls, submit.pool.urls);
        }
    }
    #[test]
    fn malformed_selected_values_fail_without_consulting_lower_precedence_layers() {
        for (key, value) in [
            ("BT_NETWORK", "unknown"),
            ("BT_CHAIN_ENDPOINT", "https://example.com"),
            ("BT_RETRY_FOREVER", "sometimes"),
            ("RTAO_METRICS_ADDR", "anywhere"),
            ("BT_FALLBACK_ENDPOINTS", "none,wss://example.com"),
            ("BT_SUBMIT_ENDPOINT", "https://example.com"),
            ("BT_SUBMIT_ENDPOINT", "finney"),
        ] {
            let env = BTreeMap::from([(key.into(), OsString::from(value))]);
            assert!(
                NeuronConfig::resolve_layers(
                    None,
                    &env,
                    &stored(),
                    Path::new("/isolated/home"),
                    Some(NetUid(1))
                )
                .is_err(),
                "{key}"
            );
        }
        assert!(
            NeuronConfig::augment_args(Command::new("validator"))
                .try_get_matches_from(["validator", "--netuid", "4096"])
                .is_err()
        );
    }

    #[tokio::test]
    async fn shared_store_honors_btcli_config_override_before_resolving_layers() {
        use std::sync::atomic::{AtomicU64, Ordering};
        static NEXT_FIXTURE: AtomicU64 = AtomicU64::new(0);
        struct Fixture(PathBuf);
        impl Drop for Fixture {
            fn drop(&mut self) {
                // All store workers have been awaited; this exact directory
                // contains only the files written by this test.
                std::fs::remove_dir_all(&self.0).expect("remove config fixture");
            }
        }
        let root = std::env::var_os("RTAO_TEMP_ROOT")
            .map(PathBuf::from)
            .unwrap_or_else(|| {
                PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/neuron-tests")
            })
            .join("tmp")
            .join(format!(
                "neuron-config-{}-{}",
                std::process::id(),
                NEXT_FIXTURE.fetch_add(1, Ordering::Relaxed)
            ));
        std::fs::create_dir_all(&root).expect("isolated config fixture");
        let fixture = Fixture(root);
        let override_path = fixture.0.join("operator.json");
        std::fs::write(&override_path, br#"{"network":"test","wallet":"shared","wallet_hotkey":"validator","retry_forever":true}"#).expect("shared btcli file");
        let environment = BTreeMap::from([
            ("BTCLI_CONFIG".into(), override_path.into_os_string()),
            ("BT_WALLET".into(), OsString::from("environment")),
        ]);
        let cfg = NeuronConfig::load_with(&fixture.0, &environment, None, Some(NetUid(4)))
            .await
            .expect("shared store load");
        assert_eq!(
            (
                cfg.network.as_str(),
                cfg.wallet.as_str(),
                cfg.hotkey.as_str()
            ),
            ("test", "environment", "validator")
        );
        assert!(cfg.retry_forever);
    }
}
