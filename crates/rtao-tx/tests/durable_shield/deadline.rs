//! Causal final live-head lookup after successful pinned qualification.
use super::*;

#[tokio::test(start_paused = true)]
async fn final_live_signing_readiness_obeys_both_deadlines_and_releases_owner() {
    // Real socket progress must not auto-advance the virtual clock.
    let keepalive = tokio::spawn(async {
        loop {
            tokio::task::yield_now().await;
        }
    });
    for (full, admission) in [(3, 1), (1, 3)] {
        let mut lab = JournalLab::new(FakeState::default(), path(), CryptoType::Ed25519)
            .await
            .unwrap();
        // Head tracking is off. Expire the actual live-head cache while leaving
        // the pinned version/metadata qualification path available.
        tokio::time::advance(Duration::from_secs(13)).await;
        let entered = Arc::new(tokio::sync::Notify::new());
        let signal = entered.clone();
        let pinned = Arc::new(AtomicU64::new(0));
        let seen = pinned.clone();
        lab.node.mutate_state(move |state| {
            state.set_handler("state_getRuntimeVersion", move |state, request| {
                if request.params.get(0).is_none_or(Value::is_null) {
                    signal.notify_one();
                    rtao_testkit::Reply::silent()
                } else {
                    seen.fetch_add(1, Ordering::SeqCst);
                    // The handler is temporarily removed during dispatch, so
                    // delegation uses the existing exact built-in pinned reply.
                    state.dispatch(request)
                }
            });
        });
        let binding = lab
            .begin(
                "bounded-signing",
                json!({"intent":"retained"}),
                json!({"material":1}),
            )
            .unwrap();
        let mut opts = rtao_client::ShieldOptions {
            period: ShieldPeriod::P4,
            wait: Wait::Finalization,
            submit: rtao_client::SubmitOptions {
                journal: Some(binding.clone()),
                deadline: Some(Duration::from_secs(full)),
                submit_deadline: Some(Duration::from_secs(admission)),
                ..Default::default()
            },
            ..Default::default()
        };
        let client = lab.client.clone();
        let task_opts = opts.clone();
        let mut task = tokio::spawn(async move { client.prepare_journal_shield(&task_opts).await });
        tokio::select! {
            _=entered.notified()=>{},
            early=&mut task=>panic!("ended before final unpinned request: {}",early.is_ok()),
        }
        assert!(
            pinned.load(Ordering::SeqCst) > 0,
            "pinned runtime qualification must precede the stalled live lookup"
        );
        assert!(
            binding.reserve().is_err(),
            "preparation owns the account while awaiting signing readiness"
        );
        tokio::time::advance(Duration::from_secs(full.min(admission))).await;
        // The old code remains pending at this exact causal deadline. Bound the
        // assertion by polling, then abort only the test-owned control task.
        for _ in 0..100 {
            if task.is_finished() {
                break;
            }
            tokio::task::yield_now().await;
        }
        let finished = task.is_finished();
        if !finished {
            task.abort();
        }
        let outcome = task.await;
        assert!(
            finished,
            "final ensure_signing escaped the original minimum deadline"
        );
        let error = match outcome.unwrap() {
            Ok(_) => panic!("stalled lookup became ready"),
            Err(error) => error,
        };
        assert!(
            matches!(
                error,
                rtao_client::SubmitFailure::Client(rtao_client::ClientError::Rpc(
                    rtao_rpc::RpcError::Timeout { .. }
                ))
            ),
            "{error:?}"
        );
        let owner = binding
            .reserve()
            .expect("deadline released preparation ownership");
        drop(owner);
        assert_eq!(lab.authors(), 0);
        assert_eq!(lab.signer.signatures(), 0);
        assert!(binding.record().unwrap().attempts.is_empty());
        assert_eq!(binding.record().unwrap().material, json!({"material":1}));
        opts.submit.journal = None;
        drop(opts);
        drop(binding);
        lab.close().await;
    }
    keepalive.abort();
}
