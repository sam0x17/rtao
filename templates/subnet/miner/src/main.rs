//! Authenticated echo routes sharing the runtime's persistent state.

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
impl Miner for EchoMiner {
    type State = u64;
    fn router(&self, _cx: &NeuronCx, state: StateCell<Self::State>) -> Router {
        Router::new()
            .route("/echo", post(echo))
            .layer(Extension(state))
    }
}
async fn echo(
    VerifiedCaller(_caller): VerifiedCaller,
    Extension(state): Extension<StateCell<u64>>,
    Json(body): Json<serde_json::Value>,
) -> Json<serde_json::Value> {
    let mut requests = state.write();
    *requests = requests.saturating_add(1);
    Json(body)
}
#[tokio::main]
async fn main() -> anyhow::Result<()> {
    rustls::crypto::ring::default_provider()
        .install_default()
        .map_err(|_| anyhow::anyhow!("TLS crypto provider already installed"))?;
    let matches = Args::command().get_matches();
    let config = NeuronConfig::resolve_matches(&matches).await?;
    rtao::neuron::telemetry::init_tracing(config.log, Default::default())?;
    NeuronRuntime::miner(config, EchoMiner).run().await
}
