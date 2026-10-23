//! Actual private paired pipeline with synthetic code-qualified history.
use super::*;
use crate::journal::{AttemptOutcome, RecoveryReason, RecoveryStatus};
const CERTIFICATE: &str = "rtao_shield_absence_v1";
fn options(f: &Fixture) -> ShieldOptions {
    let mut options = f.options();
    options.dropped_inner_retries = 1;
    options
}
async fn missing_inner() -> Fixture {
    let f = Fixture::new(false).await;
    assert!(matches!(
        f.client
            .submit_shielded_once(f.call().await, &f.signer, &options(&f))
            .await,
        Err(SubmitFailure::PendingShield { .. })
    ));
    f
}
async fn unqualified_scan(f: &Fixture) {
    let plain = f.journal.bind(f.binding.id()).unwrap();
    assert!(matches!(
        plain.recover(&f.client, 128).await.unwrap(),
        RecoveryStatus::Unresolved(RecoveryReason::ShieldAdapterRequired)
    ));
    let record = plain.record().unwrap();
    assert!(record.attempts[0].next > record.attempts[0].expiry.unwrap());
    assert_eq!(record.attempts[0].outcome, AttemptOutcome::Unresolved);
}
fn body_reads(f: &Fixture) -> usize {
    f.node
        .requests()
        .iter()
        .filter(|r| r.method == "chain_getBlock")
        .count()
}
fn author_reads(f: &Fixture) -> usize {
    f.node
        .requests()
        .iter()
        .filter(|r| r.method.starts_with("author_submit"))
        .count()
}

#[tokio::test]
async fn qualified_negative_exact_window_budget_repeated_recovery_and_private_reseal_keep_original_authority()
 {
    let mut f = missing_inner().await;
    unqualified_scan(&f).await;
    let before = body_reads(&f);
    let result = f.binding.recover(&f.client, 4).await.unwrap();
    assert!(
        matches!(
            &result,
            RecoveryStatus::Unresolved(RecoveryReason::ShieldQualificationBudget { required: 5 })
        ),
        "{result:?}"
    );
    assert_eq!(body_reads(&f), before);
    let result = f.binding.recover(&f.client, 5).await.unwrap();
    assert!(matches!(&result, RecoveryStatus::Rebuildable), "{result:?}");
    assert_eq!(
        body_reads(&f) - before,
        5,
        "one body covers both exact signed hashes"
    );
    let record = f.binding.record().unwrap();
    assert_eq!(
        record.attempts[0].outcome,
        AttemptOutcome::ExpiredUnexecuted
    );
    assert!(matches!(
        record.attempts[1].outcome,
        AttemptOutcome::Finalized { .. }
    ));
    let proof = record.shield_pairs[0].extensions[CERTIFICATE].clone();
    f.restart().await;
    assert_eq!(
        f.binding.record().unwrap().shield_pairs[0].extensions[CERTIFICATE],
        proof
    );
    for _ in 0..2 {
        assert!(matches!(
            f.binding.recover(&f.client, 0).await.unwrap(),
            RecoveryStatus::Rebuildable
        ));
    }
    assert_eq!(body_reads(&f) - before, 5);
    assert_eq!(author_reads(&f), 1);
    // Evidence is append-only even though it resides in a forward-compatible
    // schema3 extension. Neither deletion nor replacement can mint a new send.
    assert!(
        f.journal
            .update(|image| {
                image
                    .operations
                    .get_mut(f.binding.id())
                    .unwrap()
                    .shield_pairs[0]
                    .extensions
                    .remove(CERTIFICATE);
                Ok(())
            })
            .is_err()
    );
    assert_eq!(
        f.binding.record().unwrap().shield_pairs[0].extensions[CERTIFICATE],
        proof
    );
    let mut enlarged = options(&f);
    enlarged.dropped_inner_retries = 2;
    assert!(
        f.client
            .submit_shielded_once(f.call().await, &f.signer, &enlarged)
            .await
            .is_err()
    );
    assert_eq!(author_reads(&f), 1);
    install_inner_history(&f.node, f.binding.clone());
    let mut resumed = options(&f);
    resumed.submit.journal = Some(
        f.journal
            .bind(f.binding.id())
            .unwrap()
            .with_shield_protocol(Arc::new(TestOnlyProtocol {
                genesis: f.client.genesis(),
            })),
    );
    let report = f
        .client
        .submit_shielded_once(f.call().await, &f.signer, &resumed)
        .await
        .unwrap();
    let record = f.binding.record().unwrap();
    assert_eq!(record.shield_pairs.len(), 2);
    assert_eq!(record.shield_pairs[0].extensions[CERTIFICATE], proof);
    assert_eq!(
        record.attempt(record.shield_pairs[1].inner).unwrap().hash,
        report.hash
    );
    assert_eq!(author_reads(&f), 2);
    // Further calls return the exact historical effect without spending another
    // pair or sending a third carrier, including after policy-budget exhaustion.
    let repeated = f
        .client
        .submit_shielded_once(f.call().await, &f.signer, &resumed)
        .await
        .unwrap();
    assert_eq!(repeated.hash, report.hash);
    assert_eq!(repeated.shield, report.shield);
    assert_eq!(author_reads(&f), 2);
    f.client.close().await;
}

#[tokio::test]
async fn qualified_negative_rechecks_old_cursors_unknown_parent_runtime_and_missing_body_then_recovers()
 {
    for missing_body in [false, true] {
        let f = missing_inner().await;
        unqualified_scan(&f).await;
        let start = f.binding.record().unwrap().attempts[0].start;
        let target = f
            .node
            .mutate_state(|state| state.block_hash(start + 2))
            .unwrap();
        let bad = Arc::new(std::sync::atomic::AtomicBool::new(true));
        let enabled = bad.clone();
        f.node.mutate_state(|state| {
            if missing_body {
                state.set_handler("chain_getBlock", move |state, request| {
                    if enabled.load(Ordering::SeqCst) && request.params[0] == target {
                        rtao_testkit::Reply::result(json!({"block":{"extrinsics":null}}))
                    } else {
                        state.dispatch(request)
                    }
                });
            } else {
                state.set_handler("state_getStorageHash", move |state, request| {
                    if enabled.load(Ordering::SeqCst) && request.params[1] == target {
                        rtao_testkit::Reply::result(json!(crate::hash_hex(H256([43; 32]))))
                    } else {
                        state.dispatch(request)
                    }
                });
            }
        });
        let result = f.binding.recover(&f.client, 32).await.unwrap();
        if missing_body {
            assert!(matches!(
                result,
                RecoveryStatus::Unresolved(RecoveryReason::HistoryUnavailable { .. })
            ));
        } else {
            assert!(
                matches!(
                    &result,
                    RecoveryStatus::Unresolved(RecoveryReason::ShieldProtocolUnverified {
                        attempt: 0
                    })
                ),
                "{result:?}"
            );
        }
        let record = f.binding.record().unwrap();
        assert_eq!(record.attempts[0].outcome, AttemptOutcome::Unresolved);
        assert!(!record.shield_pairs[0].extensions.contains_key(CERTIFICATE));
        if !missing_body {
            assert_eq!(
                record.incompatibility.as_ref().unwrap().code,
                "synthetic-profile-refused"
            );
        }
        assert_eq!(author_reads(&f), 1);
        bad.store(false, Ordering::SeqCst);
        assert!(matches!(
            f.binding.recover(&f.client, 32).await.unwrap(),
            RecoveryStatus::Rebuildable
        ));
        assert_eq!(author_reads(&f), 1);
        f.client.close().await;
    }
}

#[tokio::test]
async fn qualified_negative_refuses_imported_noncanonical_inner_and_retains_exact_evidence() {
    for alter_era in [false, true] {
        let f = missing_inner().await;
        let mut image = f.journal.snapshot().unwrap();
        let record = image.operations.get_mut(f.binding.id()).unwrap();
        let inner = &mut record.attempts[0];
        if alter_era {
            inner.era["current"] = json!(inner.start + 1);
        } else {
            inner.bytes.push(0);
            inner.hash = H256(
                rtao_core::codec::storage::hash_param("Blake2_256", &inner.bytes)
                    .unwrap()
                    .try_into()
                    .unwrap(),
            );
        }
        let expected = inner.clone();
        let path = f.root.join("imported-noncanonical.json");
        let original = serde_json::to_vec(&image).unwrap();
        let mut file = std::fs::OpenOptions::new();
        file.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            file.mode(0o600);
        }
        use std::io::Write;
        file.open(&path).unwrap().write_all(&original).unwrap();
        let imported = OperationJournal::open(&path).unwrap();
        let binding = imported
            .bind(f.binding.id())
            .unwrap()
            .with_shield_protocol(Arc::new(TestOnlyProtocol {
                genesis: f.client.genesis(),
            }));
        assert!(matches!(
            binding.recover(&f.client, 128).await.unwrap(),
            RecoveryStatus::Unresolved(RecoveryReason::ShieldCanonicalEncodingRequired {
                attempt: 0
            })
        ));
        let retained = binding.record().unwrap();
        assert_eq!(retained.attempts[0].bytes, expected.bytes);
        assert_eq!(retained.attempts[0].era, expected.era);
        assert_eq!(retained.attempts[0].outcome, AttemptOutcome::Unresolved);
        assert!(
            !retained.shield_pairs[0]
                .extensions
                .contains_key(CERTIFICATE)
        );
        assert_eq!(author_reads(&f), 1);
        drop(binding);
        drop(imported);
        f.client.close().await;
    }
}

#[tokio::test]
async fn qualified_negative_late_conflicting_inner_retains_both_evidence_and_blocks_reseal() {
    let f = missing_inner().await;
    let result = f.binding.recover(&f.client, 128).await.unwrap();
    assert!(matches!(&result, RecoveryStatus::Rebuildable), "{result:?}");
    let record = f.binding.record().unwrap();
    let inner = record.attempts[0].hash;
    let AttemptOutcome::Finalized { block, .. } = record.attempts[1].outcome else {
        panic!("carrier receipt")
    };
    install_inner_history(&f.node, f.binding.clone());
    // Deliberately contradictory synthetic history models a late receipt from
    // a stale/dishonest history source. Never discard the earlier certificate.
    let late = f
        .client
        .resolve_extrinsic(inner, block, true)
        .await
        .unwrap();
    f.binding.observed(&late).await.unwrap();
    assert!(matches!(
        f.binding.recover(&f.client, 128).await.unwrap(),
        RecoveryStatus::Unresolved(RecoveryReason::EvidenceConflict)
    ));
    let record = f.binding.record().unwrap();
    assert!(record.shield_pairs[0].extensions.contains_key(CERTIFICATE));
    assert_eq!(
        record.attempts[0].outcome,
        AttemptOutcome::ExpiredUnexecuted
    );
    assert!(matches!(
        record.attempts[0].evidence_conflicts.as_slice(),
        [AttemptOutcome::Finalized { .. }]
    ));
    assert!(
        f.client
            .submit_shielded_once(f.call().await, &f.signer, &options(&f))
            .await
            .is_err()
    );
    assert_eq!(author_reads(&f), 1);
    f.client.close().await;
}

#[tokio::test]
async fn qualified_negative_restart_pause_and_guard_reinstallation_never_reauthorize_implicitly() {
    let mut f = missing_inner().await;
    f.restart().await;
    f.journal
        .control(crate::journal::JournalControl {
            submissions_paused: true,
            allow_obligations: false,
            extensions: Default::default(),
        })
        .unwrap();
    assert!(matches!(
        f.binding.recover(&f.client, 128).await.unwrap(),
        RecoveryStatus::Rebuildable
    ));
    let proof = f.binding.record().unwrap().shield_pairs[0].extensions[CERTIFICATE].clone();
    let plain = f.journal.bind(f.binding.id()).unwrap();
    assert!(matches!(
        plain.recover(&f.client, 0).await.unwrap(),
        RecoveryStatus::Unresolved(RecoveryReason::ShieldAdapterRequired)
    ));
    let revoked = plain.with_shield_protocol(Arc::new(TestOnlyProtocol {
        genesis: H256([0; 32]),
    }));
    assert!(matches!(
        revoked.recover(&f.client, 0).await.unwrap(),
        RecoveryStatus::Unresolved(RecoveryReason::ShieldProtocolUnverified { .. })
    ));
    assert_eq!(
        f.binding.record().unwrap().shield_pairs[0].extensions[CERTIFICATE],
        proof
    );
    let signer = CountingSigner {
        inner: KeypairSigner::from_seed(&[7; 32], CryptoType::Ed25519).unwrap(),
        prepares: AtomicU64::new(0),
        signs: AtomicU64::new(0),
    };
    assert!(
        f.client
            .submit_shielded_once(f.call().await, &signer, &options(&f))
            .await
            .is_err()
    );
    assert_eq!(signer.prepares.load(Ordering::SeqCst), 0);
    assert_eq!(signer.signs.load(Ordering::SeqCst), 0);
    assert_eq!(author_reads(&f), 1);
    f.journal.control(Default::default()).unwrap();
    install_inner_history(&f.node, f.binding.clone());
    let report = f
        .client
        .submit_shielded_once(f.call().await, &signer, &options(&f))
        .await
        .unwrap();
    assert!(report.success);
    assert_eq!(author_reads(&f), 2);
    assert!(f.binding.record().unwrap().incompatibility.is_none());
    f.client.close().await;
}

#[tokio::test]
async fn qualified_negative_total_pair_limit_and_finalized_effect_do_not_require_new_signing_authority()
 {
    let f = Fixture::new(true).await;
    let signer = CountingSigner {
        inner: KeypairSigner::from_seed(&[7; 32], CryptoType::Ed25519).unwrap(),
        prepares: AtomicU64::new(0),
        signs: AtomicU64::new(0),
    };
    let mut too_large = options(&f);
    too_large.dropped_inner_retries = 16;
    assert!(
        f.client
            .submit_shielded_once(f.call().await, &signer, &too_large)
            .await
            .is_err()
    );
    assert_eq!(signer.signs.load(Ordering::SeqCst), 0);
    assert_eq!(signer.prepares.load(Ordering::SeqCst), 0);
    assert_eq!(author_reads(&f), 0);
    let call = f.call().await;
    let report = f
        .client
        .submit_shielded_once(call.clone(), &signer, &options(&f))
        .await
        .unwrap();
    let signs = signer.signs.load(Ordering::SeqCst);
    // The current runtime is now outside the installed trusted signing profile.
    // Historical finalized effects are still observable without a new signer
    // invocation or asserting the new runtime is economically compatible.
    f.node.mutate_state(|state| {
        state.set_result(
            "state_getStorageHash",
            json!(crate::hash_hex(H256([43; 32]))),
        )
    });
    let recovered = f
        .client
        .submit_shielded_once(call, &signer, &options(&f))
        .await
        .unwrap();
    assert_eq!(recovered.hash, report.hash);
    assert_eq!(recovered.shield, report.shield);
    assert_eq!(signer.signs.load(Ordering::SeqCst), signs);
    assert_eq!(author_reads(&f), 1);
    f.client.close().await;
}

struct FailCertificatePublication {
    inner: crate::journal::NativeJournalStore,
    fail: std::sync::atomic::AtomicBool,
    after_publication: bool,
}
impl crate::journal::JournalStore for FailCertificatePublication {
    fn claim_scope(
        &self,
        scope: &str,
    ) -> Result<Box<dyn crate::journal::JournalLease>, JournalError> {
        self.inner.claim_scope(scope)
    }
    fn load(&self) -> Result<crate::journal::JournalImage, JournalError> {
        self.inner.load()
    }
    fn compare_exchange(
        &self,
        expected: u64,
        image: &crate::journal::JournalImage,
    ) -> Result<(), JournalError> {
        let certificate = image.operations.values().any(|r| {
            r.shield_pairs
                .iter()
                .any(|p| p.extensions.contains_key(CERTIFICATE))
        });
        if certificate && self.fail.swap(false, Ordering::SeqCst) {
            if self.after_publication {
                self.inner.compare_exchange(expected, image)?;
            }
            return Err(JournalError::Storage(
                "injected certificate publication interruption".into(),
            ));
        }
        self.inner.compare_exchange(expected, image)
    }
}

#[tokio::test]
async fn qualified_negative_certificate_interrupted_publication_reopens_conservatively_before_and_after_commit()
 {
    for after_publication in [false, true] {
        let f = missing_inner().await;
        let mut image = f.journal.snapshot().unwrap();
        image.extensions.insert(
            "preserved-protocol-evidence".into(),
            json!({"unknown":"retained"}),
        );
        let path = f.root.join("interrupted-certificate.json");
        let mut file = std::fs::OpenOptions::new();
        file.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            file.mode(0o600);
        }
        use std::io::Write;
        file.open(&path)
            .unwrap()
            .write_all(&serde_json::to_vec(&image).unwrap())
            .unwrap();
        let store = Arc::new(FailCertificatePublication {
            inner: crate::journal::NativeJournalStore::open(&path).unwrap(),
            fail: std::sync::atomic::AtomicBool::new(true),
            after_publication,
        });
        let journal = OperationJournal::new(store.clone()).unwrap();
        let binding = journal
            .bind(f.binding.id())
            .unwrap()
            .with_shield_protocol(Arc::new(TestOnlyProtocol {
                genesis: f.client.genesis(),
            }));
        assert!(matches!(
            binding.recover(&f.client, 128).await,
            Err(JournalError::Storage(_))
        ));
        let record = binding.record().unwrap();
        assert_eq!(
            record.shield_pairs[0].extensions.contains_key(CERTIFICATE),
            after_publication
        );
        assert_eq!(
            record.attempts[0].outcome,
            if after_publication {
                AttemptOutcome::ExpiredUnexecuted
            } else {
                AttemptOutcome::Unresolved
            }
        );
        assert_eq!(author_reads(&f), 1);
        drop(binding);
        drop(journal);
        drop(store);
        let journal = OperationJournal::open(&path).unwrap();
        assert_eq!(
            journal.snapshot().unwrap().extensions["preserved-protocol-evidence"],
            json!({"unknown":"retained"})
        );
        let binding = journal
            .bind(f.binding.id())
            .unwrap()
            .with_shield_protocol(Arc::new(TestOnlyProtocol {
                genesis: f.client.genesis(),
            }));
        assert!(matches!(
            binding.recover(&f.client, 128).await.unwrap(),
            RecoveryStatus::Rebuildable
        ));
        assert_eq!(
            binding.record().unwrap().shield_pairs[0].extensions[CERTIFICATE]["version"],
            1
        );
        assert_eq!(author_reads(&f), 1);
        drop(binding);
        drop(journal);
        f.client.close().await;
    }
}

#[tokio::test]
async fn qualified_negative_canonical_nested_call_uses_existing_runtime_encoder() {
    let f = Fixture::new(false).await;
    let runtime = f.client.refresh_runtime().await.unwrap();
    let call = crate::compose::batch_all(&runtime, vec![f.call().await]).unwrap();
    assert!(matches!(
        f.client
            .submit_shielded_once(call, &f.signer, &options(&f))
            .await,
        Err(SubmitFailure::PendingShield { .. })
    ));
    assert!(matches!(
        f.binding.recover(&f.client, 128).await.unwrap(),
        RecoveryStatus::Rebuildable
    ));
    assert_eq!(author_reads(&f), 1);
    f.client.close().await;
}

#[tokio::test]
async fn qualified_negative_exhaustion_preserves_certified_absence_and_intent_without_another_signature()
 {
    let f = Fixture::new(false).await;
    let signer = CountingSigner {
        inner: KeypairSigner::from_seed(&[7; 32], CryptoType::Ed25519).unwrap(),
        prepares: AtomicU64::new(0),
        signs: AtomicU64::new(0),
    };
    assert!(matches!(
        f.client
            .submit_shielded_once(f.call().await, &signer, &f.options())
            .await,
        Err(SubmitFailure::PendingShield { .. })
    ));
    assert!(matches!(
        f.binding.recover(&f.client, 128).await.unwrap(),
        RecoveryStatus::Unresolved(RecoveryReason::ShieldPairBudgetExhausted {
            maximum_pairs: 1,
            consumed: 1
        })
    ));
    let signs = signer.signs.load(Ordering::SeqCst);
    assert!(matches!(
        f.client
            .submit_shielded_once(f.call().await, &signer, &f.options())
            .await,
        Err(SubmitFailure::PendingShield {
            reason: RecoveryReason::ShieldPairBudgetExhausted {
                maximum_pairs: 1,
                consumed: 1
            },
            ..
        })
    ));
    assert_eq!(signer.signs.load(Ordering::SeqCst), signs);
    let record = f.binding.record().unwrap();
    assert_eq!(
        record.attempts[0].outcome,
        AttemptOutcome::ExpiredUnexecuted
    );
    assert!(record.shield_pairs[0].extensions.contains_key(CERTIFICATE));
    assert_eq!(
        record.intent.correlation_id.as_deref(),
        Some("application-revision")
    );
    assert_eq!(author_reads(&f), 1);
    f.client.close().await;
}
