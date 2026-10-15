//! Read one live subnet through the public client and curated namespace.

use rtao_client::{Client, ClientConfig};
use rtao_metagraph::SubnetsMetagraphExt;
use rtao_primitives::NetUid;
use rtao_reads::Namespaces;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let netuid = std::env::args()
        .nth(1)
        .map(|value| value.parse::<u16>())
        .transpose()?
        .unwrap_or(1);
    let mut config = ClientConfig::for_network("finney")?;
    config.runtime_cache_dir = None;
    config.config_store = None;
    config.track_heads = false;
    let client = Client::connect(config).await?;
    let result = client.subnets().metagraph(NetUid::new(netuid)?).await;
    client.close().await;
    let Some(graph) = result? else {
        println!("subnet {netuid} is absent");
        return Ok(());
    };
    println!(
        "subnet={} block={} spec={} decoder={:?} neurons={} validators={} commitments={} unregistered={}",
        graph.header.netuid.0,
        graph.at.number,
        graph.at.spec_version,
        graph.decode_path(),
        graph.len(),
        graph.validators().count(),
        graph
            .commitments
            .as_ref()
            .map_or(0, |entries| entries.len()),
        graph
            .unregistered_commitments
            .as_ref()
            .map_or(0, |entries| entries.len())
    );
    Ok(())
}
