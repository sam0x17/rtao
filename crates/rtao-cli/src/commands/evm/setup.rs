// Derived from RaoFoundation/subtensor sdk/python/bittensor/cli/commands/evm/setup.py @ 850f2755bb05a3851f948489f0ab27b0645cebce (Apache-2.0); modified by rtao.
//! EVM network configuration, endpoint diagnosis and existing-node sudo setup.

use super::{failure, optional};
use crate::{
    context::{AppContext, ExternalSignerFactory, SubmissionOptions},
    error::CliError,
    globals::{Tier, with_globals},
    output::{DiagnosticContext, HumanView, ReviewSection, TableSpec},
    prompt::PromptHistory,
};
use clap::{Arg, ArgMatches, Command};
use rtao::{
    client::{Client, Inclusion, View},
    evm::{self, EvmKeyInfo, EvmNetwork, EvmRpc, U256},
    signer::{BoxFuture, Coldkey},
    tx::{BuildCx, Built, ExtrinsicResult, Intent, IntentError, IntentInterface, Root},
};
use serde_json::{Value, json};

/// Build the four setup and diagnosis commands.
pub fn commands() -> Vec<Command> {
    let endpoint = || {
        Arg::new("rpc_url")
            .long("rpc-url")
            .help("Override the EVM HTTP(S) RPC endpoint.")
    };
    vec![
        with_globals(Command::new("networks").about("EVM connectivity per network: chain ID, RPC URL, currency."), Tier::Read),
        with_globals(Command::new("config").about("Ready-to-paste EVM tool configuration for the configured network.")
            .arg(Arg::new("format").long("format").default_value("metamask").help("Tool to emit config for: metamask, hardhat, or remix.")), Tier::Read),
        with_globals(Command::new("doctor").about("Probe the EVM endpoint: reachability, chain ID, gas price, key balance.")
            .arg(Arg::new("evm_key").long("evm-key").help("EVM key name or WALLET/NAME. Also checks its balance and nonce."))
            .arg(endpoint()), Tier::Read),
        with_globals(Command::new("setup-localnet").about("Set the chain ID and disable the deployment whitelist on an existing node.")
            .long_about("Submit two Sudo.sudo extrinsics, signed by the configured wallet coldkey, to an explicitly selected existing node. This client command does not start or manage a node.")
            .arg(Arg::new("chain_id").long("chain-id").default_value("945").value_parser(clap::value_parser!(u64)).help("EVM chain ID to set (945 mimics testnet, 964 mainnet)."))
            .arg(endpoint()), Tier::Tx),
    ]
}

/// Command-owned argument names excluded from shared global resolution.
pub fn reserved(name: &str) -> Vec<&'static str> {
    match name {
        "config" => vec!["format"],
        "doctor" => vec!["evm_key", "rpc_url"],
        "setup-localnet" => vec!["chain_id", "rpc_url"],
        _ => Vec::new(),
    }
}

/// A single fixed, inspectable native operation from the Python setup command.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SetupStep {
    /// Set the EVM chain ID using the native sudo key.
    ChainId {
        /// Desired EVM chain ID.
        chain_id: u64,
        /// Observed previous ID, absent when the node reports an RPC error.
        current: Option<u64>,
    },
    /// Remove the deployment whitelist using the native sudo key.
    DisableWhitelist,
}

// This private adapter describes the authenticated CLI operation; it is not
// inserted into the generated Python intent catalog. Root origin makes the
// shared executor apply Sudo.sudo before its ordinary Proxy/multisig handling.
static SETUP_SPEC: rtao_catalog::IntentSpec = rtao_catalog::IntentSpec {
    name: "evm_setup_localnet",
    summary: "Prepare an existing local EVM endpoint",
    description: "One of the two native sudo calls from evm setup-localnet.",
    python_class: "cli.commands.evm.setup.setup_localnet",
    signer: "coldkey",
    origin: "root",
    verify: None,
    wraps: &[
        ("AdminUtils", "sudo_set_evm_chain_id"),
        ("EVM", "disable_whitelist"),
    ],
    fields: &[],
    all_amount_fields: &[],
    mev_shield_default: false,
    mev_shield_required: false,
    catalog_json: "{}",
    class_vars_json: "{}",
};

impl IntentInterface for SetupStep {
    const SPEC: &'static rtao_catalog::IntentSpec = &SETUP_SPEC;
    type Role = Coldkey;
    type Origin = Root;
    type Output = ();
    const WRAPS: &'static [&'static rtao_runtime::CallDescriptor] = &[
        &rtao_runtime::calls::admin_utils::SUDO_SET_EVM_CHAIN_ID,
        &rtao_runtime::calls::evm::DISABLE_WHITELIST,
    ];
    fn to_json(&self) -> Value {
        match self {
            Self::ChainId { chain_id, .. } => json!({"op":SETUP_SPEC.name,"chain_id":chain_id}),
            Self::DisableWhitelist => json!({"op":SETUP_SPEC.name,"disabled":true}),
        }
    }
}
impl Intent for SetupStep {
    fn summary(&self) -> String {
        match self {
            Self::ChainId { chain_id, current } => format!(
                "set EVM chain ID to {chain_id}{}",
                current
                    .filter(|id| *id != 0)
                    .map(|id| format!(" (currently {id})"))
                    .unwrap_or_default()
            ),
            Self::DisableWhitelist => "disable the contract deployment whitelist".into(),
        }
    }
    async fn build<V: View + ?Sized>(&self, cx: &BuildCx<'_, V>) -> Result<Built, IntentError> {
        let call = match self {
            Self::ChainId { chain_id, .. } => {
                rtao_runtime::calls::admin_utils::sudo_set_evm_chain_id(*chain_id)
            }
            Self::DisableWhitelist => rtao_runtime::calls::evm::disable_whitelist(true),
        };
        Ok(Built::new(call.compose(cx.runtime)?))
    }
}

/// Native submission controls, shared with ordinary semantic commands.
pub struct SetupExecution<'a> {
    /// The explicit native connection; read and dry-run paths need none.
    pub client: Option<&'a Client>,
    /// Includes durable hooks and caller-selected period/finality options.
    pub options: SubmissionOptions<'a>,
    /// Optional injected external native signer factory.
    pub external: Option<&'a dyn ExternalSignerFactory>,
    /// Ordinary public wallet confirmation history.
    pub history: &'a mut PromptHistory,
}

/// Inject endpoint/public-file operations for fixtures without sockets or keys.
pub trait SetupBackend: Send + Sync {
    /// Perform one Ethereum JSON-RPC request. Submissions never use this path.
    fn request<'a>(
        &'a self,
        method: &'a str,
        params: &'a [Value],
    ) -> BoxFuture<'a, evm::Result<Value>>;
    /// List only public EVM key metadata.
    fn stored_keys<'a>(
        &'a self,
        ctx: &'a AppContext,
    ) -> BoxFuture<'a, Result<Vec<EvmKeyInfo>, CliError>>;
    /// Resolve only public metadata for a selected key.
    fn key_info<'a>(
        &'a self,
        ctx: &'a AppContext,
        key: Option<&'a str>,
    ) -> BoxFuture<'a, Result<EvmKeyInfo, CliError>>;
    /// Submit using shared review, policy, signer and durable-hook machinery.
    /// The default implementation never acquires a signer until that machinery
    /// has reviewed the call; fixtures may replace the transport boundary.
    fn submit<'a>(
        &'a self,
        ctx: &'a mut AppContext,
        step: &'a SetupStep,
        execution: SetupExecution<'a>,
    ) -> BoxFuture<'a, Result<Option<ExtrinsicResult>, CliError>> {
        Box::pin(async move {
            let client = execution
                .client
                .ok_or_else(|| failure("setup-localnet requires an explicit native client"))?;
            ctx.submit_with_options(
                client,
                step,
                SubmissionOptions {
                    bind_reviewed_call: true,
                    replan_owner_rate_limit: Some(false),
                    // Setup has no proof-refresh or custom execution strategy.
                    execution: None,
                    ..execution.options
                },
                execution.external,
                execution.history,
            )
            .await
        })
    }
}

struct NativeBackend(EvmRpc);
impl SetupBackend for NativeBackend {
    fn request<'a>(
        &'a self,
        method: &'a str,
        params: &'a [Value],
    ) -> BoxFuture<'a, evm::Result<Value>> {
        Box::pin(self.0.call(method, params))
    }
    fn stored_keys<'a>(
        &'a self,
        ctx: &'a AppContext,
    ) -> BoxFuture<'a, Result<Vec<EvmKeyInfo>, CliError>> {
        let wallet = ctx.globals.wallet_name.clone();
        let path = ctx.globals.wallet_path.clone();
        Box::pin(async move {
            rtao::core::offload(move || evm::list_evm_keys(&wallet, &path))
                .await
                .map_err(failure)?
                .map_err(failure)
        })
    }
    fn key_info<'a>(
        &'a self,
        ctx: &'a AppContext,
        key: Option<&'a str>,
    ) -> BoxFuture<'a, Result<EvmKeyInfo, CliError>> {
        Box::pin(super::key_info(ctx, key))
    }
}

/// Dispatch a setup-family route. This starts no node, service or daemon.
pub async fn run(
    ctx: &mut AppContext,
    name: &str,
    matches: &ArgMatches,
    client: Option<&Client>,
    options: SubmissionOptions<'_>,
    external: Option<&dyn ExternalSignerFactory>,
    history: &mut PromptHistory,
) -> Result<(), CliError> {
    match name {
        "networks" => return networks(ctx),
        "config" => {
            return configuration(
                ctx,
                matches,
                &evm::evm_network(&ctx.globals.network).map_err(failure)?,
            );
        }
        "setup-localnet" => refuse_public_network(ctx)?,
        "doctor" => {}
        _ => {
            return Err(CliError::input(format!(
                "unknown EVM setup command {name:?}"
            )));
        }
    }
    let (network, rpc) = super::rpc(ctx, optional(matches, "rpc_url")).await?;
    run_with(
        ctx,
        name,
        matches,
        &network,
        &NativeBackend(rpc),
        SetupExecution {
            client,
            options,
            external,
            history,
        },
    )
    .await
}

/// Execute against an injected endpoint, public-key reader and native boundary.
pub async fn run_with(
    ctx: &mut AppContext,
    name: &str,
    matches: &ArgMatches,
    network: &EvmNetwork,
    backend: &dyn SetupBackend,
    execution: SetupExecution<'_>,
) -> Result<(), CliError> {
    match name {
        "networks" => networks(ctx),
        "config" => configuration(ctx, matches, network),
        "doctor" => doctor(ctx, matches, network, backend).await,
        "setup-localnet" => setup(ctx, matches, backend, execution).await,
        _ => Err(CliError::input(format!(
            "unknown EVM setup command {name:?}"
        ))),
    }
}

fn networks(ctx: &AppContext) -> Result<(), CliError> {
    let presets = evm::evm_networks();
    // Python dict order is finney, test, local; the library uses a BTreeMap.
    let records = ["finney", "test", "local"]
        .into_iter()
        .filter_map(|name| presets.get(name));
    let mut rows = Vec::new();
    let mut machine = Vec::new();
    for network in records {
        rows.push(vec![
            json!(network.name),
            json!(
                network
                    .chain_id
                    .filter(|id| *id != 0)
                    .map(|id| id.to_string())
                    .unwrap_or_else(|| "42 at genesis (setup-localnet: 945)".into())
            ),
            json!(network.rpc_url),
            json!(network.currency_symbol),
        ]);
        machine.push(json!({"network":network.name,"chain_id":network.chain_id,"rpc_url":network.rpc_url,"currency":network.currency_symbol}));
    }
    let mut table = TableSpec::new(
        "Bittensor EVM networks",
        ["network", "chain id", "rpc url", "currency"]
            .map(str::to_owned)
            .to_vec(),
        rows,
    );
    table.records = Some(json!(machine));
    ctx.output.table(&table).map_err(failure)?;
    ctx.output.message("note: EVM values use 18 decimals (1 TAO = 1e18) while native TAO uses 9 (1 TAO = 1e9 rao) — MetaMask balances are the same funds, different exponent").map_err(failure)
}

fn configuration(
    ctx: &AppContext,
    matches: &ArgMatches,
    network: &EvmNetwork,
) -> Result<(), CliError> {
    match optional(matches, "format").unwrap_or("metamask") {
        "metamask" => {
            let display_name = if network.name == "finney" {
                "Subtensor".into()
            } else {
                format!("Subtensor {}", network.name)
            };
            ctx.output.detail(Some("MetaMask network settings (Add network > Add manually)"), &[
                ("Network name".into(), json!(display_name)),
                ("RPC URL".into(), json!(network.rpc_url)),
                ("Chain ID".into(), network.chain_id.filter(|id| *id != 0).map(|id| json!(id)).unwrap_or_else(|| json!("42 at genesis; run `btcli evm doctor` for the live value (`setup-localnet` sets 945)"))),
                ("Currency symbol".into(), json!(network.currency_symbol)),
            ], Some(&json!({"network_name":display_name,"rpc_url":network.rpc_url,"chain_id":network.chain_id,"currency_symbol":network.currency_symbol}))).map_err(failure)?;
            ctx.output.message("MetaMask assumes 18 decimals; displayed amounts are the true balance, just with 1e18 per TAO instead of the native 1e9 rao").map_err(failure)
        }
        "hardhat" => {
            let snippet = hardhat(&network.rpc_url);
            // Only the static template supplies line boundaries. Its endpoint
            // is a JSON string literal, so embedded controls remain escaped.
            let lines = snippet.lines().map(str::to_owned).collect::<Vec<_>>();
            let mut output = ctx.output.clone();
            output
                .registry_mut()
                .register("evm-hardhat-config", move |_| {
                    Ok(HumanView::Lines(lines.clone()))
                });
            output
                .render("evm-hardhat-config", &json!(snippet))
                .map_err(failure)
        }
        "remix" => ctx
            .output
            .detail(
                Some("Remix IDE settings for subtensor EVM"),
                &[
                    ("Compiler".into(), json!("0.8.24 (or lower)")),
                    ("EVM version".into(), json!("cancun")),
                    (
                        "Environment".into(),
                        json!("Injected Provider - MetaMask (connected to the network above)"),
                    ),
                    ("RPC URL".into(), json!(network.rpc_url)),
                ],
                None,
            )
            .map_err(failure),
        value => Err(CliError::input(format!(
            "unknown format {}",
            rtao::primitives::pyfmt::string_repr(value)
        ))
        .with_help("expected one of: metamask, hardhat, remix")),
    }
}

fn hardhat(url: &str) -> String {
    // JSON string quoting prevents an explicit endpoint from injecting JS.
    let quoted = json!(url).to_string();
    format!(
        "// hardhat.config.js — subtensor EVM\n\
// Deployer key, either of:\n\
//   export ETH_PRIVATE_KEY=$(btcli evm key export --private-key)\n\
//   export ETH_KEYSTORE=./key.json ETH_KEYSTORE_PASSWORD=…   (btcli evm key export --out key.json; no raw key leaves the file)\n\
const {{ Wallet }} = require(\"ethers\");\n\
const fs = require(\"fs\");\n\n\
const deployerKey =\n  process.env.ETH_PRIVATE_KEY ??\n  (process.env.ETH_KEYSTORE\n    ? Wallet.fromEncryptedJsonSync(\n        fs.readFileSync(process.env.ETH_KEYSTORE, \"utf8\"),\n        process.env.ETH_KEYSTORE_PASSWORD\n      ).privateKey\n    : undefined);\n\n\
module.exports = {{\n  solidity: {{ version: \"0.8.24\", settings: {{ evmVersion: \"cancun\" }} }},\n  networks: {{\n    subtensor: {{\n      url: {quoted},\n      accounts: deployerKey ? [deployerKey] : [],\n    }},\n  }},\n  mocha: {{ timeout: 300000 }}, // ~12s blocks: default timeouts are too short\n}};"
    )
}

async fn quantity(backend: &dyn SetupBackend, method: &str, params: &[Value]) -> evm::Result<U256> {
    let value = backend.request(method, params).await?;
    let text = value
        .as_str()
        .and_then(|raw| raw.strip_prefix("0x"))
        .filter(|raw| {
            !raw.is_empty()
                && raw.len() <= 64
                && (raw.len() == 1 || !raw.starts_with('0'))
                && raw.bytes().all(|b| b.is_ascii_hexdigit())
        });
    let text = text.ok_or_else(|| {
        evm::Error::invalid(format!("{method} returned a malformed Ethereum quantity"))
    })?;
    U256::from_str_radix(text, 16).map_err(|_| {
        evm::Error::invalid(format!(
            "{method} returned an overflowing Ethereum quantity"
        ))
    })
}
async fn number(backend: &dyn SetupBackend, method: &str) -> evm::Result<u64> {
    u64::try_from(quantity(backend, method, &[]).await?)
        .map_err(|_| evm::Error::invalid(format!("{method} exceeds u64")))
}
fn rpc_failure(error: evm::Error) -> CliError {
    match error {
        evm::Error::Transport(_) | evm::Error::Timeout(_) => failure(error),
        _ => failure(format!("EVM RPC error: {error}")).with_help("subtensor reports any invalid transaction through eth_estimateGas — insufficient balance, a bad call, an unset chain ID, or an enabled deployment whitelist all surface here, not just gas problems"),
    }
}
fn diagnostic(ctx: &AppContext, message: &str, help: &str) -> Result<(), CliError> {
    ctx.output
        .error(
            message,
            DiagnosticContext {
                help: Some(help),
                ..Default::default()
            },
        )
        .map_err(failure)
}
async fn doctor(
    ctx: &AppContext,
    matches: &ArgMatches,
    network: &EvmNetwork,
    backend: &dyn SetupBackend,
) -> Result<(), CliError> {
    let mut fields = vec![("rpc url".into(), json!(network.rpc_url))];
    let block = number(backend, "eth_blockNumber").await.map_err(|error| {
        failure(format!(
            "EVM RPC unreachable at {}: {error}",
            network.rpc_url
        ))
        .with_help("check the endpoint; on localnet, start the chain with ./scripts/localnet.sh")
    })?;
    fields.push(("block number".into(), json!(block)));
    match number(backend, "eth_chainId").await {
        Ok(chain_id) => {
            fields.push(("chain id".into(), json!(chain_id)));
            if let Some(expected) = network.chain_id.filter(|expected| *expected != chain_id) {
                diagnostic(
                    ctx,
                    &format!(
                        "chain ID mismatch: node reports {chain_id}, expected {expected} for {}",
                        network.name
                    ),
                    "MetaMask and signed transactions will reject the wrong chain ID",
                )?;
            }
        }
        Err(error @ evm::Error::Rpc { .. }) => {
            fields.push(("chain id".into(), json!(format!("error: {error}"))));
            diagnostic(
                ctx,
                "chain ID not set on this node",
                "on a fresh localnet, set it with the AdminUtils.sudo_set_evm_chain_id sudo extrinsic (945 to simulate testnet, 964 mainnet)",
            )?;
        }
        Err(error) => return Err(rpc_failure(error)),
    }
    let gas = quantity(backend, "eth_gasPrice", &[])
        .await
        .map_err(rpc_failure)?;
    fields.push((
        "gas price (wei)".into(),
        serde_json::from_str(&gas.to_string()).map_err(failure)?,
    ));
    let stored = backend.stored_keys(ctx).await?;
    let key = optional(matches, "evm_key");
    let info = if key.is_some() {
        Some(backend.key_info(ctx, key).await?)
    } else {
        stored
            .iter()
            .find(|key| key.name == "default")
            .or_else(|| stored.first())
            .cloned()
    };
    if let Some(info) = info {
        let wei = quantity(
            backend,
            "eth_getBalance",
            &[json!(info.address), json!("latest")],
        )
        .await
        .map_err(rpc_failure)?;
        fields.push((
            format!("key {} ({})", info.name, info.address),
            json!(evm::wei_to_balance(wei).map_err(failure)?.to_string()),
        ));
        let nonce = quantity(
            backend,
            "eth_getTransactionCount",
            &[json!(info.address), json!("latest")],
        )
        .await
        .map_err(rpc_failure)?;
        fields.push((
            "nonce".into(),
            json!(u64::try_from(nonce).map_err(failure)?),
        ));
        if wei.is_zero() {
            ctx.output.message(&format!("key {} has no balance — fund it with `btcli evm fund` (or transfer TAO to its mirror {})", info.name, info.ss58_mirror)).map_err(failure)?;
        }
    }
    ctx.output
        .detail(
            Some(&format!("EVM endpoint: {}", network.name)),
            &fields,
            None,
        )
        .map_err(failure)
}

pub(super) fn refuse_public_network(ctx: &AppContext) -> Result<(), CliError> {
    if matches!(ctx.globals.network.as_str(), "finney" | "test") {
        return Err(CliError::input(format!(
            "refusing to run against {} — this command is for localnets",
            ctx.globals.network
        ))
        .with_help("pass --network local (or a ws:// endpoint) and a sudo-holding wallet"));
    }
    Ok(())
}
fn stages(ctx: &AppContext, plan: &[SetupStep]) -> Vec<ReviewSection> {
    let steps = plan
        .iter()
        .enumerate()
        .map(|(i, step)| (format!("step {}", i + 1), json!(step.summary())))
        .collect::<Vec<_>>();
    let mut setup = vec![
        (
            "summary".into(),
            json!(format!("submit {} sudo extrinsic(s)", plan.len())),
        ),
        ("wallet".into(), json!(ctx.globals.wallet_name)),
    ];
    setup.extend(steps.clone());
    let mut transaction = vec![("operation".into(), json!("Sudo.sudo"))];
    transaction.extend(steps);
    vec![
        ReviewSection {
            title: "Setup".into(),
            rows: setup,
        },
        ReviewSection {
            title: "Fees".into(),
            rows: vec![("estimated fee".into(), json!("unavailable"))],
        },
        ReviewSection {
            title: "Signer".into(),
            rows: vec![("signer".into(), json!(ctx.globals.wallet_name))],
        },
        ReviewSection {
            title: "Transaction".into(),
            rows: transaction,
        },
    ]
}
async fn setup(
    ctx: &mut AppContext,
    matches: &ArgMatches,
    backend: &dyn SetupBackend,
    execution: SetupExecution<'_>,
) -> Result<(), CliError> {
    refuse_public_network(ctx)?;
    let chain_id = matches.get_one::<u64>("chain_id").copied().unwrap_or(945);
    let current = match number(backend, "eth_chainId").await {
        Ok(id) => Some(id),
        Err(error @ (evm::Error::Transport(_) | evm::Error::Timeout(_))) => return Err(failure(format!("EVM RPC unreachable: {error}")).with_help("start the chain first; on a source-built localnet pass --rpc-url http://127.0.0.1:9945")),
        Err(evm::Error::Rpc { .. }) => None,
        Err(error) => return Err(rpc_failure(error)),
    };
    let mut plan = Vec::new();
    if current == Some(chain_id) {
        ctx.output
            .message(&format!("chain ID already {chain_id} — skipping"))
            .map_err(failure)?;
    } else {
        plan.push(SetupStep::ChainId { chain_id, current });
    }
    plan.push(SetupStep::DisableWhitelist);
    ctx.output
        .transaction_card(&stages(ctx, &plan))
        .map_err(failure)?;
    if ctx.globals.dry_run {
        return ctx.output.detail(Some("setup-localnet plan (each submitted as Sudo.sudo, signed by the wallet coldkey)"), &plan.iter().enumerate().map(|(i, step)| (format!("step {}", i + 1), json!(step.summary()))).collect::<Vec<_>>(), None).map_err(failure);
    }
    ctx.confirm(&format!(
        "submit {} sudo extrinsic(s) with wallet {}",
        plan.len(),
        rtao::primitives::pyfmt::string_repr(&ctx.globals.wallet_name)
    ))?;
    // One aggregate confirmation covers these exact immutable steps. Shared
    // submission still performs and binds the real runtime/policy review; its
    // redundant yes/no prompts are skipped only in this scoped context clone.
    let mut confirmed = ctx.clone();
    confirmed.globals.assume_yes = true;
    let mut completed = Vec::new();
    for step in &plan {
        let attempt = backend
            .submit(
                &mut confirmed,
                step,
                SetupExecution {
                    client: execution.client,
                    options: execution.options,
                    external: execution.external,
                    history: &mut *execution.history,
                },
            )
            .await;
        let result = match attempt {
            Ok(Some(result)) => result,
            Ok(None) => {
                return Err(recovery(
                    failure("setup stopped without a submitted result"),
                    &completed,
                ));
            }
            Err(error) => return Err(recovery(error, &completed)),
        };
        let mut known = json!({"step":step.summary(),"result":result.to_json()});
        if let Some(hash) = result.fate_unknown {
            // Python-shaped receipt JSON omits this Rust reconciliation field.
            // Keep the exact attempted hash in the sequential recovery record.
            known["fate_unknown"] = json!(format!("0x{}", hex::encode(hash.0)));
        }
        completed.push(known);
        let saved_approval = confirmed
            .books
            .multisig(&confirmed.globals.wallet_name)
            .is_some_and(|entry| entry["threshold"].as_u64().unwrap_or(0) >= 2)
            && !crate::call::multisig_executed(&result);
        if !result.success
            || result.fate_unknown.is_some()
            || !matches!(result.inclusion, Inclusion::InBlock | Inclusion::Finalized)
            || saved_approval
        {
            return Err(recovery(
                failure(
                    "setup stopped before the next step; reconcile the last native result before submitting again",
                ),
                &completed,
            ));
        }
    }
    ctx.output.message("localnet is EVM-ready — `btcli evm config --format metamask` prints the wallet settings, `btcli evm doctor` verifies").map_err(failure)
}
fn recovery(mut error: CliError, results: &[Value]) -> CliError {
    if !results.is_empty() {
        let retained = format!(
            "earlier setup results (do not replay completed steps): {}",
            json!(results)
        );
        error.help = Some(match error.help.take() {
            Some(help) => format!("{help}; {retained}"),
            None => retained,
        });
    }
    error
}
