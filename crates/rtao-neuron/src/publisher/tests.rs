//! Production publication and discovery regressions.

use super::*;
use rtao_client::{Client, ClientConfig, Network};
use rtao_primitives::CryptoType;
use rtao_signer::{KeyView, SignerError, SignerRole};
use rtao_signer::{KeypairSigner, Signer};
use rtao_testkit::rpc::RpcClient;
use rtao_testkit::{FakeChain, FakeState, Reply};
use std::sync::atomic::{AtomicUsize, Ordering};

fn config() -> AxonConfig {
    AxonConfig {
        external_ip: Some(IpAddr::V4(Ipv4Addr::new(8, 8, 8, 8))),
        ..Default::default()
    }
}
async fn fixture(
    config: &AxonConfig,
) -> anyhow::Result<(FakeChain, Client, Executor, KeypairSigner)> {
    let signer = KeypairSigner::from_uri("//Alice", CryptoType::Sr25519)?;
    let mut state = FakeState::try_default()?;
    let mut previous = desired_axon(
        config,
        config
            .external_ip
            .ok_or_else(|| anyhow::anyhow!("fixture IP"))?,
    );
    previous.block = 20;
    state.seed_typed(
        "SubtensorModule",
        "Axons",
        (NetUid(1), signer.account()),
        previous,
    )?;
    state.seed(&st::SERVING_RATE_LIMIT, (NetUid(1),), 0u64)?;
    let chain = FakeChain::from_state(state);
    let mut cfg = ClientConfig::new(Network::resolve_with("local", |_| None)?)?;
    cfg.runtime_cache_dir = None;
    cfg.config_store = None;
    let client = Client::from_rpc(RpcClient::new(chain.clone()), cfg).await?;
    let executor = rtao_intents::executor(client.clone());
    Ok((chain, client, executor, signer))
}
#[tokio::test]
async fn unchanged_axon_never_unlocks_or_submits_even_during_rate_limit() -> anyhow::Result<()> {
    let config = config();
    let (chain, client, executor, signer) = fixture(&config).await?;
    chain.mutate_state(|state| state.seed(&st::SERVING_RATE_LIMIT, (NetUid(1),), u64::MAX))?;
    let mut publisher = AxonPublisher::new(config)?;
    struct PublicHotkey {
        account: rtao_primitives::AccountId,
        unlocks: AtomicUsize,
    }
    impl KeyView for PublicHotkey {
        fn hotkey_address(&self) -> Result<rtao_primitives::AccountId, SignerError> {
            Ok(self.account)
        }
        fn hotkey_public_key(&self) -> Result<[u8; 32], SignerError> {
            Ok(self.account.0)
        }
        fn coldkey_address(&self) -> Result<rtao_primitives::AccountId, SignerError> {
            Err(SignerError::KeyUnavailable(SignerRole::Coldkey))
        }
    }
    impl KeySource<Hotkey> for PublicHotkey {
        fn address(&self) -> Result<rtao_primitives::AccountId, SignerError> {
            Ok(self.account)
        }
        async fn signer(&self) -> Result<Arc<dyn Signer>, SignerError> {
            self.unlocks.fetch_add(1, Ordering::SeqCst);
            Err(SignerError::KeyView("unexpected publisher unlock".into()))
        }
    }
    let keys = PublicHotkey {
        account: signer.account(),
        unlocks: AtomicUsize::new(0),
    };
    let outcome = publisher.poll(&executor, &keys, NetUid(1)).await;
    client.close().await;
    assert_eq!(outcome?, AxonPublishOutcome::Unchanged);
    assert!(chain.mutate_state(|state| state.submissions.is_empty()));
    assert_eq!(keys.unlocks.load(Ordering::SeqCst), 0);
    Ok(())
}
#[tokio::test]
async fn cert_rotation_republishes() -> anyhow::Result<()> {
    let mut config = config();
    config.certificate = Some(NeuronCertificate {
        algorithm: 1,
        public_key: vec![2; 64],
    });
    let (chain, client, executor, signer) = fixture(&config).await?;
    chain.mutate_state(|state| {
        state.seed_typed(
            "SubtensorModule",
            "NeuronCertificates",
            (NetUid(1), signer.account()),
            NeuronCertificate {
                algorithm: 1,
                public_key: vec![1; 64],
            },
        )
    })?;
    let mut publisher = AxonPublisher::new(config.clone())?;
    let outcome = publisher.poll(&executor, &signer, NetUid(1)).await;
    assert!(matches!(outcome?, AxonPublishOutcome::Published { .. }));
    assert_eq!(chain.mutate_state(|state| state.submissions.len()), 1);
    let certificate = config
        .certificate
        .clone()
        .ok_or_else(|| anyhow::anyhow!("certificate"))?;
    chain.mutate_state(|state| {
        state.seed_typed(
            "SubtensorModule",
            "NeuronCertificates",
            (NetUid(1), signer.account()),
            certificate,
        )
    })?;
    let unchanged = publisher.poll(&executor, &signer, NetUid(1)).await;
    client.close().await;
    assert_eq!(unchanged?, AxonPublishOutcome::Unchanged);
    assert_eq!(chain.mutate_state(|state| state.submissions.len()), 1);
    Ok(())
}
#[tokio::test]
async fn changed_axon_schedules_first_allowed_block_without_submission() -> anyhow::Result<()> {
    let config = config();
    let (chain, client, executor, signer) = fixture(&config).await?;
    chain.mutate_state(|state| state.seed(&st::SERVING_RATE_LIMIT, (NetUid(1),), 1000u64))?;
    let mut changed = config;
    changed.external_port = Some(9000);
    let mut publisher = AxonPublisher::new(changed)?;
    let outcome = publisher.poll(&executor, &signer, NetUid(1)).await;
    client.close().await;
    assert_eq!(outcome?, AxonPublishOutcome::Scheduled { retry_at: 1020 });
    assert!(chain.mutate_state(|state| state.submissions.is_empty()));
    assert_eq!(rate_limit_retry(1020, 20, 1000), None);
    assert_eq!(rate_limit_retry(1, 0, 50), None);
    Ok(())
}
#[tokio::test]
async fn stored_certificate_is_ignored_without_configured_certificate() -> anyhow::Result<()> {
    let config = config();
    let (chain, client, executor, signer) = fixture(&config).await?;
    chain.mutate_state(|state| {
        state.seed_typed(
            "SubtensorModule",
            "NeuronCertificates",
            (NetUid(1), signer.account()),
            NeuronCertificate {
                algorithm: 1,
                public_key: vec![1; 64],
            },
        )
    })?;
    let mut publisher = AxonPublisher::new(config)?;
    let outcome = publisher.poll(&executor, &signer, NetUid(1)).await;
    client.close().await;
    assert_eq!(outcome?, AxonPublishOutcome::Unchanged);
    assert!(chain.mutate_state(|state| state.submissions.is_empty()));
    Ok(())
}
#[test]
fn tls_certificate_length_checked() -> anyhow::Result<()> {
    let mut config = config();
    config.certificate = Some(NeuronCertificate {
        algorithm: 9,
        public_key: vec![2; 64],
    });
    config.validate()?;
    assert_eq!(
        checked_certificate(
            config
                .certificate
                .as_ref()
                .ok_or_else(|| anyhow::anyhow!("certificate"))?
        )?
        .len(),
        65
    );
    config
        .certificate
        .as_mut()
        .ok_or_else(|| anyhow::anyhow!("certificate"))?
        .public_key
        .push(2);
    assert!(config.validate().is_err());
    Ok(())
}
#[test]
fn nonglobal_addresses_require_local_opt_in_and_ipv6_is_encoded() -> anyhow::Result<()> {
    for text in [
        "127.0.0.1",
        "10.0.0.1",
        "172.16.0.1",
        "192.168.0.1",
        "100.64.0.1",
        "169.254.1.1",
        "192.0.2.1",
        "0.0.0.0",
        "224.0.0.1",
        "::1",
        "fe80::1",
        "fc00::1",
        "2001:db8::1",
    ] {
        let mut config = config();
        config.external_ip = Some(text.parse()?);
        assert!(config.validate().is_err(), "{text}");
        config.allow_non_global = true;
        config.validate()?;
    }
    let ip: IpAddr = "2606:4700:4700::1111".parse()?;
    for text in [
        "64:ff9b::808:808",
        "2001:1::1",
        "2001:3::1",
        "2001:4:112::1",
        "2001:20::1",
        "2001:30::1",
        "192.88.99.2",
    ] {
        let mut config = config();
        config.external_ip = Some(text.parse()?);
        config.validate()?;
    }
    let mut config = config();
    config.external_ip = Some(ip);
    config.validate()?;
    let desired = desired_axon(&config, ip);
    assert_eq!(desired.ip_type, 6);
    assert_eq!(
        desired.ip,
        u128::from("2606:4700:4700::1111".parse::<std::net::Ipv6Addr>()?)
    );
    Ok(())
}
#[tokio::test]
async fn serving_pool_codes_retain_semantic_names() -> anyhow::Result<()> {
    for (code, name) in [(11, "InvalidIpAddress"), (13, "InvalidPort")] {
        let initial = config();
        let (chain, client, executor, signer) = fixture(&initial).await?;
        chain.mutate_state(|state| {
            state.queue_reply(
                "author_submitAndWatchExtrinsic",
                Reply::error(
                    1010,
                    "Invalid Transaction",
                    Some(serde_json::json!(format!("Custom error: {code}"))),
                ),
            )
        });
        let mut publisher = AxonPublisher::new(AxonConfig {
            external_port: Some(9000),
            ..initial
        })?;
        let result = publisher.poll(&executor, &signer, NetUid(1)).await;
        client.close().await;
        let Err(PublisherError::Transaction(TxError::Chain(error))) = result else {
            anyhow::bail!("expected typed pool failure: {result:?}");
        };
        assert_eq!(error.name.as_deref(), Some(name));
    }
    Ok(())
}
#[tokio::test]
async fn rate_limit_pool_rejection_rereads_current_schedule() -> anyhow::Result<()> {
    let initial = config();
    let (chain, client, executor, signer) = fixture(&initial).await?;
    let account = signer.account();
    let advertised = initial.clone();
    chain.mutate_state(|state| {
        state.set_handler("author_submitAndWatchExtrinsic", move |state, _| {
            let mut previous = desired_axon(&advertised, IpAddr::V4(Ipv4Addr::new(8, 8, 8, 8)));
            previous.block = state.block;
            if let Err(error) = state
                .seed_typed("SubtensorModule", "Axons", (NetUid(1), account), previous)
                .and_then(|()| state.seed(&st::SERVING_RATE_LIMIT, (NetUid(1),), 100u64))
            {
                return Reply::error(-32000, error.to_string(), None);
            }
            Reply::error(
                1010,
                "Invalid Transaction",
                Some(serde_json::json!("Custom error: 12")),
            )
        })
    });
    let best = chain.mutate_state(|state| state.block);
    let mut publisher = AxonPublisher::new(AxonConfig {
        external_port: Some(9000),
        ..initial
    })?;
    let result = publisher.poll(&executor, &signer, NetUid(1)).await;
    client.close().await;
    assert_eq!(
        result?,
        AxonPublishOutcome::Scheduled {
            retry_at: best + 100
        }
    );
    Ok(())
}
async fn bind(
    text: &'static str,
) -> anyhow::Result<(Url, tokio::task::JoinHandle<std::io::Result<()>>)> {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let url = Url::parse(&format!("http://{}/", listener.local_addr()?))?;
    let handle = tokio::spawn(async move {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let (mut socket, _) = listener.accept().await?;
        let mut request = [0; 1024];
        let _ = socket.read(&mut request).await?;
        socket
            .write_all(
                format!(
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{text}",
                    text.len()
                )
                .as_bytes(),
            )
            .await?;
        socket.shutdown().await
    });
    Ok((url, handle))
}

#[tokio::test]
async fn explicit_ips_skip_native_roots_until_discovery_is_enabled() -> anyhow::Result<()> {
    static LOADS: AtomicUsize = AtomicUsize::new(0);
    fn native_roots() -> Vec<rustls::pki_types::CertificateDer<'static>> {
        LOADS.fetch_add(1, Ordering::SeqCst);
        Vec::new()
    }
    let mut settings = config();
    let mut publisher = AxonPublisher::with_root_loader(settings.clone(), native_roots)?;
    assert_eq!(LOADS.load(Ordering::SeqCst), 0);
    assert_eq!(Some(publisher.resolve_ip().await?), settings.external_ip);
    settings.external_ip = Some("1.1.1.1".parse()?);
    publisher.update_config(settings.clone())?;
    assert_eq!(Some(publisher.resolve_ip().await?), settings.external_ip);
    assert_eq!(LOADS.load(Ordering::SeqCst), 0);

    let mut invalid = settings.clone();
    invalid.port = 0;
    assert!(publisher.update_config(invalid).is_err());
    assert_eq!(Some(publisher.resolve_ip().await?), settings.external_ip);
    assert_eq!(LOADS.load(Ordering::SeqCst), 0);

    let (source, server) = bind("9.9.9.9\n").await?;
    settings.external_ip = None;
    settings.ip_sources = vec![source];
    publisher.update_config(settings.clone())?;
    assert_eq!(LOADS.load(Ordering::SeqCst), 0);
    let discovered = publisher.resolve_ip().await;
    server.await??;
    assert_eq!(discovered?, "9.9.9.9".parse::<IpAddr>()?);
    assert_eq!(LOADS.load(Ordering::SeqCst), 1);

    settings.external_ip = Some("1.0.0.1".parse()?);
    publisher.update_config(settings.clone())?;
    assert_eq!(Some(publisher.resolve_ip().await?), settings.external_ip);
    assert_eq!(LOADS.load(Ordering::SeqCst), 1);
    let (source, server) = bind("8.8.4.4\n").await?;
    settings.external_ip = None;
    settings.ip_sources = vec![source];
    publisher.update_config(settings)?;
    let discovered = publisher.resolve_ip().await;
    server.await??;
    assert_eq!(discovered?, "8.8.4.4".parse::<IpAddr>()?);
    assert_eq!(LOADS.load(Ordering::SeqCst), 1);
    Ok(())
}

#[tokio::test]
async fn discovery_falls_back_and_bounds_response_bytes() -> anyhow::Result<()> {
    let (bad, bad_server) = bind("::1").await?;
    let (good, good_server) = bind("8.8.4.4\n").await?;
    let settings = AxonConfig {
        external_ip: None,
        ip_sources: vec![bad, good],
        ..config()
    };
    let publisher = AxonPublisher::new(settings)?;
    let outcome = publisher.resolve_ip().await;
    bad_server.await??;
    good_server.await??;
    assert_eq!(outcome?, IpAddr::V4(Ipv4Addr::new(8, 8, 4, 4)));
    let (source,server)=bind("xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx").await?;
    let publisher = AxonPublisher::new(AxonConfig {
        external_ip: None,
        ip_sources: vec![source],
        ..config()
    })?;
    let result = publisher.resolve_ip().await;
    server.await??;
    assert!(result.is_err());
    Ok(())
}
