//! Minimal validator binary and wallet-free TLS connection diagnostic.

use clap::{CommandFactory, Parser};
use rtao_neuron::{
    EmaScores, NeuronConfig, NeuronCx, NeuronRuntime, StateCell, Validator, WeightsInput,
    WeightsProposal,
};
use rtao_primitives::MechId;

#[derive(Parser)]
#[command(about = "Run a hotkey-only rtao validator")]
struct Arguments {
    #[command(flatten)]
    neuron: NeuronConfig,
    /// Connect, validate genesis and metadata, then close before loading a wallet.
    #[arg(long)]
    check_connection: bool,
    /// Stderr tracing format.
    #[arg(long, value_enum, default_value = "compact")]
    log_format: rtao_neuron::telemetry::LogFormat,
    /// Fixed weights version key; upgrade this binary before raising the value.
    #[arg(
        long = "weights-version-key",
        env = "RTAO_WEIGHTS_VERSION_KEY",
        default_value_t = 0
    )]
    weights_version_key: u64,
    /// Give every registered non-owner miner the same score for protocol testing.
    #[arg(long)]
    synthetic_weights: bool,
}
struct Scores {
    synthetic: bool,
}
impl Validator for Scores {
    type State = EmaScores;
    async fn step(&self, cx: &NeuronCx, state: &StateCell<Self::State>) -> anyhow::Result<()> {
        // A subnet binary replaces this protocol-test score with its own queries.
        if !self.synthetic {
            return Ok(());
        }
        use rtao_signer::KeyView;
        let hotkey = cx.hotkey.hotkey_address()?;
        let graph = cx.metagraph.load();
        let mut scores = state.write();
        for index in 0..graph.len() {
            let uid = rtao_primitives::Uid(u16::try_from(index)?);
            if let Some(miner) = graph
                .hotkey(uid)
                .filter(|key| **key != hotkey && **key != graph.header.owner_hotkey)
            {
                scores.update(*miner, 1.0, 1.0)?;
            }
        }
        Ok(())
    }
    fn weights(&self, _cx: &NeuronCx, state: &Self::State, _mechanism: MechId) -> WeightsProposal {
        if !self.synthetic {
            return WeightsProposal::Skip("scoring is not configured");
        }
        let scores = state
            .iter()
            .map(|(hotkey, score)| (*hotkey, score))
            .collect::<Vec<_>>();
        if scores.is_empty() {
            WeightsProposal::Skip("no miner scores available")
        } else {
            WeightsProposal::Set(WeightsInput::ByHotkey(scores))
        }
    }
    fn on_metagraph(&self, diff: &rtao_metagraph::MetagraphDiff, state: &mut Self::State) {
        state.apply(diff);
    }
}
#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // The binary owns argv. NeuronConfig only resolves the supplied matches.
    let mut command = Arguments::command();
    command = command.mut_arg("netuid", |arg| arg.required(false));
    let matches = command.get_matches();
    let log_format = matches
        .get_one::<rtao_neuron::telemetry::LogFormat>("log_format")
        .copied()
        .unwrap_or_default();
    if matches.get_flag("check_connection") {
        let environment = std::env::vars_os()
            .filter_map(|(key, value)| key.into_string().ok().map(|key| (key, value)))
            .collect();
        let home = std::env::var_os("HOME").ok_or_else(|| anyhow::anyhow!("HOME is required"))?;
        let config = NeuronConfig::load_with(
            std::path::Path::new(&home),
            &environment,
            Some(&matches),
            Some(rtao_primitives::NetUid(1)),
        )
        .await?;
        rtao_neuron::telemetry::init_tracing(config.log, log_format)?;
        let client = rtao_client::Client::connect(rtao_neuron::client_config(&config)?).await?;
        client.ensure_signing().await?;
        let runtime = client.refresh_runtime().await?;
        anyhow::ensure!(
            runtime.surface_report().breaking_items() == 0,
            "runtime has incompatible required metadata"
        );
        client.close().await;
        println!("connection verified");
        return Ok(());
    }
    let config = NeuronConfig::resolve_matches(&matches).await?;
    rtao_neuron::telemetry::init_tracing(config.log, log_format)?;
    let metrics = rtao_neuron::Metrics::default();
    #[cfg(feature = "prometheus")]
    metrics.install_recorder()?;
    let version_key = matches
        .get_one::<u64>("weights_version_key")
        .copied()
        .unwrap_or(0);
    NeuronRuntime::validator(
        config,
        Scores {
            synthetic: matches.get_flag("synthetic_weights"),
        },
    )
    .with_metrics(metrics)
    .with_weight_policy(rtao_neuron::WeightPolicy {
        version_key: rtao_neuron::VersionKey::Fixed(version_key),
        ..Default::default()
    })
    .run()
    .await
}
