//! The real RPC TLS connector works with both providers and no process default.
#![forbid(unsafe_code)]

use std::{error::Error, time::Duration};

use rtao_rpc::{ReadMethod, Session, SessionConfig};
use rtao_testkit::FakeNode;
use url::Url;

type Result<T = ()> = std::result::Result<T, Box<dyn Error + Send + Sync>>;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn wss_with_two_compiled_providers_and_no_process_default() -> Result {
    assert!(rustls::crypto::CryptoProvider::get_default().is_none());
    assert!(
        !rustls::crypto::ring::default_provider()
            .cipher_suites
            .is_empty()
    );
    assert!(
        !rustls::crypto::aws_lc_rs::default_provider()
            .cipher_suites
            .is_empty()
    );
    let mut node = FakeNode::start_tls().await?;
    assert!(rustls::crypto::CryptoProvider::get_default().is_none());
    let outcome: Result = async {
        let mut config = SessionConfig::new(Url::parse(node.url())?);
        config
            .extra_roots
            .push(node.extra_root().ok_or("TLS fake must expose its root")?);
        config.handshake_timeout = Duration::from_secs(5);
        config.silence_timeout = Duration::from_secs(5);
        config.max_retries = 1;
        let session =
            tokio::time::timeout(Duration::from_secs(5), Session::connect(config)).await??;
        let reply = tokio::time::timeout(
            Duration::from_secs(5),
            session.request::<String>(ReadMethod::SystemChain, Vec::<String>::new()),
        )
        .await;
        session.close().await;
        assert_eq!(reply??, "FakeChain");
        assert_eq!(node.connect_attempts(), 1);
        assert_eq!(
            node.connections()
                .iter()
                .filter(|record| record.accepted)
                .count(),
            1
        );
        assert!(rustls::crypto::CryptoProvider::get_default().is_none());
        Ok(())
    }
    .await;
    node.shutdown().await;
    outcome?;
    assert!(rustls::crypto::CryptoProvider::get_default().is_none());
    Ok(())
}
