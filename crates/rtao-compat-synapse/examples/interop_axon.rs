//! Rust v10 axon for the bittensor==10.5.0 interop harness.
//!
//! Usage: `interop_axon <port>`; prints `READY <port> <hotkey>` once listening
//! on 127.0.0.1 and serves until killed. Synapses and behaviours mirror
//! `ci/interop/synapse/common.py`.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, missing_docs)]

#[path = "interop/synapses.rs"]
mod synapses;

use std::sync::Arc;

use rtao_compat_synapse::V10AxonConfig;
use rtao_primitives::CryptoType;
use rtao_signer::KeypairSigner;
use synapses::build_axon;

#[tokio::main]
async fn main() {
    let port: u16 = std::env::args()
        .nth(1)
        .and_then(|port| port.parse().ok())
        .expect("usage: interop_axon <port>");
    let bob = Arc::new(KeypairSigner::from_uri("//Bob", CryptoType::Sr25519).unwrap());
    let axon = build_axon(bob, V10AxonConfig::default());
    let hotkey = axon.hotkey();
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", port))
        .await
        .unwrap();
    println!("READY {port} {hotkey}");
    axon.serve(listener, std::future::pending()).await.unwrap();
}
