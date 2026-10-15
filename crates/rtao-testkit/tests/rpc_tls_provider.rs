//! Explicit TLS selection stays independent of the process-default provider.
//!
//! This is a separate integration-test binary because rustls process-default
//! installation is irreversible. The provider is deliberately unusable; a
//! connector that consulted it would fail before completing the TLS handshake.
//! This does not enable a second compiled cryptographic implementation.

use std::{error::Error, sync::Arc, time::Duration};

use rtao_rpc::{ReadMethod, Session, SessionConfig};
use rtao_testkit::FakeNode;
use serde_json::json;
use url::Url;

type Result<T = ()> = std::result::Result<T, Box<dyn Error + Send + Sync>>;

#[tokio::test]
async fn wss_ignores_an_unusable_process_default_provider() -> Result {
    assert!(rustls::crypto::CryptoProvider::get_default().is_none());
    let mut unusable = rustls::crypto::ring::default_provider();
    unusable.cipher_suites.clear();
    unusable.kx_groups.clear();
    unusable
        .install_default()
        .map_err(|_| "process-default provider was unexpectedly already installed")?;
    let installed = rustls::crypto::CryptoProvider::get_default()
        .ok_or("process-default provider was not installed")?;
    assert!(installed.cipher_suites.is_empty());
    assert!(installed.kx_groups.is_empty());

    let mut node = FakeNode::start_tls().await?;
    let mut config = SessionConfig::new(Url::parse(node.url())?);
    config.silence_timeout = Duration::from_secs(2);
    config.extra_roots.push(
        node.extra_root()
            .ok_or("TLS fake did not expose its trust root")?,
    );
    let session = tokio::time::timeout(Duration::from_secs(5), Session::connect(config)).await??;
    assert_eq!(
        session
            .request::<String>(ReadMethod::SystemChain, json!([]))
            .await?,
        "FakeChain"
    );
    assert!(Arc::ptr_eq(
        installed,
        rustls::crypto::CryptoProvider::get_default()
            .ok_or("process-default provider disappeared")?,
    ));
    assert_eq!(node.connections().len(), 1);
    session.close().await;
    node.shutdown().await;
    Ok(())
}
