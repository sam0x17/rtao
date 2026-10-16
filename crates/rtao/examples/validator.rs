//! A validator author supplies scoring and a pure weight proposal.

use clap::{CommandFactory, Parser};
use rtao::neuron::{
    EmaScores, NeuronConfig, NeuronCx, NeuronRuntime, StateCell, Validator, WeightsInput,
    WeightsProposal,
};

#[derive(Parser)]
struct Args {
    #[command(flatten)]
    neuron: NeuronConfig,
    #[arg(long = "weights-version-key", default_value_t = 0)]
    weights_version_key: u64,
}
struct MyValidator;
#[docify::export(validator_implementation)]
impl Validator for MyValidator {
    type State = EmaScores;
    async fn step(&self, cx: &NeuronCx, state: &StateCell<Self::State>) -> anyhow::Result<()> {
        let graph = cx.metagraph.load();
        // Replace this echo challenge and scoring rule with your subnet's protocol.
        let challenge = serde_json::json!({"challenge": "rtao-example"});
        let request = challenge.clone();
        let replies = cx
            .http
            .fan_out::<serde_json::Value, serde_json::Value>(
                graph.axon_targets(),
                move |_| (http::Method::POST, "/echo".into(), request.clone()),
                rtao::btauth::client::FanOut::default(),
            )
            .await;
        let latest = cx.metagraph.load();
        let mut scores = state.write();
        for (uid, reply) in replies {
            // Bind a reply to its queried identity, even if the UID was replaced.
            if let Some(hotkey) = graph
                .hotkey(uid)
                .filter(|hotkey| latest.by_hotkey(hotkey).is_some())
            {
                let observation = f64::from(u8::from(reply.is_ok_and(|reply| reply == challenge)));
                scores.update(*hotkey, observation, 0.1)?;
            }
        }
        Ok(())
    }
    fn weights(
        &self,
        _cx: &NeuronCx,
        state: &Self::State,
        _mechanism: rtao::primitives::MechId,
    ) -> WeightsProposal {
        WeightsProposal::Set(WeightsInput::ByHotkey(
            state
                .iter()
                .map(|(hotkey, score)| (*hotkey, score))
                .collect(),
        ))
    }
    fn on_metagraph(&self, diff: &rtao::metagraph::MetagraphDiff, state: &mut Self::State) {
        state.apply(diff);
    }
}
#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let matches = Args::command().get_matches();
    let config = NeuronConfig::resolve_matches(&matches).await?;
    rtao::neuron::telemetry::init_tracing(config.log, Default::default())?;
    let metrics = rtao::neuron::Metrics::default();
    #[cfg(feature = "prometheus")]
    metrics.install_recorder()?;
    let version_key = matches
        .get_one::<u64>("weights_version_key")
        .copied()
        .unwrap_or(0);
    NeuronRuntime::validator(config, MyValidator)
        .with_metrics(metrics)
        .with_weight_policy(rtao::neuron::WeightPolicy {
            version_key: rtao::neuron::VersionKey::Fixed(version_key),
            ..Default::default()
        })
        .run()
        .await
}
