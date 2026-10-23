//! Actual private paired path while the public route remains review-gated.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
use super::*;
use crate::journal::{
    ExecutionMode, JournalError, OperationIntent, OperationJournal, PairPhase, ShieldProtocolGuard,
};
use rtao_signer::{KeypairSigner, Signer};
use rtao_testkit::{FakeNode, FakeState};
use serde_json::Value;
use std::{
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
};

struct Fixture {
    root: PathBuf,
    node: FakeNode,
    client: Client,
    signer: KeypairSigner,
    journal: Arc<OperationJournal>,
    binding: crate::journal::JournalBinding,
    config: crate::ClientConfig,
    include_inner: bool,
}
struct TestOnlyProtocol {
    genesis: H256,
}
impl ShieldProtocolGuard for TestOnlyProtocol {
    fn profile(&self) -> &str {
        "synthetic-local-signed-inner"
    }
    fn check(&self, genesis: H256, code_hash: H256) -> Result<(), crate::journal::Incompatibility> {
        if genesis == self.genesis && code_hash == H256([42; 32]) {
            return Ok(());
        }
        Err(crate::journal::Incompatibility {
            code: "synthetic-profile-refused".into(),
            descriptor: None,
            detail: "install audited runtime qualification".into(),
            extensions: Default::default(),
        })
    }
}
impl Fixture {
    async fn new(inner: bool) -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let root = PathBuf::from(std::env::var_os("RTAO_TEST_TMPDIR").unwrap()).join(format!(
            "paired-native-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&root).unwrap();
        let mut state = FakeState::default();
        state
            .seed_constant("Aura", "SlotDuration", json!(1))
            .unwrap();
        state
            .seed_typed("MevShield", "NextKey", (), vec![0u8; 1184])
            .unwrap();
        state.set_result(
            "state_getStorageHash",
            json!(crate::hash_hex(H256([42; 32]))),
        );
        let node = FakeNode::with_state(Arc::new(std::sync::Mutex::new(state)))
            .await
            .unwrap();
        let mut config =
            crate::ClientConfig::new(crate::Network::resolve_with("local", |_| None).unwrap())
                .unwrap();
        config.track_heads = false;
        config.runtime_cache_dir = None;
        config.config_store = None;
        let session_config = rtao_rpc::SessionConfig::new(url::Url::parse(node.url()).unwrap());
        let session = rtao_rpc::Session::connect(session_config).await.unwrap();
        let client = Client::from_session(session, config.clone()).await.unwrap();
        let signer = KeypairSigner::from_seed(&[7; 32], CryptoType::Ed25519).unwrap();
        let journal = OperationJournal::open(root.join("journal.json")).unwrap();
        let binding = journal
            .begin(OperationIntent {
                id: "durable-shield".into(),
                correlation_id: Some("application-revision".into()),
                genesis: client.genesis(),
                account: signer.account(),
                scope: "paired-native-test".into(),
                effects: json!({"remark":[1,2,3]}),
                obligation: false,
                semantic_policy: None,
                extensions: Default::default(),
            })
            .unwrap()
            .with_shield_protocol(Arc::new(TestOnlyProtocol {
                genesis: client.genesis(),
            }))
            .material(json!({"required":"synthetic-material-marker"}))
            .unwrap();
        install_author_observer(&node, binding.clone(), inner);
        if inner {
            install_inner_history(&node, binding.clone());
        }
        Self {
            root,
            node,
            client,
            signer,
            journal,
            binding,
            config,
            include_inner: inner,
        }
    }
    async fn restart(&mut self) {
        let id = self.binding.id().to_owned();
        let mut placeholder = self.binding.record().unwrap().intent;
        placeholder.id = "restart-placeholder".into();
        self.client.close().await;
        // Remove server-side test observers' strong journal references too.
        self.node.mutate_state(|state| {
            state.set_handler("author_submitAndWatchExtrinsic", |state, request| {
                state.dispatch(request)
            });
            state.set_handler("chain_getBlock", |state, request| state.dispatch(request));
        });
        let temporary = OperationJournal::open(self.root.join("restart-placeholder.json")).unwrap();
        let temporary_binding = temporary.begin(placeholder).unwrap();
        drop(std::mem::replace(&mut self.binding, temporary_binding));
        drop(std::mem::replace(&mut self.journal, temporary));
        // This would refuse if any old native journal owner survived.
        self.journal = OperationJournal::open(self.root.join("journal.json")).unwrap();
        self.binding = self
            .journal
            .bind(&id)
            .unwrap()
            .with_shield_protocol(Arc::new(TestOnlyProtocol {
                genesis: self.client.genesis(),
            }));
        let session = rtao_rpc::Session::connect(rtao_rpc::SessionConfig::new(
            url::Url::parse(self.node.url()).unwrap(),
        ))
        .await
        .unwrap();
        self.client = Client::from_session(session, self.config.clone())
            .await
            .unwrap();
        install_author_observer(&self.node, self.binding.clone(), self.include_inner);
        if self.include_inner {
            install_inner_history(&self.node, self.binding.clone());
        }
    }
    async fn call(&self) -> CallBytes {
        rtao_runtime::calls::system::remark(vec![1, 2, 3])
            .compose(&self.client.refresh_runtime().await.unwrap())
            .unwrap()
    }
    fn options(&self) -> ShieldOptions {
        ShieldOptions {
            period: ShieldPeriod::P4,
            wait: Wait::Finalization,
            dropped_inner_retries: 0,
            submit: SubmitOptions {
                journal: Some(self.binding.clone()),
                deadline: Some(Duration::from_secs(3)),
                ..Default::default()
            },
            ..Default::default()
        }
    }
}
fn install_author_observer(
    node: &FakeNode,
    observing: crate::journal::JournalBinding,
    inner: bool,
) {
    node.mutate_state(|state| {
        state.set_handler("author_submitAndWatchExtrinsic", move |state, request| {
            let record = observing.record().unwrap();
            assert!(matches!(record.mode, ExecutionMode::Shielded(_)));
            let pair = record.shield_pairs.last().unwrap();
            let carrier = record.attempt(pair.carrier.unwrap()).unwrap();
            let signed_inner = record.attempt(pair.inner).unwrap();
            assert!(matches!(pair.phase, PairPhase::DispatchAuthorized { .. }));
            assert_eq!(
                request.params[0],
                json!(format!("0x{}", hex::encode(&carrier.bytes)))
            );
            assert_eq!(
                signed_inner.material,
                json!({"required":"synthetic-material-marker"})
            );
            assert_eq!(
                signed_inner.material_generation,
                Some(record.material_generation)
            );
            assert_eq!(carrier.nonce + 1, signed_inner.nonce);
            let reply = state.dispatch(request);
            if !inner {
                state.advance(8);
            }
            reply
        })
    });
}
fn install_inner_history(node: &FakeNode, observing: crate::journal::JournalBinding) {
    node.mutate_state(|state|state.set_handler("chain_getBlock",move |state,request| {
        let mut reply=state.dispatch(request);
        if let Some(Ok(raw))=&mut reply.outcome {
            let number=raw.pointer("/block/header/number").and_then(Value::as_str)
                .and_then(|text|u64::from_str_radix(text.trim_start_matches("0x"),16).ok());
            let record=observing.record().unwrap();
            if let Some(pair)=record.shield_pairs.last() {
                let inner=record.attempt(pair.inner).unwrap();
                if number.is_some_and(|n|n>=inner.start)
                    && let Some(list)=raw["block"]["extrinsics"].as_array_mut()
                    && !list.is_empty() {
                    let index=list.len();
                    list.push(json!(format!("0x{}",hex::encode(&inner.bytes))));
                    let events=(0..=index).map(|i|json!({"phase":"ApplyExtrinsic","extrinsic_idx":i,
                        "module_id":"System","event_id":"ExtrinsicSuccess",
                        "attributes":{"dispatch_info":{"weight":{"ref_time":1,"proof_size":0},"class":"Normal","pays_fee":"Yes"}},"topics":[]})).collect::<Vec<_>>();
                    state.seed_events(request.params[0].as_str().unwrap(),json!(events)).unwrap();
                }
            }
        }
        reply
    }));
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

#[tokio::test]
async fn paired_native_publication_precedes_actual_carrier_io_and_only_finalized_inner_returns() {
    let f = Fixture::new(true).await;
    let report = f
        .client
        .submit_shielded_once(f.call().await, &f.signer, &f.options())
        .await
        .unwrap();
    let record = f.binding.record().unwrap();
    assert_eq!(report.hash, record.attempts[0].hash);
    assert_ne!(report.hash, record.attempts[1].hash);
    assert_eq!(report.inclusion, crate::Inclusion::Finalized);
    assert!(report.success);
    assert_eq!(
        record.finalized_effect().unwrap().unwrap().hash,
        report.hash
    );
    for _ in 0..2 {
        assert!(
            matches!(f.binding.recover(&f.client, 32).await.unwrap(), crate::journal::RecoveryStatus::Finalized(r) if r.hash == report.hash)
        );
    }
    assert_eq!(
        f.node
            .requests()
            .iter()
            .filter(|r| r.method == "author_submitAndWatchExtrinsic")
            .count(),
        1
    );
    assert!(!format!("{record:?}").contains("synthetic-material-marker"));
    let mut next = record.intent.clone();
    next.id = "next-authorized-work".into();
    assert!(f.journal.begin(next).unwrap().reserve().is_ok());
    f.client.close().await;
}
#[tokio::test]
async fn paired_native_carrier_only_and_closed_validity_stays_typed_pending_without_retry() {
    let f = Fixture::new(false).await;
    let result = f
        .client
        .submit_shielded_once(f.call().await, &f.signer, &f.options())
        .await;
    assert!(matches!(
        result,
        Err(SubmitFailure::PendingShield {
            pair: 0,
            reason: crate::journal::RecoveryReason::InnerUnresolved,
            ..
        })
    ));
    assert!(
        f.binding
            .record()
            .unwrap()
            .finalized_effect()
            .unwrap()
            .is_none()
    );
    assert!(f.binding.can_dispatch().is_err());
    assert_eq!(
        f.node
            .requests()
            .iter()
            .filter(|r| r.method == "author_submitAndWatchExtrinsic")
            .count(),
        1
    );
    f.client.close().await;
}
struct CountingSigner {
    inner: KeypairSigner,
    prepares: AtomicU64,
    signs: AtomicU64,
}
impl Signer for CountingSigner {
    fn account(&self) -> AccountId {
        self.inner.account()
    }
    fn crypto_type(&self) -> CryptoType {
        self.inner.crypto_type()
    }
    fn prepare_shielded(&self) -> rtao_signer::BoxFuture<'_, Result<(), rtao_signer::SignerError>> {
        Box::pin(async {
            self.prepares.fetch_add(1, Ordering::SeqCst);
            Ok(())
        })
    }
    fn sign<'a>(
        &'a self,
        request: &'a rtao_signer::SigningRequest<'a>,
    ) -> rtao_signer::BoxFuture<'a, Result<rtao_signer::MultiSignature, rtao_signer::SignerError>>
    {
        Box::pin(async move {
            self.signs.fetch_add(1, Ordering::SeqCst);
            self.inner.sign(request).await
        })
    }
}
#[tokio::test]
async fn paired_native_public_gate_and_nonfinal_private_wait_do_not_sign_or_dispatch() {
    let f = Fixture::new(true).await;
    let signer = CountingSigner {
        inner: KeypairSigner::from_seed(&[7; 32], CryptoType::Ed25519).unwrap(),
        prepares: AtomicU64::new(0),
        signs: AtomicU64::new(0),
    };
    assert!(matches!(
        f.client
            .submit_shielded(f.call().await, &signer, &f.options())
            .await,
        Err(SubmitFailure::Client(ClientError::Journal(
            JournalError::Blocked(_)
        )))
    ));
    let mut options = f.options();
    options.wait = Wait::Pool;
    assert!(matches!(
        f.client
            .submit_shielded_once(f.call().await, &signer, &options)
            .await,
        Err(SubmitFailure::Client(ClientError::Journal(
            JournalError::Blocked(_)
        )))
    ));
    assert_eq!(signer.prepares.load(Ordering::SeqCst), 0);
    assert_eq!(signer.signs.load(Ordering::SeqCst), 0);
    assert!(f.binding.record().unwrap().attempts.is_empty());
    assert!(
        f.node
            .requests()
            .iter()
            .all(|r| !r.method.starts_with("author_submit"))
    );
    f.client.close().await;
}

#[tokio::test]
async fn paired_native_refreshes_preexisting_nonce_cache_under_account_owner() {
    let f = Fixture::new(true).await;
    // Another client or previously abandoned preparation can advance this
    // client's cache independently of the actual node/account state.
    f.client
        .nonces()
        .lease_explicit(&f.signer.account(), 91)
        .settle();
    f.client
        .submit_shielded_once(f.call().await, &f.signer, &f.options())
        .await
        .unwrap();
    let record = f.binding.record().unwrap();
    assert_eq!(
        record.attempts[1].nonce, 0,
        "refresh actual account state, not cached 92"
    );
    assert_eq!(record.attempts[0].nonce, 1);
    f.client.close().await;
}

#[tokio::test]
async fn paired_native_inner_only_sealing_failure_retires_without_renewing_pair_budget() {
    let f = Fixture::new(true).await;
    let signer = CountingSigner {
        inner: KeypairSigner::from_seed(&[7; 32], CryptoType::Ed25519).unwrap(),
        prepares: AtomicU64::new(0),
        signs: AtomicU64::new(0),
    };
    let mut options = f.options();
    options.dropped_inner_retries = 1;
    f.node
        .mutate_state(|state| state.seed_typed("MevShield", "NextKey", (), vec![0u8; 1183]))
        .unwrap();
    assert!(
        f.client
            .submit_shielded_once(f.call().await, &signer, &options)
            .await
            .is_err()
    );
    let record = f.binding.record().unwrap();
    assert_eq!(record.attempts.len(), 1);
    assert_eq!(record.shield_pairs[0].phase, PairPhase::Preparing);
    assert_eq!(signer.signs.load(Ordering::SeqCst), 1);
    assert!(
        f.node
            .requests()
            .iter()
            .all(|r| !r.method.starts_with("author_submit"))
    );
    // Restart-style rebinding cannot reset the original policy or pair ordinal.
    let rebound = f
        .journal
        .bind(f.binding.id())
        .unwrap()
        .with_shield_protocol(Arc::new(TestOnlyProtocol {
            genesis: f.client.genesis(),
        }));
    options.submit.journal = Some(rebound);
    f.node
        .mutate_state(|state| state.seed_typed("MevShield", "NextKey", (), vec![0u8; 1184]))
        .unwrap();
    assert!(matches!(
        f.client
            .submit_shielded_once(f.call().await, &signer, &options)
            .await,
        Err(SubmitFailure::Client(ClientError::Journal(
            JournalError::FreshSigningContext { attempt: 0 }
        )))
    ));
    assert_eq!(f.binding.record().unwrap().shield_pairs.len(), 1);
    assert_eq!(
        f.binding.record().unwrap().shield_pairs[0].phase,
        PairPhase::Retired
    );
    assert!(
        f.node
            .requests()
            .iter()
            .all(|r| !r.method.starts_with("author_submit"))
    );
    f.node.mutate_state(|state| state.advance(1));
    let report = f
        .client
        .submit_shielded_once(f.call().await, &signer, &options)
        .await
        .unwrap();
    let record = f.binding.record().unwrap();
    assert_eq!(record.shield_pairs.len(), 2);
    assert_eq!(record.shield_pairs[0].phase, PairPhase::Retired);
    assert_eq!(record.shield_pairs[1].id, 1);
    assert_eq!(
        report.hash,
        record.attempt(record.shield_pairs[1].inner).unwrap().hash
    );
    let signs = signer.signs.load(Ordering::SeqCst);
    let repeated = f
        .client
        .submit_shielded_once(f.call().await, &signer, &options)
        .await
        .unwrap();
    assert_eq!(repeated.hash, report.hash);
    assert_eq!(signer.signs.load(Ordering::SeqCst), signs);
    assert_eq!(
        f.node
            .requests()
            .iter()
            .filter(|r| r.method == "author_submitAndWatchExtrinsic")
            .count(),
        1
    );
    f.client.close().await;
}

/// Intercepts a real native CAS before publication, without holding the store's
/// load/ownership mutex. Used to cancel the async caller while its I/O worker
/// still owns the account/operation reservation.
struct BlockPublication {
    inner: crate::journal::NativeJournalStore,
    enabled: std::sync::atomic::AtomicBool,
    entered: tokio::sync::Notify,
    released: Arc<tokio::sync::Notify>,
    proceed: std::sync::Mutex<bool>,
    wake: std::sync::Condvar,
}
struct ObservedLease {
    inner: Option<Box<dyn crate::journal::JournalLease>>,
    released: Arc<tokio::sync::Notify>,
}
impl Drop for ObservedLease {
    fn drop(&mut self) {
        drop(self.inner.take());
        self.released.notify_one();
    }
}
impl crate::journal::JournalStore for BlockPublication {
    fn claim_scope(
        &self,
        scope: &str,
    ) -> Result<Box<dyn crate::journal::JournalLease>, JournalError> {
        let lease = self.inner.claim_scope(scope)?;
        if scope.starts_with("journal-account/") {
            Ok(Box::new(ObservedLease {
                inner: Some(lease),
                released: self.released.clone(),
            }))
        } else {
            Ok(lease)
        }
    }
    fn load(&self) -> Result<crate::journal::JournalImage, JournalError> {
        self.inner.load()
    }
    fn compare_exchange(
        &self,
        expected: u64,
        image: &crate::journal::JournalImage,
    ) -> Result<(), JournalError> {
        if self.enabled.swap(false, Ordering::SeqCst) {
            self.entered.notify_one();
            let mut proceed = self.proceed.lock().unwrap();
            while !*proceed {
                proceed = self.wake.wait(proceed).unwrap();
            }
        }
        self.inner.compare_exchange(expected, image)
    }
}
struct ReleasePublication(Arc<BlockPublication>);
impl Drop for ReleasePublication {
    fn drop(&mut self) {
        *self.0.proceed.lock().unwrap() = true;
        self.0.wake.notify_all();
    }
}

#[tokio::test]
async fn paired_native_cancelled_publication_retains_actual_account_claim_until_io_worker_ends() {
    let f = Fixture::new(true).await;
    let store = Arc::new(BlockPublication {
        inner: crate::journal::NativeJournalStore::open(f.root.join("cancelled-publication.json"))
            .unwrap(),
        enabled: std::sync::atomic::AtomicBool::new(false),
        entered: tokio::sync::Notify::new(),
        released: Arc::new(tokio::sync::Notify::new()),
        proceed: std::sync::Mutex::new(false),
        wake: std::sync::Condvar::new(),
    });
    let journal = OperationJournal::new(store.clone()).unwrap();
    let binding = journal
        .begin(f.binding.record().unwrap().intent)
        .unwrap()
        .with_shield_protocol(Arc::new(TestOnlyProtocol {
            genesis: f.client.genesis(),
        }));
    let mut intent = binding.record().unwrap().intent;
    intent.id = "competing-account-operation".into();
    let competing = journal.begin(intent).unwrap();
    let preparation = binding
        .claim_shield(binding.shield_policy(4, 0, None).unwrap())
        .unwrap();
    let context = binding.qualify_shield_head(&f.client).await.unwrap();
    let unsigned = f
        .client
        .prepare_call(
            f.call().await,
            &f.signer,
            &SubmitOptions {
                nonce: crate::NoncePolicy::Explicit(1),
                mortality: Mortality::Mortal(Period::new(4)),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    let signature = f
        .signer
        .sign(&unsigned.signing_request(SignPurpose::ShieldInner))
        .await
        .unwrap();
    let (bytes, hash) = f
        .client
        .attach_signature(&unsigned, signature)
        .await
        .unwrap();
    let release = ReleasePublication(store.clone());
    store.enabled.store(true, Ordering::SeqCst);
    let task = tokio::spawn(async move {
        preparation
            .publish_inner(&unsigned, &bytes, hash, &context)
            .await
    });
    tokio::time::timeout(Duration::from_secs(2), store.entered.notified())
        .await
        .unwrap();
    task.abort();
    assert!(task.await.unwrap_err().is_cancelled());
    assert!(matches!(binding.reserve(), Err(JournalError::Owned)));
    assert!(matches!(competing.reserve(), Err(JournalError::Owned)));
    assert!(binding.record().unwrap().attempts.is_empty());
    drop(release);
    tokio::time::timeout(Duration::from_secs(2), store.released.notified())
        .await
        .unwrap();
    assert!(binding.reserve().is_ok());
    let record = binding.record().unwrap();
    assert_eq!(record.attempts.len(), 1);
    assert_eq!(record.attempts[0].hash, hash);
    assert_eq!(record.shield_pairs[0].phase, PairPhase::Preparing);
    assert!(
        f.node
            .requests()
            .iter()
            .all(|r| !r.method.starts_with("author_submit"))
    );
    f.client.close().await;
}

#[path = "negative_tests.rs"]
mod negative_tests;
