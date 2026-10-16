// Derived from RaoFoundation/subtensor sdk/python/tests/unit/test_null_weights.py @ 850f2755bb05a3851f948489f0ab27b0645cebce (Apache-2.0); modified by rtao.

mod null_weights {
    use super::*;
    use crate::weights::{
        commit::{CommitError, PreparedWeightCommit, with_test_encryptor},
        conform, normalize,
    };
    use std::sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    };

    fn full_row() -> (Vec<Uid>, Vec<u16>) {
        let mut weights = vec![1; 2_500];
        weights[0] = 65_534;
        ((0..2_500).map(Uid).collect(), weights)
    }

    fn conform_state() -> ConformState {
        ConformState {
            uid: Uid(0),
            min_allowed_weights: 1,
            max_weight_limit: u16::MAX,
        }
    }

    // pyt: sdk/python/tests/unit/test_null_weights.py::test_raw_full_row_preserves_small_weights
    #[test]
    fn raw_full_row_preserves_small_weights() {
        let (uids, weights) = full_row();
        let values = weights.iter().copied().map(f64::from).collect::<Vec<_>>();
        assert_eq!(
            conform(&uids, &values, conform_state(), NetUid(1), true).unwrap(),
            (uids, weights)
        );
    }

    // pyt: sdk/python/tests/unit/test_null_weights.py::test_raw_rejects_invalid_values
    #[test]
    fn raw_rejects_all_five_original_invalid_values() {
        for bad in [-1.0, 65_536.0, 1.5, f64::INFINITY, f64::NAN] {
            assert!(conform(&[Uid(1)], &[bad], conform_state(), NetUid(1), true).is_err());
        }
    }

    // pyt: sdk/python/tests/unit/test_null_weights.py::test_raw_checks_limit_without_clipping
    #[test]
    fn raw_checks_limit_without_clipping() {
        let state = ConformState {
            max_weight_limit: 32_767,
            ..conform_state()
        };
        let error = conform(&[Uid(1), Uid(2)], &[100.0, 1.0], state, NetUid(1), true).unwrap_err();
        assert!(error.to_string().contains("max_weight_limit"));
        assert_eq!(
            normalize(&[Uid(1), Uid(2)], &[3.0, 1.0]).unwrap(),
            (vec![Uid(1), Uid(2)], vec![65_535, 21_845])
        );
    }

    // pyt: sdk/python/tests/unit/test_null_weights.py::test_raw_requires_null_consensus
    #[tokio::test]
    async fn raw_requires_null_consensus() {
        // Typed runtime enums replace the Python mock's string return values.
        let (yuma, _) = fixture(false, false).await;
        assert!(
            ensure_raw_null(&yuma, NETUID)
                .await
                .unwrap_err()
                .to_string()
                .contains("Null")
        );
        let (null, _) = fixture(true, false).await;
        ensure_raw_null(&null, NETUID).await.unwrap();
    }

    // pyt: sdk/python/tests/unit/test_null_weights.py::test_owner_consensus_intent_uses_enum
    #[cfg(feature = "hyperparameters")]
    #[tokio::test]
    async fn owner_consensus_intent_uses_enum_and_rejects_numeric_two() {
        let (snapshot, _) = fixture(true, false).await;
        let intent = crate::SetHyperparameter {
            netuid: NetUid(1),
            name: "epoch_consensus".into(),
            value: json!("Null"),
        };
        let built = build(&intent, &snapshot).await.unwrap();
        // Exact typed call bytes cover the module, function and complete params.
        let expected = rtao_runtime::calls::admin_utils::sudo_set_epoch_consensus(
            NetUid(1),
            rtao_core::DynValue::new(Value::Str("Null".into())),
        )
        .compose(&View::runtime(&snapshot).await.unwrap())
        .unwrap();
        assert_eq!(built.call.as_bytes(), expected.as_bytes());
        // Native checked tool decoding replaces Python constructor validation.
        let error = crate::AnyIntent::from_tool(
            "set_hyperparameter",
            json!({"netuid":1,"name":"epoch_consensus","value":2}),
        )
        .unwrap_err();
        assert!(error.to_string().contains("Yuma or Null"));
    }

    // pyt: sdk/python/tests/unit/test_null_weights.py::test_full_row_intent_composes_exact_values
    #[tokio::test]
    async fn full_row_intent_composes_exact_values_in_both_branches() {
        let (uids, weights) = full_row();
        let values = weights.iter().copied().map(f64::from).collect::<Vec<_>>();
        for commit_reveal in [false, true] {
            // The shared fixture uses netuid 3, mechanism 1 and validator UID 2.
            let (snapshot, _) = fixture(true, commit_reveal).await;
            let mut intent = input(&values);
            intent.raw_u16 = true;
            let encrypted = Arc::new(Mutex::new(Vec::new()));
            let observed = encrypted.clone();
            let built = with_test_encryptor(
                move |request| {
                    observed.lock().unwrap().push(request);
                    Ok(PreparedWeightCommit {
                        ciphertext: vec![0; 17_000],
                        reveal_round: 1_000,
                    })
                },
                build(&intent, &snapshot),
            )
            .await
            .unwrap();
            let runtime = View::runtime(&snapshot).await.unwrap();
            let encrypted = encrypted.lock().unwrap();
            let expected = if commit_reveal {
                assert_eq!(encrypted.len(), 1);
                assert_eq!(encrypted[0].row, (uids.clone(), weights.clone()));
                calls::commit_timelocked_mechanism_weights(
                    NETUID,
                    MECHID,
                    vec![0; 17_000],
                    1_000,
                    4,
                )
            } else {
                assert!(encrypted.is_empty());
                calls::set_mechanism_weights(
                    NETUID,
                    MECHID,
                    uids.iter().map(|uid| uid.0).collect(),
                    weights.clone(),
                    123,
                )
            }
            .compose(&runtime)
            .unwrap();
            assert_eq!(built.call.as_bytes(), expected.as_bytes());
        }
    }

    // pyt: sdk/python/tests/unit/test_null_weights.py::test_null_nonpermit_rejected_before_composing_or_encrypting
    #[tokio::test]
    async fn null_nonpermit_rejected_before_composing_or_encrypting() {
        let (snapshot, fake) = fixture(true, true).await;
        fake.mutate_state(|state| {
            state
                .seed_typed(
                    "SubtensorModule",
                    "ValidatorPermit",
                    (NETUID,),
                    vec![true, false, false],
                )
                .unwrap();
        });
        let mut intent = input(&[1.0]);
        intent.uids = None;
        intent.weights = Some(WeightsArg::Map(
            [("0".into(), Ratio::new(1.0).unwrap())].into(),
        ));
        intent.raw_u16 = true;
        let encryptions = Arc::new(AtomicUsize::new(0));
        let observed = encryptions.clone();
        let error = with_test_encryptor(
            move |_| {
                observed.fetch_add(1, Ordering::Relaxed);
                Err(CommitError::InvalidMechanismCount)
            },
            build(&intent, &snapshot),
        )
        .await
        .unwrap_err();
        assert!(matches!(&error, IntentError::Chain(_)));
        assert_eq!(error.code(), ErrorCode::NotAuthorized);
        assert_eq!(encryptions.load(Ordering::Relaxed), 0);
    }
}
