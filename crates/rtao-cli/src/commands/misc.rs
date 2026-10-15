// Derived from RaoFoundation/subtensor sdk/python/bittensor/cli/commands/misc.py @ 850f2755bb05a3851f948489f0ab27b0645cebce (Apache-2.0); modified by rtao.
//! Offline denomination conversion and concurrent existing-endpoint latency probes.

use crate::{
    context::{AppContext, ConnectionFactory, NetworkConnector},
    error::CliError,
    globals::{Tier, with_globals},
    output::TableSpec,
};
use clap::{Arg, ArgAction, ArgMatches, Command};
use futures::future::join_all;
use rtao::{
    client::{BlockHeader, ClientConfig, Network},
    primitives::{DynBalance, NetUid, pyfmt},
    signer::BoxFuture,
};
use serde_json::{Value, json};
use std::time::{Duration, Instant};

/// Construct the two direct misc helpers; callers append the specialised groups.
pub fn command() -> Command {
    Command::new("misc")
        .about("Helpers and less-used command groups.")
        .subcommand_required(true)
        .arg_required_else_help(true)
        .subcommand(with_globals(
            Command::new("convert")
                .about("Convert between a display amount and native rao.")
                .arg(
                    Arg::new("amount")
                        .required(true)
                        .allow_hyphen_values(true)
                        .help("Display amount, or a rao value with --rao."),
                )
                .arg(
                    Arg::new("from_rao")
                        .long("rao")
                        .action(ArgAction::SetTrue)
                        .help("Treat the amount as rao and output its display amount."),
                )
                .arg(
                    Arg::new("netuid")
                        .long("netuid")
                        .default_value("0")
                        .value_parser(clap::value_parser!(u16).range(0..4096))
                        .help("Denomination subnet: 0 is TAO; otherwise subnet alpha."),
                ),
            Tier::Read,
        ))
        .subcommand(with_globals(
            Command::new("latency")
                .about("Measure websocket connection latency to existing network endpoints.")
                .arg(
                    Arg::new("networks")
                        .long("networks")
                        .default_value("")
                        .help("Comma-separated network presets or ws(s) URLs; defaults to all presets."),
                ),
            Tier::Read,
        ))
}

/// Command-owned identifiers excluded from global wallet alias resolution.
pub fn reserved(name: &str) -> Vec<&'static str> {
    match name {
        "convert" => vec!["amount", "from_rao", "netuid"],
        "latency" => vec!["networks"],
        _ => vec![],
    }
}

fn failure(error: impl std::fmt::Display) -> CliError {
    CliError::failure(error.to_string())
}

fn input(error: impl std::fmt::Display) -> CliError {
    CliError::input(error.to_string())
}

struct Conversion {
    amount: f64,
    from_rao: bool,
    balance: DynBalance,
}

impl Conversion {
    fn parse(matches: &ArgMatches) -> Result<Self, CliError> {
        let text = matches
            .get_one::<String>("amount")
            .ok_or_else(|| CliError::input("missing amount"))?;
        // Typer's pinned argument is a float. Preserve that coercion and its
        // shortest decimal representation before applying the Q25 native gate.
        let amount = pyfmt::parse_float(text)
            .filter(|amount| amount.is_finite())
            .ok_or_else(|| CliError::input("amount must be a finite number"))?;
        if amount < 0.0 {
            return Err(CliError::input("amount cannot be negative"));
        }
        let netuid = NetUid::new(
            *matches
                .get_one::<u16>("netuid")
                .ok_or_else(|| CliError::input("missing --netuid"))?,
        )
        .map_err(input)?;
        let from_rao = matches.get_flag("from_rao");
        let balance = if from_rao {
            // int(float) truncates toward zero. Check before casting because
            // Rust's float-to-int cast would silently saturate at u64::MAX.
            if amount >= 18_446_744_073_709_551_616.0 {
                return Err(CliError::input("rao amount exceeds the native u64 range"));
            }
            DynBalance::from_rao(amount.trunc() as u64, netuid)
        } else {
            // D-035 deliberately rejects sub-rao input instead of implicitly
            // rounding it, as required by the user's native-balance decision.
            DynBalance::from_amount(pyfmt::float_repr(amount), netuid).map_err(input)?
        };
        Ok(Self {
            amount,
            from_rao,
            balance,
        })
    }

    fn value(&self, ctx: &AppContext) -> Value {
        let balance = ctx.output.balance(self.balance.rao, self.balance.netuid).0;
        let currency = if balance.netuid == NetUid::ROOT {
            "TAO".to_owned()
        } else {
            format!("alpha (netuid {})", balance.netuid.get())
        };
        json!({
            "amount": if self.from_rao { balance.amount() } else { self.amount },
            "currency": currency,
            "rao": balance.rao,
            "display": balance.to_python_string(),
        })
    }
}

/// Resolve selected networks without touching sockets, caches or host settings.
/// An empty comma-separated selection expands to all presets in sorted order.
pub fn targets_with_env(
    matches: &ArgMatches,
    env: impl Fn(&str) -> Option<String>,
) -> Result<Vec<Network>, CliError> {
    let selected = matches
        .get_one::<String>("networks")
        .map_or("", String::as_str);
    let mut names = selected
        .split(',')
        .map(crate::prompt::python_strip)
        .filter(|name| !name.is_empty())
        .collect::<Vec<_>>();
    if names.is_empty() {
        names = rtao_catalog::py::NETWORKS
            .iter()
            .map(|(name, _)| *name)
            .collect();
        names.sort_unstable();
    }
    names
        .into_iter()
        .map(|name| Network::resolve_with(name, &env).map_err(input))
        .collect()
}

/// Validate native amounts and all endpoint names before any connection begins.
pub fn validate_before_connect(name: &str, matches: &ArgMatches) -> Result<(), CliError> {
    match name {
        "convert" => Conversion::parse(matches).map(|_| ()),
        "latency" => targets_with_env(matches, |name| std::env::var(name).ok()).map(|_| ()),
        _ => Err(CliError::input("unknown misc helper")),
    }
}

/// Duration and outcome of one connection plus a current-block query.
pub struct LatencyMeasurement {
    /// Time spent opening the client, querying a block and closing it.
    pub elapsed: Duration,
    /// A failed probe is a result row rather than a command failure.
    pub outcome: Result<(), CliError>,
}

/// Injectable latency boundary; fixtures never open network sockets.
pub trait LatencyBackend: Send + Sync {
    /// Probe an already resolved endpoint and release any opened connection.
    fn probe<'a>(&'a self, network: &'a Network) -> BoxFuture<'a, LatencyMeasurement>;
}

/// Existing-chain implementation, using the ordinary injectable connector.
pub struct NetworkLatencyBackend<'a> {
    /// WebSocket connector in production; socket-free client factory in fixtures.
    pub connector: &'a dyn ConnectionFactory,
}

impl NetworkLatencyBackend<'_> {
    /// Probe with explicit per-phase bounds. The ordinary CLI caps each phase
    /// at 30 seconds; callers may set shorter budgets for their diagnostics.
    pub async fn probe_with_timeouts(
        &self,
        network: &Network,
        connection_timeout: Duration,
        query_timeout: Duration,
    ) -> LatencyMeasurement {
        let started = Instant::now();
        let outcome = async {
            let mut config = ClientConfig::new(network.clone()).map_err(failure)?;
            // Endpoint diagnosis does not maintain head subscriptions or
            // write runtime/display caches on the user's machine.
            config.track_heads = false;
            config.runtime_cache_dir = None;
            config.config_store = None;
            let client = tokio::time::timeout(connection_timeout, self.connector.connect(config))
                .await
                .map_err(|_| {
                    CliError::failure(format!(
                        "connection deadline expired after {}s",
                        pyfmt::fmt_g(connection_timeout.as_secs_f64())
                    ))
                })??;
            let queried = tokio::time::timeout(query_timeout, async {
                let params =
                    serde_json::value::RawValue::from_string("[]".into()).map_err(failure)?;
                let raw = client
                    .rpc()
                    .request_raw("chain_getHeader", Some(params))
                    .await
                    .map_err(failure)?;
                let value = serde_json::from_str(raw.get()).map_err(failure)?;
                BlockHeader::from_json(value).map_err(failure)?;
                Ok::<(), CliError>(())
            })
            .await
            .map_err(|_| {
                CliError::failure(format!(
                    "block query deadline expired after {}s",
                    pyfmt::fmt_g(query_timeout.as_secs_f64())
                ))
            })
            .and_then(|result| result);
            client.close().await;
            queried
        }
        .await;
        LatencyMeasurement {
            elapsed: started.elapsed(),
            outcome,
        }
    }
}

impl LatencyBackend for NetworkLatencyBackend<'_> {
    fn probe<'a>(&'a self, network: &'a Network) -> BoxFuture<'a, LatencyMeasurement> {
        Box::pin(self.probe_with_timeouts(
            network,
            Duration::from_secs(30),
            Duration::from_secs(30),
        ))
    }
}

struct ProbeResult {
    network: String,
    endpoint: String,
    latency_ms: f64,
    error: Option<String>,
}

impl ProbeResult {
    fn value(&self) -> Value {
        let mut result = json!({
            "network": self.network,
            "endpoint": self.endpoint,
            "latency_ms": self.latency_ms,
            "ok": self.error.is_none(),
        });
        if let Some(error) = &self.error {
            result["error"] = json!(error);
        }
        result
    }
}

fn millis_text(value: f64) -> String {
    let text = format!("{value:.1}");
    let (integer, fraction) = text.split_once('.').unwrap_or((&text, "0"));
    let mut out = String::new();
    for (index, character) in integer.chars().enumerate() {
        if index > 0 && (integer.len() - index).is_multiple_of(3) {
            out.push(',');
        }
        out.push(character);
    }
    out.push('.');
    out.push_str(fraction);
    out
}

/// Run misc helpers without an application-wide chain connection or key access.
pub async fn run(ctx: &AppContext, name: &str, matches: &ArgMatches) -> Result<(), CliError> {
    run_with_backend(
        ctx,
        name,
        matches,
        &NetworkLatencyBackend {
            connector: &NetworkConnector,
        },
    )
    .await
}

/// Execute using a socket-free probe in fixtures; conversion never calls it.
pub async fn run_with_backend(
    ctx: &AppContext,
    name: &str,
    matches: &ArgMatches,
    backend: &dyn LatencyBackend,
) -> Result<(), CliError> {
    match name {
        "convert" => {
            let conversion = Conversion::parse(matches)?;
            let value = conversion.value(ctx);
            let order = if conversion.from_rao {
                ["rao", "amount", "currency", "display"]
            } else {
                ["amount", "currency", "rao", "display"]
            };
            let fields = order
                .iter()
                .map(|name| ((*name).to_owned(), value[*name].clone()))
                .collect::<Vec<_>>();
            ctx.output
                .detail(None, &fields, Some(&value))
                .map_err(failure)
        }
        "latency" => {
            let targets = targets_with_env(matches, |name| std::env::var(name).ok())?;
            // Resolve the entire selection first: a later typo must not start
            // an earlier connection or leave partial result output.
            let mut results = join_all(targets.iter().map(|network| async move {
                let measurement = backend.probe(network).await;
                // Formatting rounds the exact binary float to one decimal,
                // matching Python round(x, 1) without multiplication rounding.
                let latency_ms = format!("{:.1}", measurement.elapsed.as_secs_f64() * 1000.0)
                    .parse::<f64>()
                    .map_err(failure)?;
                Ok::<_, CliError>(ProbeResult {
                    network: network.label().to_owned(),
                    endpoint: network.endpoint().to_owned(),
                    latency_ms,
                    error: measurement.outcome.err().map(|error| error.to_string()),
                })
            }))
            .await
            .into_iter()
            .collect::<Result<Vec<_>, _>>()?;
            // Stable sorting retains caller order when rounded times tie.
            results.sort_by(|a, b| {
                a.error
                    .is_some()
                    .cmp(&b.error.is_some())
                    .then_with(|| a.latency_ms.total_cmp(&b.latency_ms))
            });
            let rows = results
                .iter()
                .map(|result| {
                    vec![
                        json!(result.network),
                        json!(if result.error.is_none() {
                            millis_text(result.latency_ms)
                        } else {
                            "—".into()
                        }),
                        json!(result.endpoint),
                        json!(match &result.error {
                            None => "✓".into(),
                            Some(error) if error.is_empty() => "✗ unreachable".into(),
                            Some(error) => format!("✗ {error}"),
                        }),
                    ]
                })
                .collect();
            let mut table = TableSpec::new(
                "latency (websocket connect + one block query)",
                ["network", "ms", "endpoint", "status"]
                    .map(str::to_owned)
                    .into(),
                rows,
            );
            table.records = Some(Value::Array(
                results.iter().map(ProbeResult::value).collect(),
            ));
            table.right_align.insert(1);
            let reachable = results
                .iter()
                .filter(|result| result.error.is_none())
                .count();
            table.footer = Some(
                match results.first().filter(|result| result.error.is_none()) {
                    Some(fastest) => format!(
                        "fastest {}  {} ms · {reachable}/{} reachable",
                        fastest.network,
                        millis_text(fastest.latency_ms),
                        results.len(),
                    ),
                    None => format!("0/{} reachable", results.len()),
                },
            );
            ctx.output.columns(&table).map_err(failure)
        }
        _ => Err(CliError::input("unknown misc helper")),
    }
}
