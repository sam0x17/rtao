//! A miner author supplies authenticated JSON routes and persistent state.

use axum::{Extension, Router, routing::post};
use clap::{CommandFactory, Parser};
use rtao::{
    btauth::server::{Json, VerifiedCaller},
    neuron::{Miner, NeuronConfig, NeuronCx, NeuronRuntime, StateCell},
};

#[derive(Parser)]
struct Args {
    #[command(flatten)]
    neuron: NeuronConfig,
}
struct EchoMiner;
#[docify::export(miner_implementation)]
impl Miner for EchoMiner {
    type State = serde_json::Value;
    async fn on_start(&self, _cx: &NeuronCx, state: &StateCell<Self::State>) -> anyhow::Result<()> {
        if state.read().is_null() {
            *state.write() = serde_json::json!({"requests":0});
        }
        Ok(())
    }
    fn router(&self, _cx: &NeuronCx, state: StateCell<Self::State>) -> Router {
        Router::new()
            .route("/echo", post(echo))
            .layer(Extension(state))
    }
}
#[docify::export(miner_handler)]
async fn echo(
    VerifiedCaller(_caller): VerifiedCaller,
    Extension(state): Extension<StateCell<serde_json::Value>>,
    Json(body): Json<serde_json::Value>,
) -> Json<serde_json::Value> {
    let mut state = state.write();
    let count = state["requests"].as_u64().unwrap_or(0).saturating_add(1);
    state["requests"] = serde_json::json!(count);
    Json(body)
}
#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let matches = Args::command().get_matches();
    let config = NeuronConfig::resolve_matches(&matches).await?;
    rtao::neuron::telemetry::init_tracing(config.log, Default::default())?;
    let metrics = rtao::neuron::Metrics::default();
    #[cfg(feature = "prometheus")]
    metrics.install_recorder()?;
    NeuronRuntime::miner(config, EchoMiner)
        .with_metrics(metrics)
        .run()
        .await
}
