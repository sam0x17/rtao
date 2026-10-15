//! Replace the echo scoring rule with your subnet's challenge protocol.

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
struct EchoValidator;
impl Validator for EchoValidator {
    type State = EmaScores;
    async fn step(&self, cx: &NeuronCx, state: &StateCell<Self::State>) -> anyhow::Result<()> {
        let graph = cx.metagraph.load();
        let challenge = serde_json::json!({"challenge": "subnet-echo-v1"});
        let body = challenge.clone();
        let replies = cx
            .http
            .fan_out::<serde_json::Value, serde_json::Value>(
                graph.axon_targets(),
                move |_| (http::Method::POST, "/echo".into(), body.clone()),
                rtao::btauth::client::FanOut::default(),
            )
            .await;
        let latest = cx.metagraph.load();
        let mut scores = state.write();
        for (uid, reply) in replies {
            if let Some(hotkey) = graph
                .hotkey(uid)
                .filter(|hotkey| latest.by_hotkey(hotkey).is_some())
            {
                let correct = reply.is_ok_and(|reply| reply == challenge);
                scores.update(*hotkey, f64::from(u8::from(correct)), 0.1)?;
                if correct {
                    println!("btauth echo accepted uid={} hotkey={hotkey}", uid.0);
                }
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
        let positive: Vec<_> = state
            .iter()
            .filter(|(_, score)| *score > 0.0)
            .map(|(hotkey, score)| (*hotkey, score))
            .collect();
        if positive.is_empty() {
            WeightsProposal::Skip("no successful challenge replies yet")
        } else {
            WeightsProposal::Set(WeightsInput::ByHotkey(positive))
        }
    }
    fn on_metagraph(&self, diff: &rtao::metagraph::MetagraphDiff, state: &mut Self::State) {
        state.apply(diff);
    }
}
#[tokio::main]
async fn main() -> anyhow::Result<()> {
    rustls::crypto::ring::default_provider()
        .install_default()
        .map_err(|_| anyhow::anyhow!("TLS crypto provider already installed"))?;
    let matches = Args::command().get_matches();
    let config = NeuronConfig::resolve_matches(&matches).await?;
    rtao::neuron::telemetry::init_tracing(config.log, Default::default())?;
    let version_key = matches
        .get_one::<u64>("weights_version_key")
        .copied()
        .unwrap_or(0);
    NeuronRuntime::validator(config, EchoValidator)
        .with_weight_policy(rtao::neuron::WeightPolicy {
            version_key: rtao::neuron::VersionKey::Fixed(version_key),
            ..Default::default()
        })
        .run()
        .await
}
