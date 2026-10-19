// Derived from RaoFoundation/subtensor sdk/python/bittensor/cli/context.py @ 850f2755bb05a3851f948489f0ab27b0645cebce (Apache-2.0); modified by rtao.
//! Shared mutation context through a real Client, SCALE metadata and executor.
#![allow(clippy::unwrap_used, clippy::expect_used)]
use rtao::{
    client::{Client, ClientConfig, HeadProvider, Network, View},
    core::value_ext::ValueExt,
    primitives::{AccountId, CryptoType},
    signer::{
        BoxFuture, Coldkey, DynKeySource, KeyView, KeypairSigner, Signer, SignerError, SignerRole,
    },
    tx::{
        BuildCx, Built, Intent, IntentError, IntentInterface, IntentPreflight, PreflightCx, Signed,
    },
};
use rtao_cli::{
    application::command_tree,
    books::Books,
    context::{AppContext, CommandReview, ShieldChoice, SubmissionOptions},
    globals::{Globals, Tier, with_globals},
    output::{MemorySink, Output, OutputOptions, ReviewSection, Stream},
    prompt::{PromptError, Terminal},
};
use rtao_testkit::{FakeChain, FakeState, rpc::RpcClient};
use serde_json::{Value, json};
use std::{
    collections::VecDeque,
    path::Path,
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
};
type TestResult = Result<(), Box<dyn std::error::Error>>;

struct RegistrationConfirmation {
    questions: Arc<Mutex<Vec<String>>>,
}
impl Terminal for RegistrationConfirmation {
    fn stdin_is_tty(&self) -> bool {
        true
    }
    fn stderr_is_tty(&self) -> bool {
        true
    }
    fn read_line(&mut self, question: &str) -> Result<String, PromptError> {
        self.questions
            .lock()
            .map_err(|_| PromptError::Unavailable)?
            .push(question.into());
        Ok("no".into())
    }
    fn read_secret(&mut self, _: &str) -> Result<rtao::wallet::SecretString, PromptError> {
        Err(PromptError::Unavailable)
    }
    fn message(&mut self, _: &str) -> Result<(), PromptError> {
        Ok(())
    }
}

#[tokio::test]
async fn live_registration_review_discloses_exact_burn_lock_and_root_recycling_before_unlock()
-> TestResult {
    for (op, cost, share, suffix, card_cost, note) in [
        (
            "burned_register",
            65_535u64,
            1u16,
            "burn τ0.000065534 · lock τ0.000000001",
            "τ0.000065534 burned + τ0.000000001 locked as collateral",
            "lock τ0.000000001 as miner collateral",
        ),
        (
            "burned_register",
            1_000_000_000,
            0,
            "burn τ1.000000000 · lock none",
            "τ1.000000000 burned",
            "lock none (full cost is burned)",
        ),
        (
            "root_register",
            2_000_000_000,
            u16::MAX,
            "burn τ2.000000000 · lock none",
            "τ2.000000000 recycled",
            "burn τ2.000000000 (recycled into issuance)",
        ),
    ] {
        let (client, chain, keys) = fixture(u64::MAX).await?;
        let netuid = if op == "root_register" { 0 } else { 3 };
        chain.mutate_state(|state| -> TestResult {
            state.seed_python("SubtensorModule", "Burn", &[json!(netuid)], json!(cost))?;
            state.seed_python(
                "SubtensorModule",
                "CollateralLockShare",
                &[json!(netuid)],
                json!(share),
            )?;
            state.seed_python("MevShield", "NextKey", &[], json!(vec![171u8; 1184]))?;
            Ok(())
        })?;
        let arguments = if op == "root_register" {
            json!({"hotkey_ss58": keys.key.account()})
        } else {
            json!({"netuid": netuid, "hotkey_ss58": keys.key.account()})
        };
        let intent = rtao_intents::AnyIntent::from_tool(op, arguments)?;
        let (mut ctx, sink, _) = context(&[])?;
        let questions = Arc::new(Mutex::new(Vec::new()));
        ctx.terminal = Arc::new(Mutex::new(Box::new(RegistrationConfirmation {
            questions: questions.clone(),
        })));
        let error = ctx
            .submit_with(&client, intent.as_dyn(), &keys, false, None)
            .await
            .err()
            .ok_or("confirmation must refuse")?;
        assert_eq!(error.message, "aborted.");
        {
            let questions = questions.lock().map_err(|_| "questions")?;
            assert_eq!(questions.len(), 1);
            assert!(questions[0].contains(&format!(" — {suffix}?")));
        }
        let card = sink.text(Stream::Stdout)?;
        assert!(card.contains(&format!(" — {suffix}")));
        assert!(card.contains(card_cost));
        assert!(sink.text(Stream::Stderr)?.contains(note));
        assert_eq!(keys.unlocks.load(Ordering::SeqCst), 0);
        assert!(chain.mutate_state(|state| state.submissions.is_empty()));
        client.close().await;
    }
    Ok(())
}

#[tokio::test]
async fn dry_run_registration_keeps_the_sdk_summary_and_machine_schema() -> TestResult {
    for op in ["root_register", "register_subnet"] {
        let (client, chain, keys) = fixture(u64::MAX).await?;
        chain.mutate_state(|state| {
            state.seed_python(
                "SubtensorModule",
                "Burn",
                &[json!(0)],
                json!(2_000_000_000u64),
            )
        })?;
        let intent =
            rtao_intents::AnyIntent::from_tool(op, json!({"hotkey_ss58": keys.key.account()}))?;
        let (ctx, sink, reads) = context(&["--dry-run", "--json", "--no-mev-shield"])?;
        assert!(
            ctx.submit_with(&client, intent.as_dyn(), &keys, false, None)
                .await?
                .is_none()
        );
        let payload: Value = serde_json::from_str(&sink.text(Stream::Stdout)?)?;
        assert_eq!(payload["summary"], intent.as_dyn().summary_dyn());
        assert!(payload.get("cost").is_none());
        assert!(sink.text(Stream::Stderr)?.is_empty());
        assert_eq!(reads.load(Ordering::SeqCst), 0);
        assert_eq!(keys.unlocks.load(Ordering::SeqCst), 0);
        assert!(chain.mutate_state(|state| state.submissions.is_empty()));
        assert!(
            !chain
                .requests()
                .iter()
                .any(|request| request.method == "state_call"
                    && request.params[0]
                        == "SubnetRegistrationRuntimeApi_get_network_registration_cost")
        );
        client.close().await;
    }
    Ok(())
}

#[tokio::test]
async fn subnet_creation_review_quotes_exact_cost_and_all_queue_flows_before_unlock() -> TestResult
{
    for (limit, cleanup, registration_waiting, prune, flow, probes_prune) in [
        (
            2,
            vec![],
            false,
            Some(7),
            "immediate · no deregistration needed",
            false,
        ),
        (
            2,
            vec![9],
            false,
            Some(7),
            "queued · waits for deregistration cleanup of subnet 9",
            false,
        ),
        (
            2,
            vec![9],
            true,
            Some(7),
            "queued · deregisters subnet 7 before registration",
            true,
        ),
        (
            1,
            vec![],
            false,
            None,
            "blocked · no subnet is eligible for deregistration",
            true,
        ),
    ] {
        let (client, chain, keys) = fixture(u64::MAX).await?;
        chain.mutate_state(|state| -> TestResult {
            state.seed_runtime(
                "SubnetRegistrationRuntimeApi",
                "get_network_registration_cost",
                json!(1_234_567_890_123u64),
            )?;
            state.seed_runtime("SubnetInfoRuntimeApi", "get_subnet_to_prune", json!(prune))?;
            for (netuid, added) in [(0, true), (1, true), (2, false)] {
                state.seed_python(
                    "SubtensorModule",
                    "NetworksAdded",
                    &[json!(netuid)],
                    json!(added),
                )?;
            }
            state.seed_python("SubtensorModule", "SubnetLimit", &[], json!(limit))?;
            state.seed_python(
                "SubtensorModule",
                "DissolveCleanupQueue",
                &[],
                json!(cleanup),
            )?;
            state.seed_python(
                "SubtensorModule",
                "NetworkRegistrationQueue",
                &[],
                if registration_waiting {
                    json!([{"coldkey":keys.key.account(), "hotkey":keys.key.account()}])
                } else {
                    json!([])
                },
            )?;
            Ok(())
        })?;
        let intent = rtao_intents::RegisterSubnet { hotkey_ss58: None };
        let (mut ctx, sink, _) = context(&["--no-mev-shield"])?;
        let questions = Arc::new(Mutex::new(Vec::new()));
        ctx.terminal = Arc::new(Mutex::new(Box::new(RegistrationConfirmation {
            questions: questions.clone(),
        })));
        let request_start = chain.requests().len();
        let error = ctx
            .submit_with(&client, &intent, &keys, false, None)
            .await
            .err()
            .ok_or("confirmation must refuse")?;
        assert_eq!(error.message, "aborted.");
        assert_eq!(
            questions.lock().map_err(|_| "questions")?.as_slice(),
            ["register a new subnet for 1,234.567890123 TAO? [y/N]: "],
        );
        let card = sink.text(Stream::Stdout)?;
        assert!(card.contains("register a new subnet for 1,234.567890123 TAO"));
        assert!(card.contains("τ1,234.567890123"));
        assert!(sink.text(Stream::Stderr)?.contains(flow));
        assert_eq!(keys.unlocks.load(Ordering::SeqCst), 0);
        assert!(chain.mutate_state(|state| state.submissions.is_empty()));
        let requests = chain
            .requests()
            .into_iter()
            .skip(request_start)
            .collect::<Vec<_>>();
        let quote = requests
            .iter()
            .find(|request| {
                request.method == "state_call"
                    && request.params[0]
                        == "SubnetRegistrationRuntimeApi_get_network_registration_cost"
            })
            .ok_or("quote request")?;
        let pinned = &quote.params[2];
        assert!(!pinned.is_null());
        let prunes = requests
            .iter()
            .filter(|request| {
                request.method == "state_call"
                    && request.params[0] == "SubnetInfoRuntimeApi_get_subnet_to_prune"
            })
            .count();
        assert_eq!(prunes, usize::from(probes_prune));
        for request in &requests {
            // Registration disclosures use one state identity. Independent
            // metadata/fee probes can query the head without that state hash.
            let state_hash = match request.method.as_str() {
                "state_getStorage" | "state_queryStorageAt" => request.params.get(1),
                "state_getKeysPaged" => request.params.get(3),
                "state_call"
                    if matches!(
                        request.params[0].as_str(),
                        Some(
                            "SubnetRegistrationRuntimeApi_get_network_registration_cost"
                                | "SubnetInfoRuntimeApi_get_subnet_to_prune"
                        )
                    ) =>
                {
                    request.params.get(2)
                }
                _ => None,
            };
            if let Some(hash) = state_hash {
                assert_eq!(hash, pinned, "{}", request.method);
            }
        }
        client.close().await;
    }
    Ok(())
}

#[tokio::test]
async fn subnet_creation_success_keeps_compact_registration_fields_in_quiet_and_json() -> TestResult
{
    use rtao::primitives::{DefaultAccount, NetUid, TaoBalance};
    use rtao_runtime::storage::subtensor_module as st;

    for (quiet, json_mode, fail_progress) in [
        (true, false, false),
        (true, true, false),
        (false, false, false),
        (false, false, true),
    ] {
        let (client, chain, keys) = fixture(u64::MAX).await?;
        let netuid = NetUid(9);
        let hotkey = AccountId([5; 32]);
        let paid_rao = 1_234_567_890_123u64;
        assert_ne!(hotkey, keys.key.account());
        chain.mutate_state(|state| -> TestResult {
            // The receipt's authenticated lock cost must replace the earlier
            // review quote in the completed registration's human fields.
            state.seed_runtime(
                "SubnetRegistrationRuntimeApi",
                "get_network_registration_cost",
                json!(2_000_000_000u64),
            )?;
            state.seed(
                &st::SUBNET_OWNER,
                (netuid,),
                DefaultAccount::from(keys.key.account()),
            )?;
            state.seed(
                &st::SUBNET_OWNER_HOTKEY,
                (netuid,),
                DefaultAccount::from(hotkey),
            )?;
            state.seed(&st::SUBNET_LOCKED, (netuid,), TaoBalance(paid_rao))?;
            state.queue_submit(rtao_testkit::SubmitScript::InBlockThenFinalized {
                events: json!([
                    {
                        "phase":"ApplyExtrinsic", "extrinsic_idx":0,
                        "module_id":"SubtensorModule", "event_id":"NetworkAdded",
                        "attributes":[netuid.0, 0], "topics":[]
                    },
                    {
                        "phase":"ApplyExtrinsic", "extrinsic_idx":0,
                        "module_id":"System", "event_id":"ExtrinsicSuccess",
                        "attributes":{"dispatch_info":{
                            "weight":{"ref_time":1,"proof_size":0},
                            "class":"Normal", "pays_fee":"Yes"
                        }},
                        "topics":[]
                    }
                ]),
            });
            Ok(())
        })?;
        let intent = rtao_intents::RegisterSubnet {
            hotkey_ss58: Some(hotkey.into()),
        };
        let mut flags = vec!["--yes", "--no-mev-shield"];
        if quiet {
            flags.push("--quiet");
        }
        if json_mode {
            flags.push("--json");
        }
        let (mut ctx, sink, reads) = context(&flags)?;
        let progress_failed = Arc::new(tokio::sync::Notify::new());
        let execution = RegistrationProgressCompletion {
            progress_failed: progress_failed.clone(),
            finished: std::sync::atomic::AtomicBool::new(false),
            receipt: Mutex::new(None),
        };
        if fail_progress {
            ctx.output = Output::new(
                ctx.output.options.clone(),
                Arc::new(RegistrationProgressSink {
                    sink: sink.clone(),
                    progress_failed,
                }),
            );
        }
        let outcome = tokio::time::timeout(
            std::time::Duration::from_secs(10),
            ctx.submit_with_keys_options(
                &client,
                &intent,
                &keys,
                SubmissionOptions {
                    execution: fail_progress
                        .then_some(&execution as &dyn rtao_cli::context::SubmissionExecution),
                    hooks: fail_progress
                        .then_some(&execution as &dyn rtao_cli::context::SubmissionHooks),
                    ..Default::default()
                },
            ),
        )
        .await?;
        if fail_progress {
            let error = outcome.err().ok_or("progress pipe must fail")?;
            assert!(error.message.contains("registration progress pipe closed"));
            assert!(execution.finished.load(Ordering::SeqCst));
            let receipt = execution
                .receipt
                .lock()
                .map_err(|_| "progress receipt")?
                .clone()
                .ok_or("durable receipt after progress failure")?;
            assert!(receipt.success);
            assert_eq!(receipt.data["netuid"], 9);
            let help = error.help.as_deref().ok_or("known receipt help")?;
            assert!(
                help.contains(
                    receipt
                        .extrinsic_id
                        .as_deref()
                        .ok_or("known receipt identity")?
                )
            );
            assert!(help.contains("already submitted"));
            assert_eq!(keys.unlocks.load(Ordering::SeqCst), 1);
            assert_eq!(reads.load(Ordering::SeqCst), 0);
            assert_eq!(
                chain.state().lock().map_err(|_| "state")?.submissions.len(),
                1
            );
            client.close().await;
            continue;
        }
        let receipt = outcome?.ok_or("registration receipt")?;
        assert!(receipt.success, "{}", receipt.message);
        assert_eq!(receipt.message, "Subnet 9 registered.");
        assert_eq!(receipt.data["netuid"], 9);
        assert_eq!(receipt.data["registration_mode"], "immediate");
        assert_eq!(receipt.data["registration_price_rao"], paid_rao);
        assert_eq!(receipt.data["registered_at_block"], 101);
        assert_eq!(receipt.extrinsic_id.as_deref(), Some("101-0000"));
        assert!(
            receipt
                .events
                .iter()
                .any(|event| event.matches("SubtensorModule", "NetworkAdded"))
        );
        let stdout = sink.text(Stream::Stdout)?;
        if json_mode {
            let payload: Value = serde_json::from_str(&stdout)?;
            assert_eq!(payload, receipt.to_json());
        } else {
            assert!(stdout.contains("subnet 9 registered"));
            assert!(stdout.lines().any(|line| {
                line.trim_start()
                    .strip_prefix("netuid")
                    .is_some_and(|value| {
                        value.starts_with(' ') && value.split_whitespace().next() == Some("9")
                    })
            }));
            for (label, expected) in [
                ("registration", "immediate · no deregistration needed"),
                ("price", "τ1,234.567890123"),
                ("registered block", "101"),
                ("registration tx", "101-0000"),
            ] {
                assert!(
                    stdout.lines().any(|line| {
                        line.trim_start()
                            .strip_prefix(label)
                            .is_some_and(|value| value.starts_with(' ') && value.trim() == expected)
                    }),
                    "missing {label}={expected}: {stdout}"
                );
            }
            assert!(!stdout.contains("registration price rao"));
            if quiet {
                assert!(!stdout.contains("Transaction"));
            } else {
                // The ordinary pre-sign card is still present; the compact
                // registration receipt adds no second Transaction section.
                assert_eq!(
                    stdout.lines().filter(|line| *line == "Transaction").count(),
                    1
                );
            }
        }
        let stderr = sink.text(Stream::Stderr)?;
        if quiet || json_mode {
            assert!(stderr.is_empty());
        } else {
            assert_eq!(
                stderr
                    .matches("NetworkAdded · subnet 9 · block 101")
                    .count(),
                1
            );
        }
        assert_eq!(reads.load(Ordering::SeqCst), 0);
        assert_eq!(keys.unlocks.load(Ordering::SeqCst), 1);
        assert_eq!(
            chain.state().lock().map_err(|_| "state")?.submissions.len(),
            1
        );
        client.close().await;
    }
    Ok(())
}

struct RegistrationProgressSink {
    sink: MemorySink,
    progress_failed: Arc<tokio::sync::Notify>,
}
impl rtao_cli::output::OutputSink for RegistrationProgressSink {
    fn write(&self, stream: Stream, bytes: &[u8]) -> std::io::Result<()> {
        if stream == Stream::Stderr && String::from_utf8_lossy(bytes).contains("NetworkAdded") {
            self.progress_failed.notify_one();
            return Err(std::io::Error::other("registration progress pipe closed"));
        }
        rtao_cli::output::OutputSink::write(&self.sink, stream, bytes)
    }
}

struct RegistrationProgressCompletion {
    progress_failed: Arc<tokio::sync::Notify>,
    finished: std::sync::atomic::AtomicBool,
    receipt: Mutex<Option<rtao::tx::ExtrinsicResult>>,
}
impl rtao_cli::context::SubmissionExecution for RegistrationProgressCompletion {
    fn execute<'a>(
        &'a self,
        client: &'a Client,
        intent: &'a dyn rtao::tx::DynIntent,
        keys: &'a dyn DynKeySource,
        options: &'a rtao::tx::ExecuteOptions,
    ) -> BoxFuture<'a, Result<rtao::tx::ExtrinsicResult, rtao_cli::error::CliError>> {
        Box::pin(async move {
            if options.progress.is_none() {
                return Err(rtao_cli::error::CliError::failure(
                    "registration progress missing",
                ));
            }
            let receipt = rtao::Executor::new(client.clone())
                .execute_dyn(intent, keys, options)
                .await
                .map_err(|error| rtao_cli::error::CliError::failure(error.to_string()))?;
            // The actual SDK has queued NetworkAdded. The operation remains
            // pending until the failing sink observes it; it must be drained.
            self.progress_failed.notified().await;
            self.finished.store(true, Ordering::SeqCst);
            Ok(receipt)
        })
    }
}
impl rtao_cli::context::SubmissionHooks for RegistrationProgressCompletion {
    fn before_submit(&self) -> BoxFuture<'_, Result<(), rtao_cli::error::CliError>> {
        Box::pin(async { Ok(()) })
    }
    fn on_result<'a>(
        &'a self,
        receipt: &'a rtao::tx::ExtrinsicResult,
    ) -> BoxFuture<'a, Result<(), rtao_cli::error::CliError>> {
        Box::pin(async move {
            assert!(self.finished.load(Ordering::SeqCst));
            *self
                .receipt
                .lock()
                .map_err(|_| rtao_cli::error::CliError::failure("progress receipt lock"))? =
                Some(receipt.clone());
            Ok(())
        })
    }
}

struct StopRegistrationBeforeSigning;
impl rtao_cli::context::SubmissionHooks for StopRegistrationBeforeSigning {
    fn before_submit(&self) -> BoxFuture<'_, Result<(), rtao_cli::error::CliError>> {
        Box::pin(async {
            Err(rtao_cli::error::CliError::failure(
                "review finished sentinel",
            ))
        })
    }
    fn on_result<'a>(
        &'a self,
        _: &'a rtao::tx::ExtrinsicResult,
    ) -> BoxFuture<'a, Result<(), rtao_cli::error::CliError>> {
        Box::pin(async { Err(rtao_cli::error::CliError::failure("unexpected receipt")) })
    }
}

#[tokio::test]
async fn subnet_creation_assume_yes_keeps_the_quote_and_skips_queue_disclosure_reads() -> TestResult
{
    let (client, chain, keys) = fixture(u64::MAX).await?;
    let omitted_keys =
        chain.mutate_state(|state| -> Result<Vec<String>, Box<dyn std::error::Error>> {
            state.seed_runtime(
                "SubnetRegistrationRuntimeApi",
                "get_network_registration_cost",
                json!(u64::MAX),
            )?;
            [
                "SubnetLimit",
                "DissolveCleanupQueue",
                "NetworkRegistrationQueue",
            ]
            .into_iter()
            .map(|name| {
                let info = state
                    .runtime
                    .storage_entry("SubtensorModule", name)
                    .ok_or("preview storage")?;
                Ok(format!(
                    "0x{}",
                    hex::encode(state.runtime.storage_key(info, &[])?)
                ))
            })
            .collect()
        })?;
    let intent = rtao_intents::RegisterSubnet { hotkey_ss58: None };
    let (ctx, sink, reads) = context(&["--yes", "--no-mev-shield"])?;
    let request_start = chain.requests().len();
    let error = ctx
        .submit_with_hooks(
            &client,
            &intent,
            &keys,
            false,
            None,
            Some(&StopRegistrationBeforeSigning),
        )
        .await
        .err()
        .ok_or("review sentinel")?;
    assert_eq!(error.message, "review finished sentinel");
    let card = sink.text(Stream::Stdout)?;
    assert!(card.contains("register a new subnet for 18,446,744,073.709551615 TAO"));
    assert!(card.contains("cost"));
    assert!(sink.text(Stream::Stderr)?.is_empty());
    assert_eq!(reads.load(Ordering::SeqCst), 0);
    assert_eq!(keys.unlocks.load(Ordering::SeqCst), 0);
    assert!(chain.mutate_state(|state| state.submissions.is_empty()));
    let requests = chain
        .requests()
        .into_iter()
        .skip(request_start)
        .collect::<Vec<_>>();
    assert_eq!(
        requests
            .iter()
            .filter(|request| request.method == "state_call"
                && request.params[0]
                    == "SubnetRegistrationRuntimeApi_get_network_registration_cost")
            .count(),
        1
    );
    assert!(
        !requests
            .iter()
            .any(|request| request.method == "state_getKeysPaged"
                || (request.method == "state_call"
                    && request.params[0] == "SubnetInfoRuntimeApi_get_subnet_to_prune"))
    );
    for key in omitted_keys {
        assert!(!requests.iter().any(|request| {
            match request.method.as_str() {
                "state_getStorage" => request.params[0] == key,
                "state_queryStorageAt" => request.params[0]
                    .as_array()
                    .is_some_and(|keys| keys.iter().any(|item| item == &key)),
                _ => false,
            }
        }));
    }
    client.close().await;
    // The live cost belongs to the semantic registration, including when a
    // saved multisig supplies the outer approval. Stop before acquiring it.
    let member_keys = members()?;
    let (client, chain, _) = fixture(u64::MAX).await?;
    let group = saved_group(&client, &member_keys).await?;
    chain.mutate_state(|state| -> Result<(), rtao::core::CoreError> {
        state.seed_runtime(
            "SubnetRegistrationRuntimeApi",
            "get_network_registration_cost",
            json!(2_000_000_000u64),
        )?;
        for account in member_keys
            .iter()
            .map(|keys| keys.key.account())
            .chain([group.address])
        {
            state.seed_python(
                "System",
                "Account",
                &[json!(account)],
                json!({"data":{"free":u64::MAX}}),
            )?;
        }
        Ok(())
    })?;
    let (ctx, sink) = saved_context(&member_keys, &["--yes", "--no-mev-shield"], 1)?;
    let factory = factory(&member_keys);
    let intent = rtao_intents::RegisterSubnet {
        hotkey_ss58: Some(AccountId([50; 32]).into()),
    };
    let error = ctx
        .submit_saved_with_options(
            &client,
            &intent,
            SubmissionOptions {
                hooks: Some(&StopRegistrationBeforeSigning),
                ..Default::default()
            },
            &factory,
            &CallCache::default(),
        )
        .await
        .err()
        .ok_or("saved review sentinel")?;
    assert_eq!(error.message, "review finished sentinel");
    let card = sink.text(Stream::Stdout)?;
    let transaction = card
        .split_once("\nTransaction\n")
        .ok_or("Transaction review missing")?
        .1;
    assert!(
        transaction
            .lines()
            .any(|line| line.split_whitespace().eq(["cost", "τ2.000000000"]))
    );
    assert_eq!(factory.acquired.load(Ordering::SeqCst), 0);
    assert!(
        member_keys
            .iter()
            .all(|keys| keys.unlocks.load(Ordering::SeqCst) == 0)
    );
    assert!(chain.mutate_state(|state| state.submissions.is_empty()));
    assert!(
        chain
            .requests()
            .iter()
            .all(|request| !request.method.starts_with("author_"))
    );
    client.close().await;
    Ok(())
}

#[tokio::test]
async fn subnet_creation_unreadable_quote_refuses_before_confirmation_or_signing() -> TestResult {
    for flags in [vec!["--no-mev-shield"], vec!["--yes", "--no-mev-shield"]] {
        let (client, chain, keys) = fixture(u64::MAX).await?;
        chain.mutate_state(|state| {
            state.seed_runtime_encoded(
                "SubnetRegistrationRuntimeApi_get_network_registration_cost",
                vec![],
                vec![],
            );
        });
        let intent = rtao_intents::RegisterSubnet { hotkey_ss58: None };
        let (ctx, sink, reads) = context(&flags)?;
        let error = ctx
            .submit_with(&client, &intent, &keys, false, None)
            .await
            .err()
            .ok_or("unreadable quote must refuse")?;
        assert_eq!(error.code, 1);
        assert_ne!(error.message, "aborted.");
        assert!(!sink.text(Stream::Stdout)?.contains("Transaction"));
        assert_eq!(reads.load(Ordering::SeqCst), 0);
        assert_eq!(keys.unlocks.load(Ordering::SeqCst), 0);
        assert!(chain.mutate_state(|state| state.submissions.is_empty()));
        client.close().await;
    }
    Ok(())
}

#[derive(Default)]
struct RefuseReceiptSink(Mutex<Vec<u8>>);
impl rtao_cli::output::OutputSink for RefuseReceiptSink {
    fn write(&self, stream: Stream, bytes: &[u8]) -> std::io::Result<()> {
        if stream == Stream::Stdout {
            let value: Value = serde_json::from_slice(bytes).map_err(std::io::Error::other)?;
            if value.get("success").is_some() {
                *self
                    .0
                    .lock()
                    .map_err(|_| std::io::Error::other("sink poisoned"))? = bytes.to_vec();
                return Err(std::io::Error::new(
                    std::io::ErrorKind::BrokenPipe,
                    "receipt pipe closed",
                ));
            }
        }
        Ok(())
    }
}

#[tokio::test]
async fn direct_and_saved_receipt_output_failure_retains_known_identity_without_resubmission()
-> TestResult {
    for saved in [false, true] {
        let (client, chain, direct_keys) = fixture(1_000_000_000).await?;
        let saved_keys = members()?;
        let (mut ctx, _, _) = context(&["--yes", "--json", "--no-mev-shield"])?;
        if saved {
            ctx = saved_context(&saved_keys, &["--yes", "--json", "--no-mev-shield"], 1)?.0;
        }
        let sink = Arc::new(RefuseReceiptSink::default());
        ctx.output = Output::new(
            OutputOptions {
                json_mode: true,
                ..Default::default()
            },
            sink.clone(),
        );
        let result = if saved {
            ctx.submit_saved_with_options(
                &client,
                &Preview { blocked: false },
                Default::default(),
                &factory(&saved_keys),
                &CallCache::default(),
            )
            .await
        } else {
            ctx.submit_with_keys_options(
                &client,
                &Preview { blocked: false },
                &direct_keys,
                Default::default(),
            )
            .await
        };
        let error = result
            .err()
            .ok_or("receipt output must fail after submission")?;
        assert!(error.message.contains("receipt pipe closed"));
        let captured: Value = serde_json::from_slice(&sink.0.lock().map_err(|_| "sink")?)?;
        assert_eq!(captured["success"], true);
        let id = captured["extrinsic_id"]
            .as_str()
            .filter(|id| !id.is_empty())
            .ok_or("known receipt identity")?;
        let help = error.help.ok_or("known receipt recovery help")?;
        assert!(help.contains(id));
        assert!(help.contains("already submitted"));
        assert_eq!(
            chain.state().lock().map_err(|_| "state")?.submissions.len(),
            1
        );
        client.close().await;
    }
    Ok(())
}

struct TerminalFixture {
    tty: bool,
    lines: VecDeque<String>,
    reads: Arc<AtomicUsize>,
}
impl Terminal for TerminalFixture {
    fn stdin_is_tty(&self) -> bool {
        self.tty
    }
    fn stderr_is_tty(&self) -> bool {
        self.tty
    }
    fn read_line(&mut self, _: &str) -> Result<String, PromptError> {
        self.reads.fetch_add(1, Ordering::SeqCst);
        self.lines.pop_front().ok_or(PromptError::Aborted)
    }
    fn read_secret(&mut self, _: &str) -> Result<rtao::wallet::SecretString, PromptError> {
        Err(PromptError::Unavailable)
    }
    fn message(&mut self, _: &str) -> Result<(), PromptError> {
        Ok(())
    }
}
fn context(
    flags: &[&str],
) -> Result<(AppContext, MemorySink, Arc<AtomicUsize>), Box<dyn std::error::Error>> {
    let parsed = command_tree([with_globals(clap::Command::new("submit"), Tier::Tx)])
        .try_get_matches_from(
            std::iter::once("rtao")
                .chain(std::iter::once("submit"))
                .chain(flags.iter().copied()),
        )?;
    let mut globals = Globals::resolve(
        &parsed,
        &serde_json::Map::new(),
        Path::new("/isolated/home"),
        |_| None,
    )?;
    globals.apply(parsed.subcommand().ok_or("submit")?.1, &[])?;
    let reads = Arc::new(AtomicUsize::new(0));
    let terminal = TerminalFixture {
        tty: true,
        lines: VecDeque::from(["yes".into()]),
        reads: reads.clone(),
    };
    let mut ctx = AppContext::new(
        globals,
        serde_json::Map::new(),
        Books::default(),
        Arc::new(Mutex::new(Box::new(terminal))),
    );
    let sink = MemorySink::default();
    ctx.output = Output::new(
        OutputOptions {
            json_mode: ctx.globals.json,
            quiet: ctx.globals.quiet,
            ..Default::default()
        },
        Arc::new(sink.clone()),
    );
    Ok((ctx, sink, reads))
}
struct Keys {
    key: KeypairSigner,
    unlocks: Arc<AtomicUsize>,
}
impl KeyView for Keys {
    fn coldkey_address(&self) -> Result<AccountId, SignerError> {
        Ok(self.key.account())
    }
    fn hotkey_address(&self) -> Result<AccountId, SignerError> {
        Ok(self.key.account())
    }
    fn hotkey_public_key(&self) -> Result<[u8; 32], SignerError> {
        Ok(self.key.account().0)
    }
}
impl DynKeySource for Keys {
    fn signer_for<'a>(
        &'a self,
        _: SignerRole,
    ) -> BoxFuture<'a, Result<Arc<dyn Signer>, SignerError>> {
        Box::pin(async move {
            self.unlocks.fetch_add(1, Ordering::SeqCst);
            Ok(Arc::new(self.key.clone()) as Arc<dyn Signer>)
        })
    }
}
async fn fixture(free: u64) -> Result<(Client, FakeChain, Keys), Box<dyn std::error::Error>> {
    let keys = Keys {
        key: KeypairSigner::from_seed(&[19; 32], CryptoType::Ed25519)?,
        unlocks: Arc::new(AtomicUsize::new(0)),
    };
    let mut state = FakeState::default();
    for seed in 19u8..22 {
        state.seed_python(
            "System",
            "Account",
            &[json!(
                KeypairSigner::from_seed(&[seed; 32], CryptoType::Ed25519)?.account()
            )],
            json!({"data":{"free":free}}),
        )?;
    }
    let chain = FakeChain::from_state(state);
    let mut config = ClientConfig::new(Network::resolve_with("local", |_| None)?)?;
    config.track_heads = false;
    config.runtime_cache_dir = None;
    config.config_store = None;
    let client = Client::from_rpc(RpcClient::new(chain.clone()), config).await?;
    Ok((client, chain, keys))
}
// The real metadata/executor runs a remark under the catalog's default-shield
// interface. No staking operation or hosted oracle behavior is claimed here.
struct Preview {
    blocked: bool,
}
impl IntentInterface for Preview {
    const SPEC: &'static rtao_catalog::IntentSpec = &rtao_catalog::INTENTS[2];
    type Role = Coldkey;
    type Origin = Signed;
    type Output = ();
    const WRAPS: &'static [&'static rtao_runtime::CallDescriptor] =
        &[&rtao_runtime::calls::system::REMARK];
    fn to_json(&self) -> Value {
        json!({"op":"add_stake"})
    }
}
impl Intent for Preview {
    fn summary(&self) -> String {
        "review context fixture".into()
    }
    async fn build<V: View + ?Sized>(&self, cx: &BuildCx<'_, V>) -> Result<Built, IntentError> {
        Ok(Built::new(
            rtao_runtime::calls::system::remark(vec![8, 9]).compose(cx.runtime)?,
        ))
    }
    async fn preflight<V: View + ?Sized>(
        &self,
        _: &PreflightCx<'_, V>,
    ) -> Result<IntentPreflight, IntentError> {
        Ok(IntentPreflight {
            warnings: vec!["source-backed warning fixture".into()],
            blocks: if self.blocked {
                vec!["preflight hard stop".into()]
            } else {
                Vec::new()
            },
            ..Default::default()
        })
    }
}
#[tokio::test]
async fn default_shield_downgrades_before_dry_run_and_forced_shortfall_refuses() -> TestResult {
    let (client, chain, keys) = fixture(0).await?;
    let (ctx, sink, _) = context(&["--dry-run", "--json"])?;
    assert!(
        ctx.submit_with(
            &client,
            &Preview { blocked: false },
            &keys,
            false,
            Some("rtao tx add-stake")
        )
        .await?
        .is_none()
    );
    let payload: Value = serde_json::from_str(&sink.text(Stream::Stdout)?)?;
    assert!(
        !payload["effects"]
            .as_array()
            .ok_or("effects")?
            .iter()
            .any(|value| value
                .as_str()
                .is_some_and(|text| text.contains("submitted MEV-shielded")))
    );
    assert_eq!(keys.unlocks.load(Ordering::SeqCst), 0);
    assert!(
        chain
            .state()
            .lock()
            .map_err(|_| "chain")?
            .submissions
            .is_empty()
    );
    let (ctx, _, reads) = context(&["--mev-shield", "--dry-run", "--json"])?;
    let error = ctx
        .submit_with(&client, &Preview { blocked: false }, &keys, false, None)
        .await
        .err()
        .ok_or("forced shortfall")?;
    assert_eq!(error.code, 2);
    assert_eq!(reads.load(Ordering::SeqCst), 0);
    client.close().await;
    Ok(())
}
#[tokio::test]
async fn preflight_blocks_before_confirmation_and_json_without_yes_never_signs() -> TestResult {
    let (client, chain, keys) = fixture(1_000_000_000).await?;
    let (ctx, sink, reads) = context(&["--no-mev-shield"])?;
    let error = ctx
        .submit_with(&client, &Preview { blocked: true }, &keys, false, None)
        .await
        .err()
        .ok_or("hard stop")?;
    assert_eq!(error.code, 1);
    assert!(error.message.contains("preflight hard stop"));
    assert_eq!(reads.load(Ordering::SeqCst), 0);
    assert!(
        sink.text(Stream::Stderr)?
            .contains("source-backed warning fixture")
    );
    let (ctx, _, reads) = context(&["--no-mev-shield", "--json"])?;
    let error = ctx
        .submit_with(&client, &Preview { blocked: false }, &keys, false, None)
        .await
        .err()
        .ok_or("confirmation refusal")?;
    assert_eq!(error.code, 1);
    assert_eq!(reads.load(Ordering::SeqCst), 0);
    assert_eq!(keys.unlocks.load(Ordering::SeqCst), 0);
    assert!(
        chain
            .state()
            .lock()
            .map_err(|_| "chain")?
            .submissions
            .is_empty()
    );
    client.close().await;
    Ok(())
}
// Client narration fixture; it does not model runtime registration or fees.
struct ShieldFailurePreview {
    required: bool,
}
impl IntentInterface for ShieldFailurePreview {
    const SPEC: &'static rtao_catalog::IntentSpec = Preview::SPEC;
    type Role = Coldkey;
    type Origin = Signed;
    type Output = ();
    const WRAPS: &'static [&'static rtao_runtime::CallDescriptor] = Preview::WRAPS;
    fn to_json(&self) -> Value {
        Preview { blocked: false }.to_json()
    }
}
impl Intent for ShieldFailurePreview {
    fn safety(&self) -> Result<rtao::tx::IntentSafety, IntentError> {
        let mut safety = Preview { blocked: false }.safety()?;
        safety.mev_shield_required = self.required;
        Ok(safety)
    }
    fn summary(&self) -> String {
        Preview { blocked: false }.summary()
    }
    async fn build<V: View + ?Sized>(&self, cx: &BuildCx<'_, V>) -> Result<Built, IntentError> {
        Preview { blocked: false }.build(cx).await
    }
}

#[tokio::test]
async fn real_execution_unlocks_once_and_renders_one_canonical_receipt() -> TestResult {
    let (client, chain, keys) = fixture(1_000_000_000).await?;
    let (ctx, sink, _) = context(&["--yes", "--json", "--no-mev-shield"])?;
    let receipt = ctx
        .submit_with(&client, &Preview { blocked: false }, &keys, false, None)
        .await?
        .ok_or("receipt")?;
    assert!(receipt.success);
    assert_eq!(keys.unlocks.load(Ordering::SeqCst), 1);
    assert_eq!(
        chain.state().lock().map_err(|_| "chain")?.submissions.len(),
        1
    );
    assert_eq!(
        serde_json::from_str::<Value>(&sink.text(Stream::Stdout)?)?,
        receipt.to_json()
    );
    client.close().await;
    // Final transport survives pool refusal, default fallback and unknown fate.
    for (required, downgraded, unknown) in [
        (false, false, false),
        (true, false, false),
        (false, true, false),
        (false, false, true),
        (true, false, true),
    ] {
        for (json_mode, quiet) in [(false, false), (false, true), (true, true)] {
            let (client, chain, keys) = fixture(1_000_000_000).await?;
            chain.mutate_state(|state| -> TestResult {
                if !downgraded {
                    state.seed_python("MevShield", "NextKey", &[], json!(vec![0u8; 1184]))?;
                }
                if unknown {
                    state.queue_submit(rtao_testkit::SubmitScript::Statuses(vec![json!("ready")]));
                } else {
                    state.queue_reply(
                        "author_submitAndWatchExtrinsic",
                        rtao_testkit::Reply::error(
                            1010,
                            "Invalid Transaction",
                            Some(json!(
                                "Inability to pay some fees, e.g. because account balance is too low"
                            )),
                        ),
                    );
                }
                Ok(())
            })?;
            let mut flags = vec!["--yes"];
            if json_mode {
                flags.push("--json");
            }
            if quiet {
                flags.push("--quiet");
            }
            let (ctx, sink, _) = context(&flags)?;
            let releases = Mutex::new(Vec::new());
            let hooks = DurablePlanHooks {
                unlocks: &keys.unlocks,
                source_acquisitions: None,
                releases: &releases,
                plans: Mutex::new(Vec::new()),
                raw: Mutex::new(Vec::new()),
                enriched: Mutex::new(Vec::new()),
                fail_result: false,
            };
            let error = ctx
                .submit_with_keys_options(
                    &client,
                    &ShieldFailurePreview { required },
                    &keys,
                    SubmissionOptions {
                        period: Some(4),
                        deadline: Some(std::time::Duration::from_secs(5)),
                        hooks: Some(&hooks),
                        ..Default::default()
                    },
                )
                .await
                .err()
                .ok_or("carrier/fallback receipt must fail")?;
            assert_eq!(error.code, 1);
            let receipt = {
                let raw = hooks.raw.lock().map_err(|_| "raw carrier receipt")?;
                assert_eq!(raw.len(), 1);
                raw[0].clone()
            };
            assert!(!receipt.success);
            assert_eq!(receipt.fate_unknown.is_some(), unknown);
            assert_ne!(receipt.data.get("shielded"), Some(&json!(true)));
            if !unknown {
                assert_eq!(
                    receipt
                        .error
                        .as_ref()
                        .and_then(|error| error.name.as_deref()),
                    Some("Payment")
                );
            }
            let stderr = sink.text(Stream::Stderr)?;
            assert_eq!(
                stderr.contains("the submission used MEV shielding;"),
                !json_mode && !quiet && !downgraded,
                "{required}/{downgraded}/{unknown}: {stderr}"
            );
            if !json_mode && !downgraded {
                assert!(!stderr.contains("retry with"), "{stderr}");
            }
            if !json_mode && required {
                assert!(!stderr.contains("--no-mev-shield"), "{stderr}");
                assert!(stderr.contains("requires MEV shielding") || unknown);
            }
            if unknown {
                assert!(stderr.contains("do not resubmit until its outcome is known"));
                assert!(!stderr.contains("--no-mev-shield"), "{stderr}");
                assert!(!stderr.contains("the chain rejected the call"), "{stderr}");
            }
            if json_mode {
                assert_eq!(
                    serde_json::from_str::<Value>(&sink.text(Stream::Stdout)?)?,
                    receipt.to_json()
                );
                if !unknown {
                    assert!(stderr.is_empty());
                } else {
                    let diagnostic: Value = serde_json::from_str(&stderr)?;
                    assert_eq!(diagnostic["error"], receipt.message);
                }
            }
            assert_eq!(
                chain
                    .requests()
                    .iter()
                    .filter(|r| r.method == "author_submitAndWatchExtrinsic")
                    .count(),
                1
            );
            client.close().await;
        }
    }

    // A watched submission can return a failed receipt with native unknown-fate
    // identity. Keep canonical stdout while making that identity usable in CLI
    // recovery, after the actual SDK and durable receipt hook have completed.
    for (json_mode, fail_output) in [(false, false), (true, false), (true, true)] {
        let (client, chain, keys) = fixture(1_000_000_000).await?;
        chain.mutate_state(|state| {
            state.queue_submit(rtao_testkit::SubmitScript::Statuses(vec![json!("ready")]))
        });
        let mut flags = vec!["--yes", "--quiet", "--no-mev-shield"];
        if json_mode {
            flags.push("--json");
        }
        let (mut ctx, sink, _) = context(&flags)?;
        let broken = Arc::new(RefuseReceiptSink::default());
        if fail_output {
            ctx.output = Output::new(ctx.output.options.clone(), broken.clone());
        }
        let releases = Mutex::new(Vec::new());
        let hooks = DurablePlanHooks {
            unlocks: &keys.unlocks,
            source_acquisitions: None,
            releases: &releases,
            plans: Mutex::new(Vec::new()),
            raw: Mutex::new(Vec::new()),
            enriched: Mutex::new(Vec::new()),
            fail_result: false,
        };
        let error = ctx
            .submit_with_keys_options(
                &client,
                &Preview { blocked: false },
                &keys,
                SubmissionOptions {
                    hooks: Some(&hooks),
                    deadline: Some(std::time::Duration::from_secs(5)),
                    ..Default::default()
                },
            )
            .await
            .err()
            .ok_or("unknown-fate receipt must fail")?;
        assert_eq!(error.code, 1);
        let receipt = {
            let receipts = hooks.raw.lock().map_err(|_| "raw receipt")?;
            assert_eq!(receipts.len(), 1, "raw receipt missing before {error:?}");
            receipts[0].clone()
        };
        assert!(!receipt.success);
        let hash = receipt.fate_unknown.ok_or("native unknown-fate hash")?;
        let submissions = chain
            .state()
            .lock()
            .map_err(|_| "state")?
            .submissions
            .clone();
        assert_eq!(submissions.len(), 1);
        assert_eq!(
            rtao::core::ops::extrinsic_lookup::find(
                &json!({"block":{"extrinsics":[format!("0x{}",hex::encode(&submissions[0]))]}}),
                hash.0,
            )?,
            Some(0)
        );
        let hash = format!("0x{}", hex::encode(hash.0));
        if fail_output {
            assert!(error.message.contains("receipt pipe closed"));
            let help = error
                .help
                .as_deref()
                .ok_or("unknown-fate output recovery")?;
            assert!(help.contains(&hash));
            assert!(help.contains("do not resubmit until its outcome is known"));
            let captured: Value = serde_json::from_slice(&broken.0.lock().map_err(|_| "sink")?)?;
            assert_eq!(captured, receipt.to_json());
            assert_eq!(keys.unlocks.load(Ordering::SeqCst), 1);
            client.close().await;
            continue;
        }
        let stderr = sink.text(Stream::Stderr)?;
        assert!(stderr.contains(&hash));
        assert!(stderr.contains("do not resubmit until its outcome is known"));
        assert!(!stderr.contains("--no-mev-shield"));
        assert!(error.message.is_empty(), "the receipt is already rendered");
        if json_mode {
            let stdout: Value = serde_json::from_str(&sink.text(Stream::Stdout)?)?;
            assert_eq!(stdout, receipt.to_json());
            assert!(stdout.get("fate_unknown").is_none());
            let diagnostic: Value = serde_json::from_str(&stderr)?;
            assert_eq!(diagnostic["error"], receipt.message);
        } else {
            assert!(sink.text(Stream::Stdout)?.is_empty());
        }
        assert_eq!(keys.unlocks.load(Ordering::SeqCst), 1);
        client.close().await;
    }
    Ok(())
}

#[derive(Default)]
struct SnapshotFacts {
    builds: AtomicUsize,
    previews: AtomicUsize,
    reviewed_call: Mutex<Option<String>>,
}
impl IntentInterface for SnapshotFacts {
    const SPEC: &'static rtao_catalog::IntentSpec = Preview::SPEC;
    type Role = Coldkey;
    type Origin = Signed;
    type Output = ();
    const WRAPS: &'static [&'static rtao_runtime::CallDescriptor] = Preview::WRAPS;
    fn to_json(&self) -> Value {
        json!({"op": "add_stake"})
    }
}
impl Intent for SnapshotFacts {
    fn summary(&self) -> String {
        "review one call and its facts".into()
    }
    async fn build<V: View + ?Sized>(&self, cx: &BuildCx<'_, V>) -> Result<Built, IntentError> {
        let build = self.builds.fetch_add(1, Ordering::SeqCst) + 1;
        let value =
            u8::try_from(build).map_err(|_| IntentError::validation("too many fixture builds"))?;
        Ok(rtao_runtime::calls::system::remark(vec![value])
            .compose(cx.runtime)?
            .into())
    }
    async fn preflight<V: View + ?Sized>(
        &self,
        cx: &PreflightCx<'_, V>,
    ) -> Result<IntentPreflight, IntentError> {
        self.previews.fetch_add(1, Ordering::SeqCst);
        let call = format!("0x{}", hex::encode(cx.call.as_bytes()));
        *self
            .reviewed_call
            .lock()
            .map_err(|_| IntentError::validation("fixture lock poisoned"))? = Some(call.clone());
        Ok(IntentPreflight {
            facts_title: rtao::tx::FactsTitle::Custom("Authenticated call facts".into()),
            facts: vec![("reviewed bytes".into(), call)],
            ..Default::default()
        })
    }
}

#[tokio::test]
async fn review_facts_share_one_build_and_preflight_with_the_reviewed_call() -> TestResult {
    let (client, chain, keys) = fixture(1_000_000_000).await?;
    let intent = SnapshotFacts::default();
    let (plan, preview) = rtao::Executor::new(client.clone())
        .plan_and_preflight_dyn(&intent, &keys, &Default::default())
        .await?;
    assert_eq!(intent.builds.load(Ordering::SeqCst), 1);
    assert_eq!(intent.previews.load(Ordering::SeqCst), 1);
    assert_eq!(
        preview.facts,
        vec![(
            "reviewed bytes".into(),
            format!("0x{}", hex::encode(plan.call().as_bytes()))
        )]
    );
    assert_eq!(preview.facts_title.as_str(), "Authenticated call facts");

    let intent = SnapshotFacts::default();
    let (ctx, sink, _) = context(&["--no-mev-shield"])?;
    *ctx.terminal.lock().map_err(|_| "terminal")? = Box::new(TerminalFixture {
        tty: true,
        lines: VecDeque::from(["no".into()]),
        reads: Arc::new(AtomicUsize::new(0)),
    });
    assert!(
        ctx.submit_with(&client, &intent, &keys, false, None)
            .await
            .is_err()
    );
    assert_eq!(intent.builds.load(Ordering::SeqCst), 1);
    assert_eq!(intent.previews.load(Ordering::SeqCst), 1);
    let reviewed = intent
        .reviewed_call
        .lock()
        .map_err(|_| "reviewed call")?
        .clone()
        .ok_or("missing reviewed call")?;
    let rendered = sink.text(Stream::Stdout)?;
    assert!(rendered.contains("Authenticated call facts"));
    assert!(rendered.contains("reviewed bytes"));
    assert!(rendered.contains(&reviewed));
    assert_eq!(keys.unlocks.load(Ordering::SeqCst), 0);
    assert!(
        chain
            .state()
            .lock()
            .map_err(|_| "state")?
            .submissions
            .is_empty()
    );
    client.close().await;
    Ok(())
}

struct ExpiringProxyProofKeys {
    keys: Keys,
    chain: FakeChain,
}
impl KeyView for ExpiringProxyProofKeys {
    fn coldkey_address(&self) -> Result<AccountId, SignerError> {
        self.keys.coldkey_address()
    }
    fn hotkey_address(&self) -> Result<AccountId, SignerError> {
        self.keys.hotkey_address()
    }
    fn hotkey_public_key(&self) -> Result<[u8; 32], SignerError> {
        self.keys.hotkey_public_key()
    }
}
impl DynKeySource for ExpiringProxyProofKeys {
    fn signer_for<'a>(
        &'a self,
        role: SignerRole,
    ) -> BoxFuture<'a, Result<Arc<dyn Signer>, SignerError>> {
        Box::pin(async move {
            // The reviewed work is valid at height 100 and expires while the
            // delegate unlocks. A cached signer prevents a second such delay.
            self.chain
                .advance(5)
                .map_err(|error| SignerError::KeyView(error.to_string()))?;
            self.keys.signer_for(role).await
        })
    }
}

#[tokio::test]
async fn late_pow_refresh_preserves_proxy_owner_and_explicit_hotkey_after_unlock() -> TestResult {
    use rtao::primitives::NetUid;
    use rtao_cli::{commands::generated, context::SubmissionOptions};
    use rtao_intents::pow::registration_prefix;

    fn arg<'a>(call: &'a Value, name: &str) -> Option<&'a Value> {
        call["call_args"]
            .as_array()?
            .iter()
            .find(|arg| arg["name"] == name)
            .map(|arg| &arg["value"])
    }

    let keys = Keys {
        key: KeypairSigner::from_seed(&[19; 32], CryptoType::Ed25519)?,
        unlocks: Arc::new(AtomicUsize::new(0)),
    };
    let delegate = keys.key.account();
    let owner = AccountId([48; 32]);
    let hotkey = AccountId([47; 32]);
    assert_ne!(delegate, owner);
    assert_ne!(delegate, hotkey);
    let mut state = FakeState::new(FakeState::try_default()?.runtime);
    state.set_finalized(98)?;
    state.seed_python(
        "System",
        "Account",
        &[json!(delegate)],
        json!({"data":{"free":1_000_000_000_000u64}}),
    )?;
    let netuid = NetUid(1);
    for item in ["NetworksAdded", "NetworkPowRegistrationAllowed"] {
        state.seed_typed("SubtensorModule", item, (netuid,), true)?;
    }
    for (item, value) in [
        ("Difficulty", 1),
        ("MinDifficulty", 1),
        ("MaxDifficulty", u64::MAX),
    ] {
        state.seed_typed("SubtensorModule", item, (netuid,), value)?;
    }
    for (item, value) in [
        ("MaxAllowedUids", 16u16),
        ("MaxRegistrationsPerBlock", 1u16),
        ("RegistrationsThisBlock", 0u16),
    ] {
        state.seed_typed("SubtensorModule", item, (netuid,), value)?;
    }
    let chain = FakeChain::from_state(state);
    let mut config = ClientConfig::new(Network::resolve_with("local", |_| None)?)?;
    config.track_heads = false;
    config.runtime_cache_dir = None;
    config.config_store = None;
    let client = Client::from_rpc(RpcClient::new(chain.clone()), config).await?;
    let original_hash = client.rpc().block_hash(100).await?.ok_or("original hash")?;
    let prefix = registration_prefix(netuid, original_hash, hotkey, owner)?;
    let work = rtao::core::pow::seal(&prefix, 0)?;
    let request = generated::TxRequest {
        op: "pow_register",
        args: serde_json::Map::from_iter([
            ("netuid".into(), json!(netuid)),
            ("hotkey_ss58".into(), json!(hotkey)),
            ("work_block".into(), json!(100)),
            ("nonce".into(), json!(0)),
            ("work_hex".into(), json!(hex::encode(work))),
            ("mining_workers".into(), json!(1)),
            ("mining_timeout".into(), json!(10)),
            ("mining_backend".into(), json!("cpu")),
        ]),
        period: None,
    };
    let owner_flag = owner.to_string();
    let (ctx, _, _) = context(&[
        "--yes",
        "--json",
        "--no-mev-shield",
        "--proxy-for",
        &owner_flag,
    ])?;
    let unlocks = keys.unlocks.clone();
    let keys = ExpiringProxyProofKeys {
        keys,
        chain: chain.clone(),
    };
    let result = generated::execute_tx_with_keys(
        &ctx,
        &client,
        &request,
        &keys,
        SubmissionOptions::default(),
    )
    .await?
    .ok_or("receipt")?;
    assert!(result.success);
    assert_eq!(result.data["proxy_for"], json!(owner));
    assert_eq!(unlocks.load(Ordering::SeqCst), 1);
    let submissions = chain
        .state()
        .lock()
        .map_err(|_| "state")?
        .submissions
        .clone();
    assert_eq!(submissions.len(), 1);
    let runtime = client.refresh_runtime().await?;
    let decoded =
        rtao::core::DynValue::new(runtime.core().decode_extrinsic(&submissions[0], true)?)
            .to_json();
    let proxy = &decoded["call"];
    assert_eq!(proxy["call_module"], json!("Proxy"));
    assert_eq!(proxy["call_function"], json!("proxy"));
    assert_eq!(arg(proxy, "real"), Some(&json!(owner)));
    let call = arg(proxy, "call").ok_or("inner call")?;
    assert_eq!(call["call_function"], json!("pow_register"));
    assert_eq!(arg(call, "hotkey"), Some(&json!(hotkey)));
    assert_eq!(arg(call, "work_block"), Some(&json!(105)));
    let nonce = arg(call, "nonce").and_then(Value::as_u64).ok_or("nonce")?;
    let hash = client.rpc().block_hash(105).await?.ok_or("fresh hash")?;
    let prefix = registration_prefix(netuid, hash, hotkey, owner)?;
    let expected = rtao::core::pow::seal(&prefix, nonce)?;
    assert_eq!(
        arg(call, "work"),
        Some(
            &rtao::core::DynValue::new(rtao::core::codec::Value::Bytes(expected.to_vec()))
                .to_json()
        )
    );
    let wrong_prefix = registration_prefix(netuid, hash, hotkey, delegate)?;
    assert_ne!(expected, rtao::core::pow::seal(&wrong_prefix, nonce)?);
    client.close().await;
    Ok(())
}
#[test]
fn only_literal_config_false_blocks_required_shield_and_off_disables_endpoints() -> TestResult {
    let (mut ctx, _, _) = context(&[])?;
    for false_like in [json!(0), json!(""), json!([]), json!({})] {
        ctx.config.insert("mev_shield".into(), false_like);
        assert_eq!(
            ctx.resolve_mev_shield(true, true, "burned_register")?,
            ShieldChoice {
                enabled: true,
                forced: true
            }
        );
    }
    ctx.config.insert("mev_shield".into(), json!(false));
    assert!(
        ctx.resolve_mev_shield(true, true, "burned_register")
            .is_err()
    );
    assert!(rtao_cli::context::endpoint_pool(" OFF ")?.is_empty());
    Ok(())
}

#[tokio::test]
async fn transient_balance_failure_reprobes_known_zero_before_carrier_decision() -> TestResult {
    let (client, chain, keys) = fixture(0).await?;
    let (ctx, _, _) = context(&[])?;
    chain.queue_transport_error(
        "state_queryStorageAt",
        rtao_testkit::rpc::Error::Client(Box::new(rtao_rpc::RpcError::Timeout {
            detail: "fixture failure".into(),
        })),
    );
    let warning = ctx
        .shield_fee_shortfall(&client, keys.key.account())
        .await
        .ok_or("fallback zero balance")?;
    assert_eq!(
        warning,
        "free TAO cannot cover the MEV-shield outer carrier fee; alpha fees only apply to the unshielded call — use --no-mev-shield"
    );
    assert_eq!(keys.unlocks.load(Ordering::SeqCst), 0);
    assert!(
        chain
            .state()
            .lock()
            .map_err(|_| "chain")?
            .submissions
            .is_empty()
    );
    client.close().await;
    Ok(())
}
#[test]
fn signing_account_precedence_preserves_explicit_typo_and_member_before_config() -> TestResult {
    let (mut ctx, _, _) = context(&["--signer", "vault"])?;
    let first = AccountId([1; 32]);
    let second = AccountId([2; 32]);
    let third = AccountId([3; 32]);
    ctx.books = Books::from_values(
        json!([{"name":"explicit","address":first},{"name":"member","address":second},{"name":"configured","address":third}]),
        json!([]),
        json!([]),
    );
    ctx.config
        .insert("signer_address".into(), json!("configured"));
    ctx.globals.signatories = vec!["member".into()];
    assert_eq!(ctx.external_signer_address()?, Some(second));
    ctx.globals.signer_address = Some("explicit".into());
    assert_eq!(ctx.external_signer_address()?, Some(first));
    ctx.globals.signer_address = Some("unknown-typo".into());
    assert_eq!(ctx.external_signer_address()?, None);
    ctx.globals.signer_address = None;
    ctx.globals.signatories.clear();
    assert_eq!(ctx.external_signer_address()?, Some(third));
    Ok(())
}
#[test]
fn confirmation_reasks_invalid_input_and_accepts_python_control_space() -> TestResult {
    let (mut ctx, _, reads) = context(&[])?;
    ctx.terminal = Arc::new(Mutex::new(Box::new(TerminalFixture {
        tty: true,
        lines: VecDeque::from(["maybe".into(), "\u{1c}YES\u{1f}".into()]),
        reads: reads.clone(),
    })));
    ctx.confirm("submit?")?;
    assert_eq!(reads.load(Ordering::SeqCst), 2);
    Ok(())
}

struct Hooks {
    before: AtomicUsize,
    after: AtomicUsize,
    failed: AtomicUsize,
}
impl rtao_cli::context::SubmissionHooks for Hooks {
    fn before_submit(&self) -> BoxFuture<'_, Result<(), rtao_cli::error::CliError>> {
        Box::pin(async move {
            self.before.fetch_add(1, Ordering::SeqCst);
            Ok(())
        })
    }
    fn on_result<'a>(
        &'a self,
        result: &'a rtao::tx::ExtrinsicResult,
    ) -> BoxFuture<'a, Result<(), rtao_cli::error::CliError>> {
        Box::pin(async move {
            self.after.fetch_add(1, Ordering::SeqCst);
            if !result.success {
                self.failed.fetch_add(1, Ordering::SeqCst);
            }
            Ok(())
        })
    }
}
#[tokio::test]
async fn recovery_hooks_skip_preview_and_run_before_failed_receipt_rendering() -> TestResult {
    let (client, chain, keys) = fixture(1_000_000_000).await?;
    let hooks = Hooks {
        before: AtomicUsize::new(0),
        after: AtomicUsize::new(0),
        failed: AtomicUsize::new(0),
    };
    let (preview, _, _) = context(&["--dry-run", "--json", "--no-mev-shield"])?;
    preview
        .submit_with_hooks(
            &client,
            &Preview { blocked: false },
            &keys,
            false,
            None,
            Some(&hooks),
        )
        .await?;
    assert_eq!(hooks.before.load(Ordering::SeqCst), 0);
    assert_eq!(hooks.after.load(Ordering::SeqCst), 0);
    chain.mutate_state(|state|state.queue_submit(rtao_testkit::SubmitScript::InBlockThenFinalized {events:json!([{"phase":"ApplyExtrinsic","extrinsic_idx":0,"module_id":"System","event_id":"ExtrinsicFailed","attributes":{"dispatch_error":{"BadOrigin":null},"dispatch_info":{"weight":{"ref_time":1,"proof_size":0},"class":"Normal","pays_fee":"Yes"}},"topics":[]}])}));
    let (ctx, sink, _) = context(&["--yes", "--json", "--no-mev-shield"])?;
    let error = ctx
        .submit_with_hooks(
            &client,
            &Preview { blocked: false },
            &keys,
            false,
            None,
            Some(&hooks),
        )
        .await
        .err()
        .ok_or("failed receipt exit")?;
    assert_eq!(error.code, 1);
    assert!(error.message.is_empty());
    assert_eq!(hooks.before.load(Ordering::SeqCst), 1);
    assert_eq!(hooks.after.load(Ordering::SeqCst), 1);
    assert_eq!(hooks.failed.load(Ordering::SeqCst), 1);
    let receipt: Value = serde_json::from_str(&sink.text(Stream::Stdout)?)?;
    assert_eq!(receipt["success"], json!(false));
    assert!(sink.text(Stream::Stderr)?.is_empty());
    client.close().await;
    Ok(())
}

#[test]
fn configured_proxy_truthiness_and_self_sentinel_preserve_dispatch_identity() -> TestResult {
    let (mut ctx, _, _) = context(&[])?;
    let account = AccountId([44; 32]);
    ctx.books = Books::from_values(
        json!([{"name":"self","address":account},{"name":"True","address":account}]),
        json!([]),
        json!([]),
    );
    for empty in [json!(""), Value::Null, json!(false), json!(0)] {
        ctx.config.insert("proxy_for".into(), empty);
        assert_eq!(ctx.dispatch_proxy(None)?, None);
    }
    ctx.config.insert("proxy_for".into(), json!("self"));
    assert_eq!(ctx.dispatch_proxy(None)?, Some(account));
    assert_eq!(ctx.dispatch_proxy(Some("self"))?, None);
    assert!(ctx.dispatch_proxy(Some("")).is_err());
    ctx.config.insert("proxy_for".into(), json!(true));
    assert_eq!(ctx.dispatch_proxy(None)?, Some(account));
    Ok(())
}

#[derive(Clone)]
struct Connected(Client);
impl rtao_cli::context::ConnectionFactory for Connected {
    fn connect<'a>(
        &'a self,
        _: ClientConfig,
    ) -> BoxFuture<'a, Result<Client, rtao_cli::error::CliError>> {
        Box::pin(async move { Ok(self.0.clone()) })
    }
}
struct RefusedConnection(rtao::client::ClientError);
impl rtao_cli::context::ConnectionFactory for RefusedConnection {
    fn connect<'a>(
        &'a self,
        _: ClientConfig,
    ) -> BoxFuture<'a, Result<Client, rtao_cli::error::CliError>> {
        Box::pin(async move { Err(self.0.clone().into()) })
    }
}
fn wallet_prompt_public_accounts(
    root: &CacheRoot,
) -> Result<(AccountId, AccountId), Box<dyn std::error::Error>> {
    let cold = AccountId([7; 32]);
    let hot = AccountId([8; 32]);
    let wallet = root.0.join("owner");
    std::fs::create_dir_all(wallet.join("hotkeys"))?;
    for (path, account) in [
        (wallet.join("coldkeypub.txt"), cold),
        (wallet.join("hotkeys/minerpub.txt"), hot),
    ] {
        std::fs::write(
            path,
            serde_json::to_vec(&json!({"ss58Address": account, "cryptoType": 1}))?,
        )?;
    }
    for path in [wallet.join("coldkey"), wallet.join("hotkeys/miner")] {
        std::fs::write(path, b"selection must use the public companion")?;
    }
    Ok((cold, hot))
}

#[tokio::test]
async fn confirmed_wallet_and_hotkey_cache_the_selected_names_by_account() -> TestResult {
    let root = CacheRoot::new()?;
    let (cold, hot) = wallet_prompt_public_accounts(&root)?;
    let (mut ctx, _, reads) = context(&[])?;
    ctx.globals.wallet_path = root.0.clone();
    ctx.globals.wallet_given = false;
    ctx.globals.hotkey_given = false;
    *ctx.terminal.lock().map_err(|_| "terminal")? = Box::new(TerminalFixture {
        tty: true,
        lines: VecDeque::from(["owner".into(), "miner".into()]),
        reads: reads.clone(),
    });
    let selected = ctx.confirm_wallet(
        &rtao_cli::prompt::WalletConfirmation {
            hotkey_help: Some("Signing hotkey.".into()),
            ..Default::default()
        },
        &mut rtao_cli::prompt::PromptHistory::default(),
    )?;
    assert_eq!(selected, None);
    assert_eq!(ctx.globals.wallet_name, "owner");
    assert_eq!(ctx.globals.hotkey_name, "miner");
    assert!(ctx.globals.wallet_given && ctx.globals.hotkey_given);
    assert_eq!(reads.load(Ordering::SeqCst), 2);
    assert_eq!(
        ctx.output.address_names()?,
        std::collections::BTreeMap::from([
            (cold.to_string(), "owner".to_owned()),
            (hot.to_string(), "owner/miner".to_owned()),
        ])
    );
    assert!(
        ctx.output
            .account_text(&cold.to_string(), None)
            .ends_with(" (owner)")
    );
    assert!(
        ctx.output
            .account_text(&hot.to_string(), None)
            .ends_with(" (owner/miner)")
    );

    // An explicitly selected wallet skips those prompts. Public signing-key
    // construction still registers both identities before the real preview.
    let (mut explicit, sink, reads) = context(&["--dry-run", "--json", "--no-mev-shield"])?;
    explicit.globals.wallet_path = root.0.clone();
    explicit.globals.wallet_name = "owner".into();
    explicit.globals.hotkey_name = "miner".into();
    explicit.globals.wallet_given = true;
    explicit.globals.hotkey_given = true;
    explicit
        .output
        .name_address(&cold.to_string(), "already selected")?;
    let contact = AccountId([42; 32]);
    let proxy = AccountId([43; 32]);
    let empty = AccountId([44; 32]);
    explicit.books = Books::from_values(
        json!([
            {"address":cold,"name":"cold contact"},
            {"address":hot,"name":"hot contact"},
            {"address":contact,"name":"unused contact"},
            {"address":empty,"name":""},
        ]),
        json!([
            {"address":contact,"name":"later proxy alias"},
            {"address":proxy,"name":"unused proxy"},
        ]),
        json!([]),
    );
    let cold_path = root.0.join("owner/coldkey");
    let hot_path = root.0.join("owner/hotkeys/miner");
    let original_cold = std::fs::read(&cold_path)?;
    let original_hot = std::fs::read(&hot_path)?;
    let keys = explicit.signing_keys(SignerRole::Coldkey, None).await?;
    let (client, chain, _) = fixture(1_000_000_000).await?;
    assert!(
        explicit
            .submit_with(
                &client,
                &Preview { blocked: false },
                keys.as_ref(),
                false,
                None
            )
            .await?
            .is_none()
    );
    assert_eq!(
        explicit.output.address_names()?,
        std::collections::BTreeMap::from([
            (cold.to_string(), "already selected".to_owned()),
            (hot.to_string(), "owner/miner".to_owned()),
            (contact.to_string(), "unused contact".to_owned()),
            (proxy.to_string(), "unused proxy".to_owned()),
        ])
    );
    assert_eq!(
        explicit.output.address_kinds()?.get(&cold.to_string()),
        Some(&rtao_cli::books::AccountKind::Coldkey)
    );
    assert_eq!(
        explicit.output.address_kinds()?.get(&hot.to_string()),
        Some(&rtao_cli::books::AccountKind::Hotkey)
    );
    let plan: Value = serde_json::from_str(&sink.text(Stream::Stdout)?)?;
    assert_eq!(plan["signer_address"], json!(cold));
    assert_eq!(reads.load(Ordering::SeqCst), 0);
    assert!(chain.mutate_state(|state| state.submissions.is_empty()));
    assert!(
        chain
            .requests()
            .iter()
            .all(|request| !request.method.starts_with("author_"))
    );
    assert_eq!(std::fs::read(cold_path)?, original_cold);
    assert_eq!(std::fs::read(hot_path)?, original_hot);
    client.close().await;
    Ok(())
}

#[test]
fn confirmed_public_targets_cache_book_and_hotkey_reference_labels_without_reselecting_keys()
-> TestResult {
    let root = CacheRoot::new()?;
    let (cold, hot) = wallet_prompt_public_accounts(&root)?;
    let pasted = hot.to_string();
    for (hotkey_target, answer, expected, label) in [
        (false, "cold contact", cold, Some("cold contact")),
        (true, "hot contact", hot, Some("hot contact")),
        (true, "owner/miner", hot, Some("owner/miner")),
        (true, pasted.as_str(), hot, None),
    ] {
        let (mut ctx, _, reads) = context(&[])?;
        ctx.globals.wallet_path = root.0.clone();
        ctx.globals.wallet_name = "owner".into();
        ctx.globals.hotkey_name = "unchanged signing hotkey".into();
        ctx.globals.wallet_given = hotkey_target;
        ctx.globals.hotkey_given = false;
        ctx.books.addresses = vec![
            json!({"name": "cold contact", "address": cold}),
            json!({"name": "hot contact", "address": hot}),
        ];
        *ctx.terminal.lock().map_err(|_| "terminal")? = Box::new(TerminalFixture {
            tty: true,
            lines: VecDeque::from([answer.into()]),
            reads: reads.clone(),
        });
        let selected = ctx.confirm_wallet(
            &rtao_cli::prompt::WalletConfirmation {
                accept_address: true,
                hotkey_help: hotkey_target.then(|| "Target hotkey.".into()),
                ..Default::default()
            },
            &mut rtao_cli::prompt::PromptHistory::default(),
        )?;
        assert_eq!(selected, Some(expected), "{answer}");
        assert_eq!(ctx.globals.wallet_name, "owner");
        assert_eq!(ctx.globals.hotkey_name, "unchanged signing hotkey");
        assert!(ctx.globals.wallet_given);
        assert_eq!(ctx.globals.hotkey_given, hotkey_target);
        assert_eq!(reads.load(Ordering::SeqCst), 1);
        let expected_names = label
            .map(|label| {
                std::collections::BTreeMap::from([(expected.to_string(), label.to_owned())])
            })
            .unwrap_or_default();
        assert_eq!(ctx.output.address_names()?, expected_names, "{answer}");
        if let Some(label) = label {
            assert!(
                ctx.output
                    .account_text(&expected.to_string(), None)
                    .ends_with(&format!(" ({label})"))
            );
        }
    }
    Ok(())
}

struct CacheRoot(std::path::PathBuf);
impl CacheRoot {
    fn new() -> Result<Self, std::io::Error> {
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let base = std::env::var_os("RTAO_TEMP_ROOT")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| {
                std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                    .join("../../target/cli-context-tests")
            });
        let root = base.join("tmp").join(format!(
            "context-cache-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::SeqCst)
        ));
        std::fs::create_dir_all(&root)?;
        Ok(Self(root))
    }
}
impl Drop for CacheRoot {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
async fn cached_context(
    store: &rtao::client::config::Store,
) -> Result<AppContext, Box<dyn std::error::Error>> {
    let parsed = command_tree([with_globals(clap::Command::new("read"), Tier::Read)])
        .try_get_matches_from(["rtao", "--network", "local", "read"])?;
    let terminal = TerminalFixture {
        tty: false,
        lines: VecDeque::new(),
        reads: Arc::new(AtomicUsize::new(0)),
    };
    Ok(AppContext::from_store(
        &parsed,
        parsed.subcommand().ok_or("read")?.1,
        &[],
        store,
        Path::new("/isolated/home"),
        |_| None,
        Arc::new(Mutex::new(Box::new(terminal))),
    )
    .await?)
}
#[tokio::test]
async fn fresh_name_cache_skips_rpc_and_stale_names_refresh_and_persist() -> TestResult {
    use rtao::primitives::NetUid;
    use std::collections::BTreeMap;
    let root = CacheRoot::new()?;
    let paths = rtao::client::config::Paths::new(&root.0, |_| None);
    let store = rtao::client::config::Store::with_clock(paths.clone(), || 1000.0);
    store
        .save_subnet_names(
            "local",
            BTreeMap::from([(NetUid(5), "cached".into())]).into(),
        )
        .await?;
    let (client, chain, _) = fixture(1_000_000_000).await?;
    let before = chain.state().lock().map_err(|_| "chain")?.requests.len();
    let ctx = cached_context(&store).await?;
    ctx.run_with(&Connected(client.clone()), |_| async { Ok(()) })
        .await?;
    assert!(client.is_closed());
    assert_eq!(ctx.output.subnet_names()?.get(&5), Some(&"cached".into()));
    assert_eq!(
        chain.state().lock().map_err(|_| "chain")?.requests.len(),
        before
    );
    let (client, chain, _) = fixture(1_000_000_000).await?;
    chain.mutate_state(|state| {
        state.seed_python(
            "SubtensorModule",
            "SubnetIdentitiesV3",
            &[json!(5)],
            json!({"subnet_name":"fresh"}),
        )
    })?;
    let stale = rtao::client::config::Store::with_clock(paths, || 200000.0);
    let ctx = cached_context(&stale).await?;
    let clone = ctx.output.clone();
    ctx.run_with(&Connected(client.clone()), |_| async { Ok(()) })
        .await?;
    assert!(client.is_closed());
    assert_eq!(clone.subnet_names()?.get(&5), Some(&"fresh".into()));
    assert_eq!(
        stale
            .load_subnet_names("local")
            .await?
            .chain_values()
            .values
            .get(&NetUid(5)),
        Some(&"fresh".into())
    );
    Ok(())
}

#[tokio::test]
async fn cosmetic_name_rpc_failure_does_not_fail_work_and_connection_is_closed() -> TestResult {
    let (client, chain, _) = fixture(1_000_000_000).await?;
    let (ctx, _, _) = context(&[])?;
    chain.queue_transport_error(
        "state_getKeysPaged",
        rtao_testkit::rpc::Error::Client(Box::new(rtao_rpc::RpcError::Timeout {
            detail: "cosmetic fixture".into(),
        })),
    );
    assert_eq!(
        ctx.run_with(&Connected(client.clone()), |_| async { Ok(77) })
            .await?,
        77
    );
    assert!(client.is_closed());

    let connection_help = "check the endpoint and network connection, then retry";
    for (cause, detail) in [
        (
            rtao_rpc::RpcError::Transport(" \u{1c}offline \n".into()),
            "offline",
        ),
        (
            rtao_rpc::RpcError::Timeout {
                detail: " \n".into(),
            },
            "the connection timed out without a response",
        ),
        (
            rtao_rpc::RpcError::MaxRetriesExceeded {
                detail: "retries exhausted".into(),
            },
            "retries exhausted",
        ),
        (
            rtao_rpc::RpcError::Handshake {
                status: 503,
                detail: "offline".into(),
            },
            "RPC endpoint rejected the WebSocket handshake (HTTP 503): offline",
        ),
        (rtao_rpc::RpcError::Closed, "RPC session closed"),
    ] {
        let client_error = rtao::client::ClientError::from(cause.clone());
        for converted in [
            rtao_cli::error::CliError::from(client_error.clone()),
            rtao_cli::error::CliError::from(rtao::tx::TxError::Rpc(cause.clone())),
            rtao_cli::error::CliError::from(rtao::tx::TxError::Intent(IntentError::Client(
                client_error.clone(),
            ))),
            rtao_cli::error::CliError::from(rtao::reads::ReadError::Client(client_error.clone())),
            rtao_cli::error::CliError::from(rtao_intents::pow::PowError::Client(client_error)),
            rtao_cli::error::CliError::from(rtao_intents::pow::PowError::Head(
                rtao::client::HeadError::Rpc(cause.clone()),
            )),
        ] {
            assert_eq!(converted.message, detail);
            assert!(matches!(
                converted.diagnostic,
                Some(rtao_cli::error::CliDiagnostic::Connection),
            ));
        }
        let (ctx, _, _) = context(&[])?;
        let calls = AtomicUsize::new(0);
        let error = ctx
            .run_with(&RefusedConnection(cause.clone().into()), |_| async {
                calls.fetch_add(1, Ordering::SeqCst);
                Ok(())
            })
            .await
            .err()
            .ok_or("connector refusal")?;
        assert_eq!(calls.load(Ordering::SeqCst), 0);
        assert_eq!(
            error.message,
            format!("could not reach {}: {detail}", ctx.globals.network)
        );
        assert_eq!(error.help.as_deref(), Some(connection_help));
        assert!(error.diagnostic.is_none());
        assert_eq!(error.code, 1);

        // The real client request path retains the same classification and
        // cleanup. Cosmetic-name acquisition is aborted on a work failure.
        let (client, chain, keys) = fixture(1_000_000_000).await?;
        chain.queue_transport_error(
            "system_health",
            rtao_testkit::rpc::Error::Client(Box::new(cause)),
        );
        let error = ctx
            .run_with(&Connected(client.clone()), |client| async move {
                client
                    .request::<Value>("system_health", json!([]))
                    .await
                    .map_err(rtao_cli::error::CliError::from)
            })
            .await
            .err()
            .ok_or("work connection failure")?;
        assert_eq!(
            error.message,
            format!("could not reach {}: {detail}", ctx.globals.network)
        );
        assert_eq!(error.help.as_deref(), Some(connection_help));
        assert!(client.is_closed());
        assert_eq!(
            chain
                .requests()
                .iter()
                .filter(|r| r.method == "system_health")
                .count(),
            1
        );
        assert_eq!(keys.unlocks.load(Ordering::SeqCst), 0);
        assert!(chain.mutate_state(|state| state.submissions.is_empty()));
    }

    // Custom network URLs can contain credentials and access tokens. Only
    // their public endpoint identity belongs in human or machine diagnostics.
    for (network, public) in [
        (
            "wss://endpoint-user:endpoint-password@rpc.example:443/private?api_key=endpoint-token#fragment-secret",
            "wss://rpc.example:443",
        ),
        (
            "ws://endpoint-user:endpoint-password@[2001:db8::1]:9944/private?api_key=endpoint-token#fragment-secret",
            "ws://[2001:db8::1]:9944",
        ),
    ] {
        for mode in [vec![], vec!["--json"], vec!["--quiet"]] {
            let mut flags = vec!["--network", network];
            flags.extend(mode);
            let (ctx, sink, reads) = context(&flags)?;
            assert_eq!(ctx.client_config()?.network.endpoint(), network);
            let calls = AtomicUsize::new(0);
            let error = ctx
                .run_with(
                    &RefusedConnection(rtao_rpc::RpcError::Transport("offline".into()).into()),
                    |_| async {
                        calls.fetch_add(1, Ordering::SeqCst);
                        Ok(())
                    },
                )
                .await
                .err()
                .ok_or("custom endpoint refusal")?;
            assert_eq!(error.message, format!("could not reach {public}: offline"));
            assert_eq!(error.help.as_deref(), Some(connection_help));
            ctx.output.cli_error(&error)?;
            let rendered = sink.text(Stream::Stderr)?;
            assert!(rendered.contains(public));
            assert!(rendered.contains(connection_help));
            for secret in [
                "endpoint-user",
                "endpoint-password",
                "/private",
                "api_key",
                "endpoint-token",
                "fragment-secret",
            ] {
                assert!(!rendered.contains(secret));
            }
            assert!(sink.text(Stream::Stdout)?.is_empty());
            assert_eq!(calls.load(Ordering::SeqCst), 0);
            assert_eq!(reads.load(Ordering::SeqCst), 0);
        }
    }

    // Work errors already contextualized by an inner run retain one prefix
    // and their original recovery help through the outer cleanup boundary.
    let (client, _, _) = fixture(1_000_000_000).await?;
    let (ctx, _, _) = context(&[])?;
    let nested = &ctx;
    let error = ctx
        .run_with(&Connected(client.clone()), |client| async move {
            nested
                .run_with(&Connected(client), |_| async {
                    Err::<(), _>(
                        rtao_cli::error::CliError::from(rtao::client::ClientError::from(
                            rtao_rpc::RpcError::Transport("offline".into()),
                        ))
                        .with_help("retain the primary guidance"),
                    )
                })
                .await
        })
        .await
        .err()
        .ok_or("nested contextual connection error")?;
    assert_eq!(
        error.message,
        format!("could not reach {}: offline", ctx.globals.network)
    );
    assert_eq!(
        error.help.as_deref(),
        Some("retain the primary guidance; check the endpoint and network connection, then retry")
    );
    assert!(client.is_closed());

    let (client, _, _) = fixture(1_000_000_000).await?;
    let mut rendered = rtao_cli::error::CliError::from(rtao::client::ClientError::from(
        rtao_rpc::RpcError::Transport("offline".into()),
    ));
    rendered.message.clear();
    let rendered = ctx
        .run_with(&Connected(client.clone()), |_| async move {
            Err::<(), _>(rendered)
        })
        .await
        .err()
        .ok_or("already rendered connection error")?;
    assert!(rendered.message.is_empty());
    assert!(rendered.help.is_none());
    assert!(client.is_closed());

    // Node response/domain errors and ambiguous in-flight outcomes never
    // acquire an ordinary connection retry instruction.
    for cause in [
        rtao_rpc::RpcError::Protocol("malformed fixture".into()),
        rtao_rpc::RpcError::StateDiscarded {
            block: "0xold".into(),
        },
        rtao_rpc::RpcError::JsonRpc {
            code: -32000,
            message: "node refusal".into(),
            data: None,
            payload: Box::new(json!({"code":-32000,"message":"node refusal"})),
        },
        rtao_rpc::RpcError::ConnectionLost { resent: false },
        rtao_rpc::RpcError::SubscriptionLost,
    ] {
        let calls = AtomicUsize::new(0);
        let message = rtao::client::ClientError::from(cause.clone()).to_string();
        let error = ctx
            .run_with(&RefusedConnection(cause.into()), |_| async {
                calls.fetch_add(1, Ordering::SeqCst);
                Ok(())
            })
            .await
            .err()
            .ok_or("non-connection connector error")?;
        assert_eq!(calls.load(Ordering::SeqCst), 0);
        assert_eq!(error.message, message);
        assert!(error.help.is_none());
        assert!(error.diagnostic.is_none());
    }
    Ok(())
}

#[derive(Default)]
struct CallCache(Mutex<std::collections::BTreeMap<String, Value>>);
impl rtao_cli::call::RawCallCache for CallCache {
    fn save(
        &self,
        hash: String,
        spec: Value,
    ) -> BoxFuture<'_, Result<(), rtao_cli::error::CliError>> {
        Box::pin(async move {
            self.0
                .lock()
                .map_err(|_| rtao_cli::error::CliError::failure("fixture cache"))?
                .insert(hash, spec);
            Ok(())
        })
    }
}
struct MemberFactory {
    keys: std::collections::BTreeMap<AccountId, Keys>,
    acquired: AtomicUsize,
    releases: Mutex<Vec<bool>>,
    fail_source_at: Option<usize>,
    fail_release: bool,
    source_override: Option<AccountId>,
    on_release: Option<Arc<dyn Fn() -> Result<(), rtao_cli::error::CliError> + Send + Sync>>,
}
impl rtao_cli::call::RawSignerFactory for MemberFactory {
    fn source<'a>(
        &'a self,
        _: &'a AppContext,
        selection: &'a rtao_cli::call::SigningSelection,
    ) -> BoxFuture<'a, Result<Arc<dyn DynKeySource>, rtao_cli::error::CliError>> {
        Box::pin(async move {
            let attempt = self.acquired.fetch_add(1, Ordering::SeqCst) + 1;
            if self.fail_source_at == Some(attempt) {
                return Err(rtao_cli::error::CliError::failure(
                    "fixture source unavailable",
                ));
            }
            let key = self
                .keys
                .get(
                    &self
                        .source_override
                        .or(selection.address)
                        .ok_or_else(|| rtao_cli::error::CliError::input("fixture member"))?,
                )
                .ok_or_else(|| rtao_cli::error::CliError::input("fixture source"))?;
            Ok(Arc::new(Keys {
                key: key.key.clone(),
                unlocks: key.unlocks.clone(),
            }) as Arc<dyn DynKeySource>)
        })
    }
    fn release<'a>(
        &'a self,
        _: &'a rtao_cli::call::SigningSelection,
        _: &'a dyn DynKeySource,
        success: bool,
    ) -> BoxFuture<'a, Result<(), rtao_cli::error::CliError>> {
        Box::pin(async move {
            self.releases
                .lock()
                .map_err(|_| rtao_cli::error::CliError::failure("fixture release"))?
                .push(success);
            if let Some(action) = &self.on_release {
                action()?;
            }
            if self.fail_release {
                return Err(rtao_cli::error::CliError::failure("fixture close failed"));
            }
            Ok(())
        })
    }
}
fn members() -> Result<Vec<Keys>, Box<dyn std::error::Error>> {
    (19u8..22)
        .map(|seed| {
            Ok(Keys {
                key: KeypairSigner::from_seed(&[seed; 32], CryptoType::Ed25519)?,
                unlocks: Arc::new(AtomicUsize::new(0)),
            })
        })
        .collect()
}
fn saved_context(
    keys: &[Keys],
    flags: &[&str],
    count: usize,
) -> Result<(AppContext, MemorySink), Box<dyn std::error::Error>> {
    let (mut ctx, sink, _) = context(flags)?;
    ctx.globals.wallet_name = "team".into();
    ctx.globals.wallet_given = true;
    ctx.globals.signatories = keys
        .iter()
        .take(count)
        .map(|keys| format!("{}=extension", keys.key.account()))
        .collect();
    ctx.books = Books::from_values(
        json!([]),
        json!([]),
        json!([{"name":"team","threshold":3,"signatories":keys.iter().map(|keys|keys.key.account().to_string()).collect::<Vec<_>>()}]),
    );
    Ok((ctx, sink))
}
fn factory(keys: &[Keys]) -> MemberFactory {
    MemberFactory {
        keys: keys
            .iter()
            .map(|key| {
                (
                    key.key.account(),
                    Keys {
                        key: key.key.clone(),
                        unlocks: key.unlocks.clone(),
                    },
                )
            })
            .collect(),
        acquired: AtomicUsize::new(0),
        releases: Mutex::new(Vec::new()),
        fail_source_at: None,
        fail_release: false,
        source_override: None,
        on_release: None,
    }
}
fn success_events(executed: Option<(AccountId, &rtao::tx::Multisig, [u8; 32])>) -> Value {
    let mut events = vec![
        json!({"phase":"ApplyExtrinsic","extrinsic_idx":0,"module_id":"System","event_id":"ExtrinsicSuccess","attributes":{"dispatch_info":{"weight":{"ref_time":1000,"proof_size":0},"class":"Normal","pays_fee":"Yes"}},"topics":[]}),
    ];
    if let Some((approving, group, hash)) = executed {
        events.push(json!({"phase":"ApplyExtrinsic","extrinsic_idx":0,"module_id":"Multisig","event_id":"MultisigExecuted","attributes":{"approving":approving,"timepoint":{"height":10,"index":1},"multisig":group.address,"call_hash":format!("0x{}",hex::encode(hash)),"result":{"Ok":[]}},"topics":[]}));
    }
    json!(events)
}
#[tokio::test]
async fn saved_semantic_dry_run_and_refusal_do_not_acquire_any_source() -> TestResult {
    let keys = members()?;
    let (client, chain, _) = fixture(1_000_000_000).await?;
    let factory = factory(&keys);
    let cache = CallCache::default();
    let (ctx, sink) = saved_context(&keys, &["--dry-run", "--json", "--no-mev-shield"], 2)?;
    assert!(
        ctx.submit_saved_with_factory(
            &client,
            &Preview { blocked: false },
            false,
            Some("rtao tx fixture"),
            &factory,
            &cache
        )
        .await?
        .is_none()
    );
    let plan: Value = serde_json::from_str(&sink.text(Stream::Stdout)?)?;
    assert_eq!(plan["op"], json!("multisig_execute"));
    assert_eq!(factory.acquired.load(Ordering::SeqCst), 0);
    assert!(ctx.output.address_names()?.is_empty());
    assert!(ctx.output.address_kinds()?.is_empty());
    let (ctx, _) = saved_context(&keys, &["--json", "--no-mev-shield"], 2)?;
    let error = ctx
        .submit_saved_with_factory(
            &client,
            &Preview { blocked: false },
            false,
            None,
            &factory,
            &cache,
        )
        .await
        .err()
        .ok_or("refusal")?;
    assert_eq!(error.code, 1);
    assert_eq!(factory.acquired.load(Ordering::SeqCst), 0);
    assert!(
        chain
            .state()
            .lock()
            .map_err(|_| "chain")?
            .submissions
            .is_empty()
    );
    let root = CacheRoot::new()?;
    let wallet_names = ["member-a", "member-b"];
    let mut sentinels = Vec::new();
    let mut hotkeys = Vec::new();
    for (index, name) in wallet_names.iter().enumerate() {
        let wallet = root.0.join(name);
        std::fs::create_dir_all(wallet.join("hotkeys"))?;
        std::fs::write(
            wallet.join("coldkeypub.txt"),
            serde_json::to_vec(&json!({
                "ss58Address":keys[index].key.account(),
                "cryptoType":u8::from(keys[index].key.crypto_type()),
            }))?,
        )?;
        let hotkey = AccountId([u8::try_from(80 + index)?; 32]);
        hotkeys.push(hotkey);
        std::fs::write(
            wallet.join("hotkeys/unusedpub.txt"),
            serde_json::to_vec(&json!({"ss58Address":hotkey,"cryptoType":1}))?,
        )?;
        for path in [wallet.join("coldkey"), wallet.join("hotkeys/unused")] {
            let bytes = b"public approval review must not unlock this key".to_vec();
            std::fs::write(&path, &bytes)?;
            sentinels.push((path, bytes));
        }
    }
    for existing_name in [None, Some("already selected")] {
        let (mut ctx, sink) = saved_context(&keys, &["--dry-run", "--no-mev-shield"], 2)?;
        ctx.globals.wallet_path = root.0.clone();
        ctx.globals.hotkey_name = "unused".into();
        ctx.globals.signatories = wallet_names
            .iter()
            .map(|name| format!("{name}=wallet"))
            .collect();
        if let Some(name) = existing_name {
            ctx.output
                .name_address(&keys[0].key.account().to_string(), name)?;
        }
        ctx.books.addresses = vec![
            json!({"address":keys[0].key.account(),"name":"first contact"}),
            json!({"address":keys[1].key.account(),"name":"later contact"}),
            json!({"address":keys[2].key.account(),"name":"unused contact"}),
        ];
        ctx.books.proxies = vec![
            json!({"address":keys[1].key.account(),"name":"later proxy"}),
            json!({"address":keys[2].key.account(),"name":"later proxy alias"}),
        ];
        assert!(
            ctx.submit_saved_with_factory(
                &client,
                &Preview { blocked: false },
                false,
                Some("rtao tx fixture"),
                &factory,
                &cache,
            )
            .await?
            .is_none()
        );
        let first_name = existing_name.unwrap_or(wallet_names[0]);
        assert_eq!(
            ctx.output.address_names()?,
            std::collections::BTreeMap::from([
                (keys[0].key.account().to_string(), first_name.to_owned()),
                (
                    keys[1].key.account().to_string(),
                    wallet_names[1].to_owned()
                ),
                (
                    keys[2].key.account().to_string(),
                    "unused contact".to_owned()
                ),
            ])
        );
        assert_eq!(
            ctx.output.address_kinds()?,
            std::collections::BTreeMap::from([
                (
                    keys[0].key.account().to_string(),
                    rtao_cli::books::AccountKind::Coldkey
                ),
                (
                    keys[1].key.account().to_string(),
                    rtao_cli::books::AccountKind::Coldkey
                ),
            ])
        );
        let names = ctx.output.address_names()?;
        assert!(
            hotkeys
                .iter()
                .all(|hotkey| !names.contains_key(&hotkey.to_string()))
        );
        assert!(
            sink.text(Stream::Stdout)?
                .contains(&format!("{first_name} ({})", keys[0].key.account()))
        );
        assert_eq!(factory.acquired.load(Ordering::SeqCst), 0);
        assert!(factory.releases.lock().map_err(|_| "release")?.is_empty());
    }
    for (path, bytes) in sentinels {
        assert_eq!(std::fs::read(path)?, bytes);
    }
    assert!(
        keys.iter()
            .all(|key| key.unlocks.load(Ordering::SeqCst) == 0)
    );
    assert!(chain.mutate_state(|state| state.submissions.is_empty()));
    assert!(
        chain
            .requests()
            .iter()
            .all(|request| !request.method.starts_with("author_"))
    );
    client.close().await;
    Ok(())
}
#[tokio::test]
async fn saved_semantic_funding_help_survives_planning_refusal_before_source() -> TestResult {
    let keys = members()?;
    let (client, chain, _) = fixture(0).await?;
    chain.mutate_state(|state| state.fee = 100_000);
    let factory = factory(&keys);
    let (mut ctx, _) = saved_context(&keys, &["--yes", "--json", "--no-mev-shield"], 2)?;
    ctx.books.multisigs[0]["threshold"] = json!(2);
    let error = ctx
        .submit_saved_with_factory(
            &client,
            &Preview { blocked: false },
            false,
            None,
            &factory,
            &CallCache::default(),
        )
        .await
        .err()
        .ok_or("semantic planning must refuse")?;
    let expected_help = format!(
        "fund {} with ≥ τ0.197 — the deposit is returned when the op executes",
        keys[0].key.account()
    );
    assert_eq!(error.code, 1);
    assert!(
        error
            .message
            .contains("multisig deposit plus the transaction fee")
    );
    assert_eq!(error.help.as_deref(), Some(expected_help.as_str()));
    assert_eq!(factory.acquired.load(Ordering::SeqCst), 0);
    assert!(
        keys.iter()
            .all(|key| key.unlocks.load(Ordering::SeqCst) == 0)
    );
    assert!(chain.mutate_state(|state| state.submissions.is_empty()));
    client.close().await;
    Ok(())
}

#[tokio::test]
async fn saved_semantic_executed_first_round_stops_and_emits_one_receipt() -> TestResult {
    let keys = members()?;
    let (client, chain, _) = fixture(1_000_000_000).await?;
    let runtime = client.refresh_runtime().await?;
    let group = rtao::tx::Multisig::new(
        &runtime,
        &keys
            .iter()
            .map(|keys| keys.key.account())
            .collect::<Vec<_>>(),
        3,
    )?;
    let inner = rtao_runtime::calls::system::remark(vec![8, 9]).compose(&runtime)?;
    chain.mutate_state(|state| {
        state.queue_submit(rtao_testkit::SubmitScript::InBlockThenFinalized {
            events: success_events(Some((keys[0].key.account(), &group, inner.hash()))),
        })
    });
    let member_factory = factory(&keys);
    let cache = CallCache::default();
    let (ctx, sink) = saved_context(&keys, &["--yes", "--json", "--no-mev-shield"], 3)?;
    let result = ctx
        .submit_saved_with_factory(
            &client,
            &Preview { blocked: false },
            false,
            None,
            &member_factory,
            &cache,
        )
        .await?
        .ok_or("receipt")?;
    assert!(result.success);
    assert_eq!(member_factory.acquired.load(Ordering::SeqCst), 1);
    assert_eq!(
        *member_factory.releases.lock().map_err(|_| "release")?,
        vec![true]
    );
    assert_eq!(keys[0].unlocks.load(Ordering::SeqCst), 1);
    assert_eq!(keys[1].unlocks.load(Ordering::SeqCst), 0);
    assert_eq!(
        chain.state().lock().map_err(|_| "chain")?.submissions.len(),
        1
    );
    assert_eq!(
        result.data["multisig_followup"]["status"],
        json!("executed")
    );
    assert_eq!(
        serde_json::from_str::<Value>(&sink.text(Stream::Stdout)?)?,
        result.to_json()
    );
    client.close().await;
    // A saved operation also retains the last round's actual transport.
    // The mixed case opens unshielded, then gains a key and encrypts round two.
    for (required, downgraded, mixed_rounds) in [
        (false, false, false),
        (true, false, false),
        (false, true, false),
        (false, false, true),
    ] {
        let keys = members()?;
        let (client, chain, _) = fixture(1_000_000_000).await?;
        let mut factory = factory(&keys);
        if mixed_rounds {
            let group = saved_group(&client, &keys).await?;
            let call = rtao_runtime::calls::system::remark(vec![8, 9])
                .compose(&client.refresh_runtime().await?)?;
            publish_pending_after_release(
                &mut factory,
                chain.clone(),
                &group,
                keys[0].key.account(),
                &call,
            );
            let publish = factory.on_release.take().ok_or("opening publisher")?;
            let next_chain = chain.clone();
            factory.on_release = Some(Arc::new(move || {
                publish()?;
                next_chain
                    .mutate_state(|state| {
                        state.seed_python("MevShield", "NextKey", &[], json!(vec![0u8; 1184]))?;
                        state.queue_reply(
                            "author_submitAndWatchExtrinsic",
                            rtao_testkit::Reply::error(
                                1010,
                                "Invalid Transaction",
                                Some(json!(
                                    "Inability to pay some fees, e.g. because account balance is too low"
                                )),
                            ),
                        );
                        Ok::<_, rtao::core::CoreError>(())
                    })
                    .map_err(|error| rtao_cli::error::CliError::failure(error.to_string()))
            }));
        } else {
            chain.mutate_state(|state| -> TestResult {
                if !downgraded {
                    state.seed_python("MevShield", "NextKey", &[], json!(vec![0u8; 1184]))?;
                }
                state.queue_reply(
                    "author_submitAndWatchExtrinsic",
                    rtao_testkit::Reply::error(
                        1010,
                        "Invalid Transaction",
                        Some(json!(
                            "Inability to pay some fees, e.g. because account balance is too low"
                        )),
                    ),
                );
                Ok(())
            })?;
        }
        let (ctx, sink) = saved_context(&keys, &["--yes"], 2)?;
        let error = ctx
            .submit_saved_with_factory(
                &client,
                &ShieldFailurePreview { required },
                false,
                None,
                &factory,
                &CallCache::default(),
            )
            .await
            .err()
            .ok_or("saved carrier/fallback receipt must fail")?;
        assert_eq!(error.code, 1);
        let stderr = sink.text(Stream::Stderr)?;
        assert_eq!(
            stderr.matches("the submission used MEV shielding;").count(),
            usize::from(!downgraded),
            "{stderr}"
        );
        if required {
            assert!(!stderr.contains("--no-mev-shield"), "{stderr}");
            assert!(stderr.contains("requires MEV shielding"));
        }
        let rounds = if mixed_rounds { 2 } else { 1 };
        assert_eq!(factory.acquired.load(Ordering::SeqCst), rounds);
        {
            let released = factory.releases.lock().map_err(|_| "carrier releases")?;
            assert_eq!(released.len(), rounds);
            assert_eq!(released.last(), Some(&false));
            if mixed_rounds {
                assert_eq!(released.first(), Some(&true));
            }
        }
        assert_eq!(
            chain
                .requests()
                .iter()
                .filter(|r| r.method == "author_submitAndWatchExtrinsic")
                .count(),
            rounds
        );
        client.close().await;
    }
    Ok(())
}

fn publish_pending_after_release(
    factory: &mut MemberFactory,
    chain: FakeChain,
    group: &rtao::tx::Multisig,
    member: AccountId,
    call: &rtao_runtime::CallBytes,
) {
    let group = group.address;
    let hash = format!("0x{}", hex::encode(call.hash()));
    chain.mutate_state(|state| {
        let mut events = success_events(None).as_array().cloned().unwrap_or_default();
        events.push(json!({"phase":"ApplyExtrinsic","extrinsic_idx":0,"module_id":"Multisig","event_id":"NewMultisig","attributes":{"approving":member,"multisig":group,"call_hash":hash},"topics":[]}));
        state.queue_submit(rtao_testkit::SubmitScript::InBlockThenFinalized { events:json!(events) });
    });
    factory.on_release = Some(Arc::new(move || {
        chain
            .mutate_state(|state| {
                let height = state.block;
                state.seed_python(
                    "Multisig",
                    "Multisigs",
                    &[json!(group), json!(hash)],
                    json!({"when":{"height":height,"index":0},"deposit":0,"depositor":member,"approvals":[member]}),
                )
            })
            .map_err(|error| rtao_cli::error::CliError::failure(error.to_string()))
    }));
}

#[tokio::test]
async fn saved_semantic_replaced_opening_refuses_before_second_source() -> TestResult {
    let keys = members()?;
    let (client, chain, _) = fixture(1_000_000_000).await?;
    let group = saved_group(&client, &keys).await?;
    let call = rtao_runtime::calls::system::remark(vec![8, 9])
        .compose(&client.refresh_runtime().await?)?;
    let mut factory = factory(&keys);
    publish_pending_after_release(
        &mut factory,
        chain.clone(),
        &group,
        keys[0].key.account(),
        &call,
    );
    let group_address = group.address;
    let call_hash = format!("0x{}", hex::encode(call.hash()));
    let member = keys[0].key.account();
    let changed_chain = chain.clone();
    factory.on_release = Some(Arc::new(move || {
        changed_chain.mutate_state(|state| state.seed_python(
        "Multisig", "Multisigs", &[json!(group_address), json!(call_hash)],
        json!({"when":{"height":999,"index":7},"deposit":0,"depositor":member,"approvals":[member]}),
    )).map_err(|error| rtao_cli::error::CliError::failure(error.to_string()))
    }));
    let (ctx, _) = saved_context(&keys, &["--yes", "--json", "--no-mev-shield"], 3)?;
    let error = ctx
        .submit_saved_with_factory(
            &client,
            &Preview { blocked: false },
            false,
            None,
            &factory,
            &CallCache::default(),
        )
        .await
        .err()
        .ok_or("replaced opening refusal")?;
    assert!(error.message.contains("replaced"));
    assert!(
        error
            .help
            .ok_or("recovery")?
            .contains(&hex::encode(call.hash()))
    );
    assert_eq!(factory.acquired.load(Ordering::SeqCst), 1);
    assert_eq!(keys[1].unlocks.load(Ordering::SeqCst), 0);
    assert_eq!(
        chain.state().lock().map_err(|_| "chain")?.submissions.len(),
        1
    );
    client.close().await;
    Ok(())
}

#[tokio::test]
async fn saved_semantic_missing_opening_identity_refuses_automatic_continuation() -> TestResult {
    let keys = members()?;
    let (client, chain, _) = fixture(1_000_000_000).await?;
    let call = rtao_runtime::calls::system::remark(vec![8, 9])
        .compose(&client.refresh_runtime().await?)?;
    let factory = factory(&keys);
    let (ctx, _) = saved_context(&keys, &["--yes", "--json", "--no-mev-shield"], 3)?;
    let error = ctx
        .submit_saved_with_factory(
            &client,
            &Preview { blocked: false },
            false,
            None,
            &factory,
            &CallCache::default(),
        )
        .await
        .err()
        .ok_or("unknown opening refusal")?;
    assert!(error.message.contains("timepoint") || error.message.contains("opening"));
    assert!(
        error
            .help
            .ok_or("recovery")?
            .contains(&hex::encode(call.hash()))
    );
    assert_eq!(factory.acquired.load(Ordering::SeqCst), 1);
    assert_eq!(keys[1].unlocks.load(Ordering::SeqCst), 0);
    assert_eq!(
        chain.state().lock().map_err(|_| "chain")?.submissions.len(),
        1
    );
    client.close().await;
    Ok(())
}
async fn saved_group(
    client: &Client,
    keys: &[Keys],
) -> Result<rtao::tx::Multisig, Box<dyn std::error::Error>> {
    Ok(rtao::Executor::new(client.clone())
        .multisig(
            &keys
                .iter()
                .map(|keys| keys.key.account())
                .collect::<Vec<_>>(),
            3,
        )
        .await?)
}

struct ChangingPreview(AtomicUsize);
impl IntentInterface for ChangingPreview {
    const SPEC: &'static rtao_catalog::IntentSpec = Preview::SPEC;
    type Role = Coldkey;
    type Origin = Signed;
    type Output = ();
    const WRAPS: &'static [&'static rtao_runtime::CallDescriptor] = Preview::WRAPS;
    fn to_json(&self) -> Value {
        json!({"op":"add_stake"})
    }
}
impl Intent for ChangingPreview {
    fn summary(&self) -> String {
        "review changing-byte fixture".into()
    }
    async fn build<V: View + ?Sized>(&self, cx: &BuildCx<'_, V>) -> Result<Built, IntentError> {
        let build = u8::try_from(self.0.fetch_add(1, Ordering::SeqCst))
            .map_err(|_| IntentError::InvalidArgument("fixture build count".into()))?;
        Ok(Built::new(
            rtao_runtime::calls::system::remark(vec![build]).compose(cx.runtime)?,
        ))
    }
}

#[tokio::test]
async fn saved_semantic_two_rounds_reuse_first_submitted_bytes_and_keep_the_original_context()
-> TestResult {
    let keys = members()?;
    let (client, chain, _) = fixture(1_000_000_000).await?;
    let group = saved_group(&client, &keys).await?;
    let runtime = client.refresh_runtime().await?;
    // The first round reviews once, then the executor builds its actual call.
    // Later rounds must retain that submitted [1] call, never rebuild [2].
    let call = rtao_runtime::calls::system::remark(vec![1]).compose(&runtime)?;
    let mut factory = factory(&keys);
    publish_pending_after_release(
        &mut factory,
        chain.clone(),
        &group,
        keys[0].key.account(),
        &call,
    );
    chain.mutate_state(|state| {
        state.queue_submit(rtao_testkit::SubmitScript::InBlockThenFinalized {
            events: success_events(Some((keys[1].key.account(), &group, call.hash()))),
        })
    });
    let (ctx, sink) = saved_context(&keys, &["--yes", "--json", "--no-mev-shield"], 2)?;
    let intent = ChangingPreview(AtomicUsize::new(0));
    let hooks = Hooks {
        before: AtomicUsize::new(0),
        after: AtomicUsize::new(0),
        failed: AtomicUsize::new(0),
    };
    let result = ctx
        .submit_saved_with_options(
            &client,
            &intent,
            rtao_cli::context::SubmissionOptions {
                hooks: Some(&hooks),
                ..Default::default()
            },
            &factory,
            &CallCache::default(),
        )
        .await?
        .ok_or("receipt")?;
    assert_eq!(intent.0.load(Ordering::SeqCst), 2);
    assert_eq!(factory.acquired.load(Ordering::SeqCst), 2);
    assert_eq!(hooks.before.load(Ordering::SeqCst), 2);
    assert_eq!(hooks.after.load(Ordering::SeqCst), 2);
    assert_eq!(hooks.failed.load(Ordering::SeqCst), 0);
    assert_eq!(
        *factory.releases.lock().map_err(|_| "releases")?,
        vec![true, true]
    );
    assert_eq!(keys[0].unlocks.load(Ordering::SeqCst), 1);
    assert_eq!(keys[1].unlocks.load(Ordering::SeqCst), 1);
    let submissions = chain
        .state()
        .lock()
        .map_err(|_| "chain")?
        .submissions
        .clone();
    assert_eq!(submissions.len(), 2);
    for bytes in &submissions {
        let decoded =
            rtao::core::DynValue::new(runtime.core().decode_extrinsic(bytes, true)?).to_json();
        let args = decoded["call"]["call_args"]
            .as_array()
            .ok_or("as_multi args")?;
        let inner = args
            .iter()
            .find(|arg| arg["name"] == json!("call"))
            .ok_or("inner call")?;
        assert_eq!(
            inner["value"],
            rtao::core::DynValue::new(runtime.core().decode_spec(
                &rtao::core::runtime::TypeSpec::Call,
                call.as_bytes(),
                true
            )?)
            .to_json()
        );
    }
    assert_eq!(
        result.data["multisig_followup"]["status"],
        json!("executed")
    );
    assert_eq!(
        result.data["multisig_followup"]["call_data"],
        json!(format!("0x{}", hex::encode(call.as_bytes())))
    );
    assert_eq!(
        serde_json::from_str::<Value>(&sink.text(Stream::Stdout)?)?,
        result.to_json()
    );
    assert_eq!(ctx.globals.wallet_name, "team");
    assert!(ctx.globals.signer_backend.is_none());
    assert!(ctx.globals.signer_address.is_none());
    client.close().await;
    Ok(())
}

#[tokio::test]
async fn saved_semantic_one_confirmation_covers_identical_later_approvals() -> TestResult {
    let keys = members()?;
    let (client, chain, _) = fixture(1_000_000_000).await?;
    let group = saved_group(&client, &keys).await?;
    let call = rtao_runtime::calls::system::remark(vec![8, 9])
        .compose(&client.refresh_runtime().await?)?;
    let mut factory = factory(&keys);
    publish_pending_after_release(
        &mut factory,
        chain.clone(),
        &group,
        keys[0].key.account(),
        &call,
    );
    chain.mutate_state(|state| {
        state.queue_submit(rtao_testkit::SubmitScript::InBlockThenFinalized {
            events: success_events(Some((keys[1].key.account(), &group, call.hash()))),
        })
    });
    let (mut ctx, _) = saved_context(&keys, &["--no-mev-shield"], 2)?;
    let reads = Arc::new(AtomicUsize::new(0));
    ctx.terminal = Arc::new(Mutex::new(Box::new(TerminalFixture {
        tty: true,
        lines: VecDeque::from(["yes".into()]),
        reads: reads.clone(),
    })));
    let result = ctx
        .submit_saved_with_factory(
            &client,
            &Preview { blocked: false },
            false,
            None,
            &factory,
            &CallCache::default(),
        )
        .await?
        .ok_or("receipt")?;
    assert!(rtao_cli::call::multisig_executed(&result));
    assert_eq!(reads.load(Ordering::SeqCst), 1);
    assert_eq!(factory.acquired.load(Ordering::SeqCst), 2);
    assert_eq!(
        *factory.releases.lock().map_err(|_| "releases")?,
        vec![true, true]
    );
    client.close().await;
    Ok(())
}

#[tokio::test]
async fn saved_semantic_later_source_failure_retains_exact_pending_recovery() -> TestResult {
    let keys = members()?;
    let (client, chain, _) = fixture(1_000_000_000).await?;
    let group = saved_group(&client, &keys).await?;
    let call = rtao_runtime::calls::system::remark(vec![8, 9])
        .compose(&client.refresh_runtime().await?)?;
    let mut factory = factory(&keys);
    factory.fail_source_at = Some(2);
    publish_pending_after_release(
        &mut factory,
        chain.clone(),
        &group,
        keys[0].key.account(),
        &call,
    );
    let (ctx, sink) = saved_context(&keys, &["--yes", "--json", "--no-mev-shield"], 3)?;
    let error = ctx
        .submit_saved_with_factory(
            &client,
            &Preview { blocked: false },
            false,
            None,
            &factory,
            &CallCache::default(),
        )
        .await
        .err()
        .ok_or("source failure")?;
    assert_eq!(error.message, "fixture source unavailable");
    let help = error.help.ok_or("pending recovery")?;
    assert!(help.contains("earlier approval(s) may remain pending"));
    assert!(help.contains("rtao multisig pending"));
    assert!(help.contains(&hex::encode(call.hash())));
    assert_eq!(factory.acquired.load(Ordering::SeqCst), 2);
    assert_eq!(
        *factory.releases.lock().map_err(|_| "releases")?,
        vec![true]
    );
    assert_eq!(keys[1].unlocks.load(Ordering::SeqCst), 0);
    assert_eq!(
        chain.state().lock().map_err(|_| "chain")?.submissions.len(),
        1
    );
    assert!(sink.text(Stream::Stdout)?.is_empty());
    client.close().await;
    Ok(())
}

#[tokio::test]
async fn saved_semantic_close_failure_stops_next_member_and_preserves_pending_hash() -> TestResult {
    let keys = members()?;
    let (client, chain, _) = fixture(1_000_000_000).await?;
    let group = saved_group(&client, &keys).await?;
    let call = rtao_runtime::calls::system::remark(vec![8, 9])
        .compose(&client.refresh_runtime().await?)?;
    let mut factory = factory(&keys);
    factory.fail_release = true;
    publish_pending_after_release(
        &mut factory,
        chain.clone(),
        &group,
        keys[0].key.account(),
        &call,
    );
    let (ctx, _) = saved_context(&keys, &["--yes", "--json", "--no-mev-shield"], 3)?;
    let error = ctx
        .submit_saved_with_factory(
            &client,
            &Preview { blocked: false },
            false,
            None,
            &factory,
            &CallCache::default(),
        )
        .await
        .err()
        .ok_or("close failure")?;
    assert_eq!(error.message, "fixture close failed");
    assert!(
        error
            .help
            .ok_or("pending recovery")?
            .contains(&hex::encode(call.hash()))
    );
    assert_eq!(factory.acquired.load(Ordering::SeqCst), 1);
    assert_eq!(keys[1].unlocks.load(Ordering::SeqCst), 0);
    assert_eq!(
        chain.state().lock().map_err(|_| "chain")?.submissions.len(),
        1
    );
    client.close().await;
    Ok(())
}

#[tokio::test]
async fn saved_semantic_identity_error_stays_primary_when_source_close_also_fails() -> TestResult {
    let keys = members()?;
    let (client, chain, _) = fixture(1_000_000_000).await?;
    let mut factory = factory(&keys);
    factory.source_override = Some(keys[1].key.account());
    factory.fail_release = true;
    let (ctx, _) = saved_context(&keys, &["--yes", "--json", "--no-mev-shield"], 3)?;
    let error = ctx
        .submit_saved_with_factory(
            &client,
            &Preview { blocked: false },
            false,
            None,
            &factory,
            &CallCache::default(),
        )
        .await
        .err()
        .ok_or("identity failure")?;
    assert_eq!(error.code, 2);
    assert!(
        error
            .message
            .contains("does not match the reviewed approval member")
    );
    assert_eq!(
        error.help.as_deref(),
        Some("post-submission cleanup also failed: fixture close failed")
    );
    assert_eq!(factory.acquired.load(Ordering::SeqCst), 1);
    assert_eq!(
        *factory.releases.lock().map_err(|_| "releases")?,
        vec![false]
    );
    assert!(
        chain
            .state()
            .lock()
            .map_err(|_| "chain")?
            .submissions
            .is_empty()
    );
    assert!(
        keys.iter()
            .all(|key| key.unlocks.load(Ordering::SeqCst) == 0)
    );
    client.close().await;
    Ok(())
}
// These public-only sources exercise context/executor ordering without opening
// a device, browser, camera or external signing transport.
struct NoticeMemberFactory<'a>(&'a MemberFactory);
impl rtao_cli::call::RawSignerFactory for NoticeMemberFactory<'_> {
    fn source<'a>(
        &'a self,
        ctx: &'a AppContext,
        selection: &'a rtao_cli::call::SigningSelection,
    ) -> BoxFuture<'a, Result<Arc<dyn DynKeySource>, rtao_cli::error::CliError>> {
        Box::pin(async move {
            let public = rtao_cli::call::RawSignerFactory::source(self.0, ctx, selection).await?;
            Ok(Arc::new(LabelOnlyKeys { public }) as Arc<dyn DynKeySource>)
        })
    }
    fn release<'a>(
        &'a self,
        selection: &'a rtao_cli::call::SigningSelection,
        keys: &'a dyn DynKeySource,
        success: bool,
    ) -> BoxFuture<'a, Result<(), rtao_cli::error::CliError>> {
        rtao_cli::call::RawSignerFactory::release(self.0, selection, keys, success)
    }
}

#[tokio::test]
async fn shield_signing_notices_use_actual_backends_and_respect_machine_quiet_and_preview()
-> TestResult {
    for (backend, expected) in [
        (
            "vault",
            "MEV-shielded vault signing: two scans, ~90 seconds total — have the phone unlocked with Vault's scanner open",
        ),
        (
            "extension",
            "MEV-shielded extension signing: two approvals, ~90 seconds total — have the extension unlocked",
        ),
        (
            "ledger",
            "MEV-shielded Ledger signing: two approvals on the device, ~90 seconds total — keep the Polkadot app open",
        ),
        (
            "\u{1c}\u{85}LeDgEr\u{1f}",
            "MEV-shielded Ledger signing: two approvals on the device, ~90 seconds total — keep the Polkadot app open",
        ),
    ] {
        for (mode, shield, dry, visible) in [
            (None, true, false, true),
            (Some("--json"), true, false, false),
            (Some("--quiet"), true, false, false),
            (Some("--dry-run"), true, true, false),
            (None, false, false, false),
        ] {
            let (client, chain, keys) = fixture(1_000_000_000).await?;
            chain.mutate_state(|state| {
                state.seed_python("MevShield", "NextKey", &[], json!(vec![171u8; 1184]))
            })?;
            let mut flags = vec![
                "--yes",
                "--signer",
                backend,
                if shield {
                    "--mev-shield"
                } else {
                    "--no-mev-shield"
                },
            ];
            if let Some(mode) = mode {
                flags.push(mode);
            }
            let (ctx, sink, reads) = context(&flags)?;
            let public = Arc::new(Keys {
                key: keys.key.clone(),
                unlocks: keys.unlocks.clone(),
            });
            let source = LabelOnlyKeys { public };
            let result = ctx
                .submit_with_keys_options(
                    &client,
                    &Preview { blocked: false },
                    &source,
                    Default::default(),
                )
                .await;
            if dry {
                assert!(result?.is_none());
            } else {
                assert!(result.is_err());
            }
            let stderr = sink.text(Stream::Stderr)?;
            assert_eq!(
                stderr.matches(expected).count(),
                usize::from(visible),
                "{backend} {mode:?}"
            );
            assert_eq!(
                stderr.matches("~90 seconds total").count(),
                usize::from(visible)
            );
            assert_eq!(keys.unlocks.load(Ordering::SeqCst), 0);
            assert_eq!(reads.load(Ordering::SeqCst), 0);
            assert!(chain.mutate_state(|state| state.submissions.is_empty()));
            assert!(
                chain
                    .requests()
                    .iter()
                    .all(|request| !request.method.starts_with("author_"))
            );
            client.close().await;
        }
    }
    // Repeated approvals use each member's backend rather than the invocation's
    // Vault setting. A single reference follows the ordinary external route.
    let keys = members()?;
    let (client, chain, _) = fixture(1_000_000_000).await?;
    chain.mutate_state(|state| {
        state.seed_python("MevShield", "NextKey", &[], json!(vec![171u8; 1184]))?;
        for member in &keys {
            state.seed_python(
                "System",
                "Account",
                &[json!(member.key.account())],
                json!({"data":{"free":1_000_000_000u64}}),
            )?;
        }
        Ok::<_, rtao::core::CoreError>(())
    })?;
    let (mut ctx, sink) = saved_context(&keys, &["--yes", "--mev-shield"], 1)?;
    ctx.globals.signer_backend = Some("vault".into());
    ctx.globals.signatories = vec![
        format!("{}=ledger", keys[0].key.account()),
        format!("{}=extension", keys[1].key.account()),
    ];
    let inner = factory(&keys);
    let source = NoticeMemberFactory(&inner);
    let error = ctx
        .submit_saved_with_factory(
            &client,
            &Preview { blocked: false },
            false,
            None,
            &source,
            &CallCache::default(),
        )
        .await
        .err()
        .ok_or("public-only approval must refuse private signing")?;
    assert_eq!(
        error.message,
        SignerError::KeyUnavailable(SignerRole::Coldkey).to_string()
    );
    let stderr = sink.text(Stream::Stderr)?;
    assert_eq!(
        stderr
            .matches("MEV-shielded Ledger signing: two approvals on the device")
            .count(),
        1,
        "saved notice missing before {error:?}"
    );
    assert!(!stderr.contains("MEV-shielded vault signing"));
    assert_eq!(inner.acquired.load(Ordering::SeqCst), 1);
    assert_eq!(*inner.releases.lock().map_err(|_| "release")?, vec![false]);
    assert!(
        keys.iter()
            .all(|key| key.unlocks.load(Ordering::SeqCst) == 0)
    );
    assert!(chain.mutate_state(|state| state.submissions.is_empty()));
    assert!(
        chain
            .requests()
            .iter()
            .all(|request| !request.method.starts_with("author_"))
    );
    client.close().await;
    Ok(())
}

#[tokio::test]
async fn typed_gateway_refusals_keep_retry_help_through_context_and_close_without_retry()
-> TestResult {
    for (retry, expected) in [
        (Some("12"), "wait 12 seconds, then retry"),
        (
            Some("Wed, 21 Oct 2037 07:28:00 GMT"),
            "retry after Wed, 21 Oct 2037 07:28:00 GMT",
        ),
        (
            Some(""),
            "reduce the request or connection rate, then retry",
        ),
        (None, "reduce the request or connection rate, then retry"),
    ] {
        let policy = rtao_rpc::policy::classify_rpc_error(&json!({
            "code":-32005,"message":"slow down",
            "data":{"policy":"rate","retry_after":retry},
        }))
        .ok_or("typed gateway policy")?;
        let message = policy.to_string();
        let (client, chain, keys) = fixture(1_000_000_000).await?;
        chain.queue_transport_error(
            "chain_getHeader",
            rtao_testkit::rpc::Error::Client(Box::new(rtao_rpc::RpcError::Policy(Box::new(
                policy.clone(),
            )))),
        );
        let (ctx, _, reads) = context(&["--yes", "--no-mev-shield"])?;
        let error = ctx
            .submit_with(&client, &Preview { blocked: false }, &keys, false, None)
            .await
            .err()
            .ok_or("typed planning policy refusal")?;
        assert_eq!(error.code, 1);
        assert_eq!(error.message, message);
        assert_eq!(error.help.as_deref(), Some(expected));
        assert_eq!(keys.unlocks.load(Ordering::SeqCst), 0);
        assert_eq!(reads.load(Ordering::SeqCst), 0);
        assert!(chain.mutate_state(|state| state.submissions.is_empty()));
        assert!(
            chain
                .requests()
                .iter()
                .all(|request| !request.method.starts_with("author_"))
        );
        client.close().await;

        // Saved-wallet account derivation pins a real snapshot before creating
        // an adapter. Preserve a refusal without acquiring any member source.
        let (client, chain, _) = fixture(1_000_000_000).await?;
        let member_keys = members()?;
        let factory = factory(&member_keys);
        let (ctx, sink) = saved_context(&member_keys, &["--yes", "--json", "--no-mev-shield"], 3)?;
        let start = chain.requests().len();
        chain.queue_transport_error(
            "chain_getHeader",
            rtao_testkit::rpc::Error::Client(Box::new(rtao_rpc::RpcError::Policy(Box::new(
                policy.clone(),
            )))),
        );
        let error = ctx
            .submit_saved_with_factory(
                &client,
                &Preview { blocked: false },
                false,
                None,
                &factory,
                &CallCache::default(),
            )
            .await
            .err()
            .ok_or("saved account policy refusal")?;
        assert_eq!(error.code, 1);
        assert_eq!(error.message, message);
        assert_eq!(error.help.as_deref(), Some(expected));
        assert_eq!(factory.acquired.load(Ordering::SeqCst), 0);
        assert!(
            member_keys
                .iter()
                .all(|key| key.unlocks.load(Ordering::SeqCst) == 0)
        );
        let requests = chain.requests();
        assert_eq!(
            requests[start..]
                .iter()
                .filter(|request| request.method == "chain_getHeader")
                .count(),
            1
        );
        assert!(
            requests[start..]
                .iter()
                .all(|request| !request.method.starts_with("author_"))
        );
        assert!(chain.mutate_state(|state| state.submissions.is_empty()));
        assert!(sink.text(Stream::Stdout)?.is_empty());
        assert!(sink.text(Stream::Stderr)?.is_empty());
        client.close().await;

        // These public execute_query calls distinguish the outer historical
        // Client::at mapping from the dispatched ReadError::Client mapping.
        for (block, method) in [
            (Some(42), "chain_getBlockHash"),
            (None, "state_queryStorageAt"),
        ] {
            let (client, chain, keys) = fixture(1_000_000_000).await?;
            let start = chain.requests().len();
            chain.queue_transport_error(
                method,
                rtao_testkit::rpc::Error::Client(Box::new(rtao_rpc::RpcError::Policy(Box::new(
                    policy.clone(),
                )))),
            );
            let (ctx, sink, reads) = context(&["--json"])?;
            let request = rtao_cli::commands::generated::QueryRequest {
                name: "balance".parse()?,
                params: serde_json::Map::from_iter([(
                    "coldkey_ss58".into(),
                    json!(keys.key.account()),
                )]),
                block,
            };
            let error =
                rtao_cli::commands::generated::execute_query(&client, &request, &ctx.output)
                    .await
                    .err()
                    .ok_or("public query policy refusal")?;
            assert_eq!(error.code, 1);
            assert_eq!(error.message, message);
            assert_eq!(error.help.as_deref(), Some(expected));
            let requests = chain.requests();
            let attempts = requests
                .iter()
                .skip(start)
                .filter(|request| request.method == method)
                .collect::<Vec<_>>();
            assert_eq!(attempts.len(), 1, "{block:?}: {method}");
            if block.is_some() {
                assert_eq!(attempts[0].params, json!([42]));
                assert!(
                    requests
                        .iter()
                        .skip(start)
                        .all(|request| request.method != "state_queryStorageAt")
                );
            } else {
                // Typed View::storage goes through fetch and the snapshot's
                // one-key state_queryStorageAt batch, even for a single account.
                assert_eq!(
                    attempts[0].params[0]
                        .as_array()
                        .ok_or("storage batch")?
                        .len(),
                    1
                );
                assert!(
                    attempts[0].params[1]
                        .as_str()
                        .is_some_and(|hash| hash.starts_with("0x") && hash.len() == 66)
                );
                assert!(
                    requests
                        .iter()
                        .skip(start)
                        .all(|request| request.method != "state_getStorage")
                );
            }
            assert!(
                requests
                    .iter()
                    .skip(start)
                    .all(|request| !request.method.starts_with("author_"))
            );
            assert_eq!(reads.load(Ordering::SeqCst), 0);
            assert_eq!(keys.unlocks.load(Ordering::SeqCst), 0);
            assert!(chain.mutate_state(|state| state.submissions.is_empty()));
            assert!(sink.text(Stream::Stdout)?.is_empty());
            assert!(sink.text(Stream::Stderr)?.is_empty());
            client.close().await;
        }

        let (client, chain, keys) = fixture(1_000_000_000).await?;
        chain.queue_transport_error(
            "system_health",
            rtao_testkit::rpc::Error::Client(Box::new(rtao_rpc::RpcError::Policy(Box::new(
                policy,
            )))),
        );
        let (ctx, sink, _) = context(&["--json"])?;
        let error = ctx
            .run_with(&Connected(client.clone()), |client| async move {
                client
                    .request::<Value>("system_health", json!([]))
                    .await
                    .map_err(rtao_cli::error::CliError::from)
            })
            .await
            .err()
            .ok_or("typed read policy refusal")?;
        assert_eq!(error.code, 1);
        assert_eq!(error.message, message);
        assert_eq!(error.help.as_deref(), Some(expected));
        assert!(client.is_closed());
        assert_eq!(
            chain
                .requests()
                .iter()
                .filter(|request| request.method == "system_health")
                .count(),
            1
        );
        assert_eq!(keys.unlocks.load(Ordering::SeqCst), 0);
        assert!(chain.mutate_state(|state| state.submissions.is_empty()));
        assert!(sink.text(Stream::Stdout)?.is_empty());
        assert!(sink.text(Stream::Stderr)?.is_empty());
    }
    // Unknown-fate metadata is distinct from gateway refusal. Keep the submitted
    // hash and its caution, without introducing wait-and-retry guidance.
    let (client, chain, keys) = fixture(1_000_000_000).await?;
    let hash = rtao_rpc::H256::from([29; 32]);
    let cause = rtao_rpc::RpcError::FateUnknown { hash };
    let message = cause.to_string();
    chain.queue_transport_error(
        "system_health",
        rtao_testkit::rpc::Error::Client(Box::new(cause)),
    );
    let (ctx, _, _) = context(&[])?;
    let error = ctx
        .run_with(&Connected(client.clone()), |client| async move {
            client
                .request::<Value>("system_health", json!([]))
                .await
                .map_err(rtao_cli::error::CliError::from)
        })
        .await
        .err()
        .ok_or("unknown fate")?;
    assert_eq!(error.message, message);
    let help = error.help.as_deref().ok_or("unknown-fate recovery hash")?;
    assert!(help.contains(&format!("inspect transaction 0x{}", hex::encode(hash.0))));
    assert!(help.contains("do not resubmit until its outcome is known"));
    assert!(!help.contains("wait "));
    assert!(!help.contains("request or connection rate"));
    assert!(client.is_closed());
    assert_eq!(keys.unlocks.load(Ordering::SeqCst), 0);
    assert!(chain.mutate_state(|state| state.submissions.is_empty()));
    assert_eq!(
        chain
            .requests()
            .iter()
            .filter(|request| request.method == "system_health")
            .count(),
        1
    );
    Ok(())
}

struct DirectExternalFactory {
    key: KeypairSigner,
    unlocks: Arc<AtomicUsize>,
    acquired: AtomicUsize,
    released: Mutex<Vec<bool>>,
    fail_release: bool,
}
impl DirectExternalFactory {
    fn new(keys: &Keys, fail_release: bool) -> Self {
        Self {
            key: keys.key.clone(),
            unlocks: keys.unlocks.clone(),
            acquired: AtomicUsize::new(0),
            released: Mutex::new(Vec::new()),
            fail_release,
        }
    }
}
impl rtao_cli::context::ExternalSignerFactory for DirectExternalFactory {
    fn keys<'a>(
        &'a self,
        globals: &'a Globals,
        role: SignerRole,
    ) -> BoxFuture<'a, Result<Arc<dyn DynKeySource>, rtao_cli::error::CliError>> {
        Box::pin(async move {
            assert_eq!(globals.signer_backend.as_deref(), Some("extension"));
            assert_eq!(role, SignerRole::Coldkey);
            self.acquired.fetch_add(1, Ordering::SeqCst);
            Ok(Arc::new(Keys {
                key: self.key.clone(),
                unlocks: self.unlocks.clone(),
            }) as Arc<dyn DynKeySource>)
        })
    }
    fn release<'a>(
        &'a self,
        globals: &'a Globals,
        source: &'a dyn DynKeySource,
        success: bool,
    ) -> BoxFuture<'a, Result<(), rtao_cli::error::CliError>> {
        Box::pin(async move {
            assert_eq!(globals.signer_backend.as_deref(), Some("extension"));
            assert_eq!(
                source
                    .coldkey_address()
                    .map_err(|error| rtao_cli::error::CliError::failure(error.to_string()))?,
                self.key.account()
            );
            self.released
                .lock()
                .map_err(|_| rtao_cli::error::CliError::failure("direct fixture release lock"))?
                .push(success);
            if self.fail_release {
                Err(rtao_cli::error::CliError::failure(
                    "direct close failure sentinel",
                ))
            } else {
                Ok(())
            }
        })
    }
}

#[tokio::test]
async fn direct_external_preview_releases_without_unlock_or_submission() -> TestResult {
    let (client, chain, keys) = fixture(1_000_000_000).await?;
    let factory = DirectExternalFactory::new(&keys, false);
    let (mut ctx, sink, reads) = context(&[
        "--dry-run",
        "--json",
        "--no-mev-shield",
        "--signer",
        "extension",
    ])?;
    let result = ctx
        .submit(
            &client,
            &Preview { blocked: false },
            false,
            None,
            Some(&factory),
            &mut rtao_cli::prompt::PromptHistory::default(),
        )
        .await?;
    assert!(result.is_none());
    assert_eq!(factory.acquired.load(Ordering::SeqCst), 1);
    assert_eq!(
        *factory.released.lock().map_err(|_| "release")?,
        vec![false]
    );
    assert_eq!(keys.unlocks.load(Ordering::SeqCst), 0);
    assert_eq!(reads.load(Ordering::SeqCst), 0);
    assert!(
        chain
            .state()
            .lock()
            .map_err(|_| "chain")?
            .submissions
            .is_empty()
    );
    let preview: Value = serde_json::from_str(&sink.text(Stream::Stdout)?)?;
    assert_eq!(preview["op"], json!("add_stake"));
    client.close().await;
    Ok(())
}

#[tokio::test]
async fn direct_external_confirmation_refusal_releases_without_unlock_or_submission() -> TestResult
{
    let (client, chain, keys) = fixture(1_000_000_000).await?;
    let factory = DirectExternalFactory::new(&keys, false);
    let (mut ctx, sink, reads) = context(&["--json", "--no-mev-shield", "--signer", "extension"])?;
    let error = ctx
        .submit(
            &client,
            &Preview { blocked: false },
            false,
            None,
            Some(&factory),
            &mut rtao_cli::prompt::PromptHistory::default(),
        )
        .await
        .err()
        .ok_or("direct refusal")?;
    assert_eq!(error.code, 1);
    assert!(error.message.contains("without confirmation"));
    assert_eq!(factory.acquired.load(Ordering::SeqCst), 1);
    assert_eq!(
        *factory.released.lock().map_err(|_| "release")?,
        vec![false]
    );
    assert_eq!(keys.unlocks.load(Ordering::SeqCst), 0);
    assert_eq!(reads.load(Ordering::SeqCst), 0);
    assert!(sink.text(Stream::Stdout)?.is_empty());
    assert!(
        chain
            .state()
            .lock()
            .map_err(|_| "chain")?
            .submissions
            .is_empty()
    );
    client.close().await;
    Ok(())
}

#[tokio::test]
async fn direct_external_success_releases_once_and_preserves_the_receipt() -> TestResult {
    let (client, chain, keys) = fixture(1_000_000_000).await?;
    let factory = DirectExternalFactory::new(&keys, false);
    let (mut ctx, sink, reads) = context(&[
        "--yes",
        "--json",
        "--no-mev-shield",
        "--signer",
        "extension",
    ])?;
    let receipt = ctx
        .submit(
            &client,
            &Preview { blocked: false },
            false,
            None,
            Some(&factory),
            &mut rtao_cli::prompt::PromptHistory::default(),
        )
        .await?
        .ok_or("direct receipt")?;
    assert!(receipt.success);
    assert_eq!(factory.acquired.load(Ordering::SeqCst), 1);
    assert_eq!(*factory.released.lock().map_err(|_| "release")?, vec![true]);
    assert_eq!(keys.unlocks.load(Ordering::SeqCst), 1);
    assert_eq!(reads.load(Ordering::SeqCst), 0);
    assert_eq!(
        chain.state().lock().map_err(|_| "chain")?.submissions.len(),
        1
    );
    assert_eq!(
        serde_json::from_str::<Value>(&sink.text(Stream::Stdout)?)?,
        receipt.to_json()
    );
    client.close().await;
    Ok(())
}

async fn direct_external_failed_chain_receipt_case(fail_release: bool) -> TestResult {
    let (client, chain, keys) = fixture(1_000_000_000).await?;
    chain.mutate_state(|state| {
        state.queue_submit(rtao_testkit::SubmitScript::InBlockThenFinalized {
            events: json!([{
                "phase":"ApplyExtrinsic", "extrinsic_idx":0,
                "module_id":"System", "event_id":"ExtrinsicFailed",
                "attributes":{
                    "dispatch_error":{"BadOrigin":null},
                    "dispatch_info":{
                        "weight":{"ref_time":1,"proof_size":0},
                        "class":"Normal","pays_fee":"Yes"
                    }
                }, "topics":[]
            }]),
        });
    });
    let factory = DirectExternalFactory::new(&keys, fail_release);
    let (mut ctx, sink, _) = context(&[
        "--yes",
        "--json",
        "--no-mev-shield",
        "--signer",
        "extension",
    ])?;
    let error = ctx
        .submit(
            &client,
            &Preview { blocked: false },
            false,
            None,
            Some(&factory),
            &mut rtao_cli::prompt::PromptHistory::default(),
        )
        .await
        .err()
        .ok_or("failed direct receipt")?;
    assert_eq!(error.code, 1);
    assert!(error.message.is_empty());
    assert_eq!(factory.acquired.load(Ordering::SeqCst), 1);
    assert_eq!(
        *factory.released.lock().map_err(|_| "release")?,
        vec![false]
    );
    assert_eq!(keys.unlocks.load(Ordering::SeqCst), 1);
    assert_eq!(
        chain.state().lock().map_err(|_| "chain")?.submissions.len(),
        1
    );
    let rendered: Value = serde_json::from_str(&sink.text(Stream::Stdout)?)?;
    assert_eq!(rendered["success"], json!(false));
    client.close().await;
    Ok(())
}

#[tokio::test]
async fn direct_external_failed_chain_receipt_releases_false_and_renders_once() -> TestResult {
    direct_external_failed_chain_receipt_case(false).await
}

#[tokio::test]
async fn direct_external_failed_receipt_remains_primary_when_release_also_fails() -> TestResult {
    direct_external_failed_chain_receipt_case(true).await
}

#[tokio::test]
async fn direct_external_close_error_propagates_after_a_known_successful_submission() -> TestResult
{
    let (client, chain, keys) = fixture(1_000_000_000).await?;
    let factory = DirectExternalFactory::new(&keys, true);
    let (mut ctx, sink, _) = context(&[
        "--yes",
        "--json",
        "--no-mev-shield",
        "--signer",
        "extension",
    ])?;
    let error = ctx
        .submit(
            &client,
            &Preview { blocked: false },
            false,
            None,
            Some(&factory),
            &mut rtao_cli::prompt::PromptHistory::default(),
        )
        .await
        .err()
        .ok_or("direct close failure")?;
    assert_eq!(error.code, 1);
    assert_eq!(error.message, "direct close failure sentinel");
    assert_eq!(*factory.released.lock().map_err(|_| "release")?, vec![true]);
    assert_eq!(keys.unlocks.load(Ordering::SeqCst), 1);
    assert_eq!(
        chain.state().lock().map_err(|_| "chain")?.submissions.len(),
        1
    );
    assert!(
        error
            .help
            .ok_or("known submission recovery")?
            .contains("already submitted")
    );
    assert!(sink.text(Stream::Stdout)?.is_empty());
    client.close().await;
    Ok(())
}

struct DirectExecutionFailure {
    preflights: AtomicUsize,
    policy: bool,
}
impl IntentInterface for DirectExecutionFailure {
    const SPEC: &'static rtao_catalog::IntentSpec = <Preview as IntentInterface>::SPEC;
    type Role = Coldkey;
    type Origin = Signed;
    type Output = ();
    const WRAPS: &'static [&'static rtao_runtime::CallDescriptor] =
        <Preview as IntentInterface>::WRAPS;
    fn to_json(&self) -> Value {
        json!({"op":"add_stake"})
    }
}
impl Intent for DirectExecutionFailure {
    fn summary(&self) -> String {
        "direct execution failure fixture".into()
    }
    async fn build<V: View + ?Sized>(&self, cx: &BuildCx<'_, V>) -> Result<Built, IntentError> {
        Preview { blocked: false }.build(cx).await
    }
    async fn preflight<V: View + ?Sized>(
        &self,
        cx: &PreflightCx<'_, V>,
    ) -> Result<IntentPreflight, IntentError> {
        if self.preflights.fetch_add(1, Ordering::SeqCst) > 0 {
            if self.policy {
                return Ok(IntentPreflight {
                    blocks: vec!["direct primary execution sentinel".into()],
                    ..Default::default()
                });
            }
            return Err(IntentError::Validation(
                "direct primary execution sentinel".into(),
            ));
        }
        Preview { blocked: false }.preflight(cx).await
    }
}

#[tokio::test]
async fn direct_external_primary_execution_error_wins_over_release_failure() -> TestResult {
    for policy in [false, true] {
        let (client, chain, keys) = fixture(1_000_000_000).await?;
        let factory = DirectExternalFactory::new(&keys, true);
        let (mut ctx, sink, _) = context(&[
            "--yes",
            "--json",
            "--no-mev-shield",
            "--signer",
            "extension",
        ])?;
        let intent = DirectExecutionFailure {
            preflights: AtomicUsize::new(0),
            policy,
        };
        let error = ctx
            .submit(
                &client,
                &intent,
                false,
                None,
                Some(&factory),
                &mut rtao_cli::prompt::PromptHistory::default(),
            )
            .await
            .err()
            .ok_or("primary execution failure")?;
        assert_eq!(error.code, 1);
        assert!(error.message.contains("direct primary execution sentinel"));
        if policy {
            assert!(matches!(
                &error.diagnostic,
                Some(rtao_cli::error::CliDiagnostic::Policy { .. })
            ));
            assert_eq!(
                error.help.as_deref(),
                Some("the action exceeds a configured safety policy")
            );
            let rendered_start = sink.bytes(Stream::Stderr)?.len();
            ctx.output.cli_error(&error)?;
            let bytes = sink.bytes(Stream::Stderr)?;
            let rendered: Value = serde_json::from_slice(&bytes[rendered_start..])?;
            assert_eq!(rendered["error"], error.message);
            assert_eq!(
                rendered["help"],
                error.help.as_deref().ok_or("policy help")?
            );
            assert_eq!(
                rendered["see"],
                format!("{}/errors/policy-violation", rtao_catalog::py::DOCS_URL)
            );
        } else {
            assert!(error.diagnostic.is_none());
        }
        assert!(!error.message.contains("direct close failure sentinel"));
        assert!(intent.preflights.load(Ordering::SeqCst) >= 2);
        assert_eq!(factory.acquired.load(Ordering::SeqCst), 1);
        assert_eq!(
            *factory.released.lock().map_err(|_| "release")?,
            vec![false]
        );
        assert_eq!(keys.unlocks.load(Ordering::SeqCst), 0);
        assert!(
            chain
                .state()
                .lock()
                .map_err(|_| "chain")?
                .submissions
                .is_empty()
        );
        assert!(sink.text(Stream::Stdout)?.is_empty());
        client.close().await;
    }
    Ok(())
}

type CallbackTrace = Arc<Mutex<Vec<&'static str>>>;

fn callback_phase(
    trace: &CallbackTrace,
    phase: &'static str,
) -> Result<(), rtao_cli::error::CliError> {
    trace
        .lock()
        .map_err(|_| rtao_cli::error::CliError::failure("callback trace lock"))?
        .push(phase);
    Ok(())
}

struct CallbackSink {
    trace: CallbackTrace,
    memory: MemorySink,
}
impl rtao_cli::output::OutputSink for CallbackSink {
    fn write(&self, stream: Stream, bytes: &[u8]) -> std::io::Result<()> {
        if stream == Stream::Stdout && !bytes.is_empty() {
            self.trace
                .lock()
                .map_err(|_| std::io::Error::other("callback trace lock"))?
                .push("render");
        }
        rtao_cli::output::OutputSink::write(&self.memory, stream, bytes)
    }
}
fn callback_sink(ctx: &mut AppContext, trace: &CallbackTrace) -> MemorySink {
    let memory = MemorySink::default();
    ctx.output = Output::new(
        OutputOptions {
            json_mode: true,
            ..Default::default()
        },
        Arc::new(CallbackSink {
            trace: trace.clone(),
            memory: memory.clone(),
        }),
    );
    memory
}

struct CallbackHooks {
    trace: CallbackTrace,
    sink: MemorySink,
    fail_before: bool,
    expected_followup: Option<&'static str>,
    observed: Mutex<Option<Value>>,
}
impl rtao_cli::context::SubmissionHooks for CallbackHooks {
    fn before_submit(&self) -> BoxFuture<'_, Result<(), rtao_cli::error::CliError>> {
        Box::pin(async move {
            callback_phase(&self.trace, "before")?;
            if self.fail_before {
                Err(rtao_cli::error::CliError::failure(
                    "before-submit failure sentinel",
                ))
            } else {
                Ok(())
            }
        })
    }
    fn on_result<'a>(
        &'a self,
        result: &'a rtao::tx::ExtrinsicResult,
    ) -> BoxFuture<'a, Result<(), rtao_cli::error::CliError>> {
        Box::pin(async move {
            // Enforce hook before render against the actual output sink, in
            // addition to checking the complete lifecycle order afterwards.
            assert!(
                self.sink
                    .text(Stream::Stdout)
                    .map_err(|error| { rtao_cli::error::CliError::failure(error.to_string()) })?
                    .is_empty()
            );
            assert!(!result.data.contains_key("multisig_followup"));
            callback_phase(&self.trace, "after")?;
            *self
                .observed
                .lock()
                .map_err(|_| rtao_cli::error::CliError::failure("callback observed lock"))? =
                Some(result.to_json());
            Ok(())
        })
    }
    fn after_followup<'a>(
        &'a self,
        result: &'a rtao::tx::ExtrinsicResult,
    ) -> BoxFuture<'a, Result<(), rtao_cli::error::CliError>> {
        Box::pin(async move {
            if let Some(status) = self.expected_followup {
                assert_eq!(result.data["multisig_followup"]["status"], json!(status));
                assert!(
                    result.data["multisig_followup"]["call_hash"]
                        .as_str()
                        .is_some()
                );
                assert!(
                    result.data["multisig_followup"]["call_data"]
                        .as_str()
                        .is_some()
                );
            }
            callback_phase(&self.trace, "followup")?;
            *self
                .observed
                .lock()
                .map_err(|_| rtao_cli::error::CliError::failure("callback observed lock"))? =
                Some(result.to_json());
            Ok(())
        })
    }
}

struct CallbackExternal<'a> {
    inner: &'a DirectExternalFactory,
    trace: CallbackTrace,
    fail_source: bool,
}
impl rtao_cli::context::ExternalSignerFactory for CallbackExternal<'_> {
    fn keys<'a>(
        &'a self,
        globals: &'a Globals,
        role: SignerRole,
    ) -> BoxFuture<'a, Result<Arc<dyn DynKeySource>, rtao_cli::error::CliError>> {
        Box::pin(async move {
            if self.fail_source {
                callback_phase(&self.trace, "source-error")?;
                return Err(rtao_cli::error::CliError::failure(
                    "callback source failure sentinel",
                ));
            }
            let keys =
                rtao_cli::context::ExternalSignerFactory::keys(self.inner, globals, role).await?;
            // Record completion, not merely entry into a lazy future.
            callback_phase(&self.trace, "acquired")?;
            Ok(keys)
        })
    }
    fn release<'a>(
        &'a self,
        globals: &'a Globals,
        keys: &'a dyn DynKeySource,
        success: bool,
    ) -> BoxFuture<'a, Result<(), rtao_cli::error::CliError>> {
        Box::pin(async move {
            rtao_cli::context::ExternalSignerFactory::release(self.inner, globals, keys, success)
                .await?;
            callback_phase(&self.trace, "released")
        })
    }
}

struct CallbackMember<'a> {
    inner: &'a MemberFactory,
    trace: CallbackTrace,
}
impl rtao_cli::call::RawSignerFactory for CallbackMember<'_> {
    fn source<'a>(
        &'a self,
        ctx: &'a AppContext,
        selection: &'a rtao_cli::call::SigningSelection,
    ) -> BoxFuture<'a, Result<Arc<dyn DynKeySource>, rtao_cli::error::CliError>> {
        Box::pin(async move {
            match rtao_cli::call::RawSignerFactory::source(self.inner, ctx, selection).await {
                Ok(keys) => {
                    callback_phase(&self.trace, "acquired")?;
                    Ok(keys)
                }
                Err(error) => {
                    callback_phase(&self.trace, "source-error")?;
                    Err(error)
                }
            }
        })
    }
    fn release<'a>(
        &'a self,
        selection: &'a rtao_cli::call::SigningSelection,
        keys: &'a dyn DynKeySource,
        success: bool,
    ) -> BoxFuture<'a, Result<(), rtao_cli::error::CliError>> {
        Box::pin(async move {
            rtao_cli::call::RawSignerFactory::release(self.inner, selection, keys, success).await?;
            callback_phase(&self.trace, "released")
        })
    }
}

fn callback_options(hooks: &CallbackHooks) -> rtao_cli::context::SubmissionOptions<'_> {
    rtao_cli::context::SubmissionOptions {
        hooks: Some(hooks),
        ..Default::default()
    }
}

#[tokio::test]
async fn direct_callback_phases_prepare_source_before_hook_and_persist_before_release() -> TestResult
{
    let (client, chain, keys) = fixture(1_000_000_000).await?;
    let base = DirectExternalFactory::new(&keys, false);
    let trace = CallbackTrace::default();
    let external = CallbackExternal {
        inner: &base,
        trace: trace.clone(),
        fail_source: false,
    };
    let (mut ctx, _, _) = context(&[
        "--yes",
        "--json",
        "--no-mev-shield",
        "--signer",
        "extension",
    ])?;
    let sink = callback_sink(&mut ctx, &trace);
    let hooks = CallbackHooks {
        trace: trace.clone(),
        sink: sink.clone(),
        fail_before: false,
        expected_followup: None,
        observed: Mutex::new(None),
    };
    let result = ctx
        .submit_with_options(
            &client,
            &Preview { blocked: false },
            callback_options(&hooks),
            Some(&external),
            &mut rtao_cli::prompt::PromptHistory::default(),
        )
        .await?
        .ok_or("direct receipt")?;
    assert_eq!(
        *trace.lock().map_err(|_| "trace")?,
        vec!["acquired", "before", "after", "released", "render"]
    );
    assert_eq!(*base.released.lock().map_err(|_| "release")?, vec![true]);
    assert_eq!(keys.unlocks.load(Ordering::SeqCst), 1);
    assert_eq!(
        chain.state().lock().map_err(|_| "chain")?.submissions.len(),
        1
    );
    assert_eq!(
        hooks.observed.lock().map_err(|_| "observed")?.as_ref(),
        Some(&result.to_json())
    );
    assert_eq!(
        serde_json::from_str::<Value>(&sink.text(Stream::Stdout)?)?,
        result.to_json()
    );
    client.close().await;
    Ok(())
}

#[tokio::test]
async fn saved_callback_phases_persist_raw_before_release_then_enrich_before_render() -> TestResult
{
    let keys = members()?;
    let (client, chain, _) = fixture(1_000_000_000).await?;
    let runtime = client.refresh_runtime().await?;
    let group = rtao::tx::Multisig::new(
        &runtime,
        &keys
            .iter()
            .map(|keys| keys.key.account())
            .collect::<Vec<_>>(),
        3,
    )?;
    let inner = rtao_runtime::calls::system::remark(vec![8, 9]).compose(&runtime)?;
    chain.mutate_state(|state| {
        state.queue_submit(rtao_testkit::SubmitScript::InBlockThenFinalized {
            events: success_events(Some((keys[0].key.account(), &group, inner.hash()))),
        })
    });
    let base = factory(&keys);
    let trace = CallbackTrace::default();
    let member = CallbackMember {
        inner: &base,
        trace: trace.clone(),
    };
    let (mut ctx, _) = saved_context(&keys, &["--yes", "--json", "--no-mev-shield"], 1)?;
    let sink = callback_sink(&mut ctx, &trace);
    let hooks = CallbackHooks {
        trace: trace.clone(),
        sink: sink.clone(),
        fail_before: false,
        expected_followup: Some("executed"),
        observed: Mutex::new(None),
    };
    let result = ctx
        .submit_saved_with_options(
            &client,
            &Preview { blocked: false },
            callback_options(&hooks),
            &member,
            &CallCache::default(),
        )
        .await?
        .ok_or("saved receipt")?;
    assert_eq!(
        *trace.lock().map_err(|_| "trace")?,
        vec![
            "before", "acquired", "after", "released", "followup", "render"
        ]
    );
    assert_eq!(*base.releases.lock().map_err(|_| "release")?, vec![true]);
    assert_eq!(keys[0].unlocks.load(Ordering::SeqCst), 1);
    assert_eq!(
        chain.state().lock().map_err(|_| "chain")?.submissions.len(),
        1
    );
    assert_eq!(
        hooks.observed.lock().map_err(|_| "observed")?.as_ref(),
        Some(&result.to_json())
    );
    assert_eq!(
        serde_json::from_str::<Value>(&sink.text(Stream::Stdout)?)?,
        result.to_json()
    );
    client.close().await;
    Ok(())
}

#[tokio::test]
async fn direct_before_submit_failure_releases_without_signing_or_receipt_hook() -> TestResult {
    let (client, chain, keys) = fixture(1_000_000_000).await?;
    let base = DirectExternalFactory::new(&keys, false);
    let trace = CallbackTrace::default();
    let external = CallbackExternal {
        inner: &base,
        trace: trace.clone(),
        fail_source: false,
    };
    let (mut ctx, _, _) = context(&[
        "--yes",
        "--json",
        "--no-mev-shield",
        "--signer",
        "extension",
    ])?;
    let sink = callback_sink(&mut ctx, &trace);
    let hooks = CallbackHooks {
        trace: trace.clone(),
        sink: sink.clone(),
        fail_before: true,
        expected_followup: None,
        observed: Mutex::new(None),
    };
    let error = ctx
        .submit_with_options(
            &client,
            &Preview { blocked: false },
            callback_options(&hooks),
            Some(&external),
            &mut rtao_cli::prompt::PromptHistory::default(),
        )
        .await
        .err()
        .ok_or("before hook failure")?;
    assert_eq!(error.message, "before-submit failure sentinel");
    assert_eq!(
        *trace.lock().map_err(|_| "trace")?,
        vec!["acquired", "before", "released"]
    );
    assert_eq!(*base.released.lock().map_err(|_| "release")?, vec![false]);
    assert_eq!(keys.unlocks.load(Ordering::SeqCst), 0);
    assert!(hooks.observed.lock().map_err(|_| "observed")?.is_none());
    assert!(
        chain
            .state()
            .lock()
            .map_err(|_| "chain")?
            .submissions
            .is_empty()
    );
    assert!(sink.text(Stream::Stdout)?.is_empty());
    client.close().await;
    Ok(())
}

#[tokio::test]
async fn saved_before_submit_failure_never_acquires_source_or_signs() -> TestResult {
    let keys = members()?;
    let (client, chain, _) = fixture(1_000_000_000).await?;
    let base = factory(&keys);
    let trace = CallbackTrace::default();
    let member = CallbackMember {
        inner: &base,
        trace: trace.clone(),
    };
    let (mut ctx, _) = saved_context(&keys, &["--yes", "--json", "--no-mev-shield"], 1)?;
    let sink = callback_sink(&mut ctx, &trace);
    let hooks = CallbackHooks {
        trace: trace.clone(),
        sink: sink.clone(),
        fail_before: true,
        expected_followup: None,
        observed: Mutex::new(None),
    };
    let error = ctx
        .submit_saved_with_options(
            &client,
            &Preview { blocked: false },
            callback_options(&hooks),
            &member,
            &CallCache::default(),
        )
        .await
        .err()
        .ok_or("before hook failure")?;
    assert_eq!(error.message, "before-submit failure sentinel");
    assert_eq!(*trace.lock().map_err(|_| "trace")?, vec!["before"]);
    assert_eq!(base.acquired.load(Ordering::SeqCst), 0);
    assert!(base.releases.lock().map_err(|_| "release")?.is_empty());
    assert_eq!(keys[0].unlocks.load(Ordering::SeqCst), 0);
    assert!(hooks.observed.lock().map_err(|_| "observed")?.is_none());
    assert!(
        chain
            .state()
            .lock()
            .map_err(|_| "chain")?
            .submissions
            .is_empty()
    );
    assert!(sink.text(Stream::Stdout)?.is_empty());
    client.close().await;
    Ok(())
}

#[tokio::test]
async fn direct_source_failure_invokes_neither_callback_and_has_nothing_to_release() -> TestResult {
    let (client, chain, keys) = fixture(1_000_000_000).await?;
    let base = DirectExternalFactory::new(&keys, false);
    let trace = CallbackTrace::default();
    let external = CallbackExternal {
        inner: &base,
        trace: trace.clone(),
        fail_source: true,
    };
    let (mut ctx, _, _) = context(&[
        "--yes",
        "--json",
        "--no-mev-shield",
        "--signer",
        "extension",
    ])?;
    let sink = callback_sink(&mut ctx, &trace);
    let hooks = CallbackHooks {
        trace: trace.clone(),
        sink: sink.clone(),
        fail_before: false,
        expected_followup: None,
        observed: Mutex::new(None),
    };
    let error = ctx
        .submit_with_options(
            &client,
            &Preview { blocked: false },
            callback_options(&hooks),
            Some(&external),
            &mut rtao_cli::prompt::PromptHistory::default(),
        )
        .await
        .err()
        .ok_or("source failure")?;
    assert_eq!(error.message, "callback source failure sentinel");
    assert_eq!(*trace.lock().map_err(|_| "trace")?, vec!["source-error"]);
    assert!(base.released.lock().map_err(|_| "release")?.is_empty());
    assert_eq!(keys.unlocks.load(Ordering::SeqCst), 0);
    assert!(hooks.observed.lock().map_err(|_| "observed")?.is_none());
    assert!(
        chain
            .state()
            .lock()
            .map_err(|_| "chain")?
            .submissions
            .is_empty()
    );
    assert!(sink.text(Stream::Stdout)?.is_empty());
    client.close().await;
    Ok(())
}

#[tokio::test]
async fn saved_source_failure_retains_plan_hook_and_has_no_receipt_or_source_to_release()
-> TestResult {
    let keys = members()?;
    let (client, chain, _) = fixture(1_000_000_000).await?;
    let mut base = factory(&keys);
    base.fail_source_at = Some(1);
    let trace = CallbackTrace::default();
    let member = CallbackMember {
        inner: &base,
        trace: trace.clone(),
    };
    let (mut ctx, _) = saved_context(&keys, &["--yes", "--json", "--no-mev-shield"], 1)?;
    let sink = callback_sink(&mut ctx, &trace);
    let hooks = CallbackHooks {
        trace: trace.clone(),
        sink: sink.clone(),
        fail_before: false,
        expected_followup: None,
        observed: Mutex::new(None),
    };
    let error = ctx
        .submit_saved_with_options(
            &client,
            &Preview { blocked: false },
            callback_options(&hooks),
            &member,
            &CallCache::default(),
        )
        .await
        .err()
        .ok_or("source failure")?;
    assert_eq!(error.message, "fixture source unavailable");
    assert_eq!(
        *trace.lock().map_err(|_| "trace")?,
        vec!["before", "source-error"]
    );
    assert_eq!(base.acquired.load(Ordering::SeqCst), 1);
    assert!(base.releases.lock().map_err(|_| "release")?.is_empty());
    assert_eq!(keys[0].unlocks.load(Ordering::SeqCst), 0);
    assert!(hooks.observed.lock().map_err(|_| "observed")?.is_none());
    assert!(
        chain
            .state()
            .lock()
            .map_err(|_| "chain")?
            .submissions
            .is_empty()
    );
    assert!(sink.text(Stream::Stdout)?.is_empty());
    client.close().await;
    Ok(())
}

struct DurablePlanHooks<'a> {
    unlocks: &'a AtomicUsize,
    source_acquisitions: Option<&'a AtomicUsize>,
    releases: &'a Mutex<Vec<bool>>,
    plans: Mutex<Vec<rtao::tx::Plan>>,
    raw: Mutex<Vec<rtao::tx::ExtrinsicResult>>,
    enriched: Mutex<Vec<rtao::tx::ExtrinsicResult>>,
    fail_result: bool,
}
impl rtao_cli::context::SubmissionHooks for DurablePlanHooks<'_> {
    fn before_submit(&self) -> BoxFuture<'_, Result<(), rtao_cli::error::CliError>> {
        Box::pin(async {
            Err(rtao_cli::error::CliError::failure(
                "durable submission must receive its reviewed plan",
            ))
        })
    }
    fn before_submit_with_plan<'a>(
        &'a self,
        plan: &'a rtao::tx::Plan,
    ) -> BoxFuture<'a, Result<(), rtao_cli::error::CliError>> {
        Box::pin(async move {
            assert_eq!(self.unlocks.load(Ordering::SeqCst), 0);
            if let Some(acquisitions) = self.source_acquisitions {
                assert_eq!(acquisitions.load(Ordering::SeqCst), 0);
            }
            assert!(self.releases.lock().map_err(callback_lock)?.is_empty());
            self.plans.lock().map_err(callback_lock)?.push(plan.clone());
            Ok(())
        })
    }
    fn on_result<'a>(
        &'a self,
        receipt: &'a rtao::tx::ExtrinsicResult,
    ) -> BoxFuture<'a, Result<(), rtao_cli::error::CliError>> {
        Box::pin(async move {
            assert_eq!(self.plans.lock().map_err(callback_lock)?.len(), 1);
            assert!(self.releases.lock().map_err(callback_lock)?.is_empty());
            assert!(!receipt.data.contains_key("multisig_followup"));
            self.raw
                .lock()
                .map_err(callback_lock)?
                .push(receipt.clone());
            if self.fail_result {
                return Err(rtao_cli::error::CliError::failure(
                    "journal receipt persistence failure",
                ));
            }
            Ok(())
        })
    }
    fn after_followup<'a>(
        &'a self,
        receipt: &'a rtao::tx::ExtrinsicResult,
    ) -> BoxFuture<'a, Result<(), rtao_cli::error::CliError>> {
        Box::pin(async move {
            assert_eq!(self.raw.lock().map_err(callback_lock)?.len(), 1);
            self.enriched
                .lock()
                .map_err(callback_lock)?
                .push(receipt.clone());
            Ok(())
        })
    }
}

fn callback_lock<T>(_: std::sync::PoisonError<T>) -> rtao_cli::error::CliError {
    rtao_cli::error::CliError::failure("durable callback fixture lock")
}

#[tokio::test]
async fn durable_direct_plan_keeps_proxy_call_and_records_receipt_before_close_failure()
-> TestResult {
    for fail_result in [false, true] {
        let (client, chain, keys) = fixture(1_000_000_000).await?;
        let base = DirectExternalFactory::new(&keys, true);
        let real = AccountId([91; 32]);
        let real_flag = real.to_string();
        let (mut ctx, sink, _) = context(&[
            "--yes",
            "--json",
            "--no-mev-shield",
            "--signer",
            "extension",
            "--proxy-for",
            &real_flag,
        ])?;
        let hooks = DurablePlanHooks {
            unlocks: keys.unlocks.as_ref(),
            source_acquisitions: None,
            releases: &base.released,
            plans: Mutex::new(Vec::new()),
            raw: Mutex::new(Vec::new()),
            enriched: Mutex::new(Vec::new()),
            fail_result,
        };
        let error = ctx
            .submit_with_options(
                &client,
                &Preview { blocked: false },
                rtao_cli::context::SubmissionOptions {
                    hooks: Some(&hooks),
                    ..Default::default()
                },
                Some(&base),
                &mut rtao_cli::prompt::PromptHistory::default(),
            )
            .await
            .err()
            .ok_or("known receipt followed by local failure")?;
        assert_eq!(
            error.message,
            if fail_result {
                "journal receipt persistence failure"
            } else {
                "direct close failure sentinel"
            }
        );
        assert!(
            error
                .help
                .ok_or("known receipt recovery")?
                .contains("already submitted")
        );
        assert_eq!(keys.unlocks.load(Ordering::SeqCst), 1);
        assert_eq!(*base.released.lock().map_err(|_| "release")?, vec![true]);
        let plans = hooks.plans.lock().map_err(|_| "plans")?.clone();
        assert_eq!(plans.len(), 1);
        assert_eq!(plans[0].signer_address, keys.key.account());
        assert_eq!(plans[0].extras["proxy_for"], json!(real));
        let submissions = chain
            .state()
            .lock()
            .map_err(|_| "state")?
            .submissions
            .clone();
        assert_eq!(submissions.len(), 1);
        let runtime = client.refresh_runtime().await?;
        let submitted = runtime.core().decode_extrinsic(&submissions[0], true)?;
        let reviewed = runtime.core().decode_spec(
            &rtao::core::runtime::TypeSpec::Call,
            plans[0].call().as_bytes(),
            true,
        )?;
        assert_eq!(submitted.field("call"), Some(&reviewed));
        let raw = hooks.raw.lock().map_err(|_| "raw")?.clone();
        assert_eq!(raw.len(), 1);
        assert!(raw[0].success);
        assert_eq!(raw[0].data["proxy_for"], json!(real));
        assert!(hooks.enriched.lock().map_err(|_| "enriched")?.is_empty());
        assert!(sink.text(Stream::Stdout)?.is_empty());
        client.close().await;
    }
    Ok(())
}

#[tokio::test]
async fn durable_saved_plan_precedes_source_and_raw_receipt_survives_close_or_journal_error()
-> TestResult {
    for fail_result in [false, true] {
        let keys = members()?;
        let (client, chain, _) = fixture(1_000_000_000).await?;
        let mut base = factory(&keys);
        base.fail_release = true;
        let (ctx, sink) = saved_context(&keys, &["--yes", "--json", "--no-mev-shield"], 1)?;
        let hooks = DurablePlanHooks {
            unlocks: keys[0].unlocks.as_ref(),
            source_acquisitions: Some(&base.acquired),
            releases: &base.releases,
            plans: Mutex::new(Vec::new()),
            raw: Mutex::new(Vec::new()),
            enriched: Mutex::new(Vec::new()),
            fail_result,
        };
        let error = ctx
            .submit_saved_with_options(
                &client,
                &Preview { blocked: false },
                rtao_cli::context::SubmissionOptions {
                    hooks: Some(&hooks),
                    ..Default::default()
                },
                &base,
                &CallCache::default(),
            )
            .await
            .err()
            .ok_or("known saved receipt followed by local failure")?;
        assert_eq!(
            error.message,
            if fail_result {
                "journal receipt persistence failure"
            } else {
                "fixture close failed"
            }
        );
        assert_eq!(keys[0].unlocks.load(Ordering::SeqCst), 1);
        assert_eq!(base.acquired.load(Ordering::SeqCst), 1);
        assert_eq!(*base.releases.lock().map_err(|_| "release")?, vec![true]);
        let plans = hooks.plans.lock().map_err(|_| "plans")?.clone();
        assert_eq!(plans.len(), 1);
        assert_eq!(plans[0].op, "multisig_execute");
        assert_eq!(plans[0].signer_address, keys[0].key.account());
        let hash = plans[0].extras["multisig_call_hash"]
            .as_str()
            .ok_or("reviewed inner hash")?;
        assert!(error.help.ok_or("saved recovery help")?.contains(hash));
        let raw = hooks.raw.lock().map_err(|_| "raw")?.clone();
        assert_eq!(raw.len(), 1);
        assert!(raw[0].success);
        assert!(!raw[0].data.contains_key("multisig_followup"));
        let enriched = hooks.enriched.lock().map_err(|_| "enriched")?.clone();
        assert_eq!(enriched.len(), usize::from(!fail_result));
        if !fail_result {
            assert!(enriched[0].data.contains_key("multisig_followup"));
        }
        assert_eq!(
            chain.state().lock().map_err(|_| "state")?.submissions.len(),
            1
        );
        assert!(sink.text(Stream::Stdout)?.is_empty());
        client.close().await;
    }
    Ok(())
}

#[tokio::test]
async fn durable_direct_call_binding_refuses_rebuild_drift_before_private_signing() -> TestResult {
    let (client, chain, keys) = fixture(1_000_000_000).await?;
    let (ctx, _, _) = context(&["--yes", "--json", "--no-mev-shield"])?;
    let intent = ChangingPreview(AtomicUsize::new(0));
    let error = ctx
        .submit_with_keys_options(
            &client,
            &intent,
            &keys,
            rtao_cli::context::SubmissionOptions {
                bind_reviewed_call: true,
                ..Default::default()
            },
        )
        .await
        .err()
        .ok_or("reviewed bytes changed")?;
    assert!(error.message.contains("differs from the reviewed call"));
    assert_eq!(intent.0.load(Ordering::SeqCst), 2);
    assert_eq!(keys.unlocks.load(Ordering::SeqCst), 0);
    assert!(
        chain
            .state()
            .lock()
            .map_err(|_| "state")?
            .submissions
            .is_empty()
    );
    client.close().await;
    Ok(())
}

#[tokio::test]
async fn durable_saved_binding_keeps_reviewed_plan_and_refuses_drift_before_private_signing()
-> TestResult {
    let keys = members()?;
    let (client, chain, _) = fixture(1_000_000_000).await?;
    let base = factory(&keys);
    let (ctx, _) = saved_context(&keys, &["--yes", "--json", "--no-mev-shield"], 1)?;
    let intent = ChangingPreview(AtomicUsize::new(0));
    let hooks = DurablePlanHooks {
        unlocks: keys[0].unlocks.as_ref(),
        source_acquisitions: Some(&base.acquired),
        releases: &base.releases,
        plans: Mutex::new(Vec::new()),
        raw: Mutex::new(Vec::new()),
        enriched: Mutex::new(Vec::new()),
        fail_result: false,
    };
    let error = ctx
        .submit_saved_with_options(
            &client,
            &intent,
            rtao_cli::context::SubmissionOptions {
                bind_reviewed_call: true,
                hooks: Some(&hooks),
                ..Default::default()
            },
            &base,
            &CallCache::default(),
        )
        .await
        .err()
        .ok_or("reviewed approval bytes changed")?;
    assert!(error.message.contains("differs from the reviewed call"));
    assert_eq!(intent.0.load(Ordering::SeqCst), 2);
    assert_eq!(hooks.plans.lock().map_err(|_| "plans")?.len(), 1);
    assert!(hooks.raw.lock().map_err(|_| "raw")?.is_empty());
    assert!(hooks.enriched.lock().map_err(|_| "enriched")?.is_empty());
    assert_eq!(base.acquired.load(Ordering::SeqCst), 1);
    assert_eq!(*base.releases.lock().map_err(|_| "release")?, vec![false]);
    assert_eq!(keys[0].unlocks.load(Ordering::SeqCst), 0);
    assert!(
        chain
            .state()
            .lock()
            .map_err(|_| "state")?
            .submissions
            .is_empty()
    );
    client.close().await;
    Ok(())
}

// This transport probe deliberately builds a remark rather than a root trade.
// Actual root preparation is covered by root_commands; here independent effect
// strings discriminate exact-summary replacement from substring/blanket rewriting.
struct CommandFactsFixture {
    fee_facts: bool,
}
impl IntentInterface for CommandFactsFixture {
    const SPEC: &'static rtao_catalog::IntentSpec = Preview::SPEC;
    type Role = Coldkey;
    type Origin = Signed;
    type Output = ();
    const WRAPS: &'static [&'static rtao_runtime::CallDescriptor] = Preview::WRAPS;
    fn to_json(&self) -> Value {
        json!({"op":"add_stake"})
    }
}
impl Intent for CommandFactsFixture {
    fn summary(&self) -> String {
        "generic semantic action".into()
    }
    async fn build<V: View + ?Sized>(&self, cx: &BuildCx<'_, V>) -> Result<Built, IntentError> {
        Ok(Built::new(
            rtao_runtime::calls::system::remark(vec![31, 41]).compose(cx.runtime)?,
        ))
    }
    async fn preflight<V: View + ?Sized>(
        &self,
        _: &PreflightCx<'_, V>,
    ) -> Result<IntentPreflight, IntentError> {
        Ok(IntentPreflight {
            effects: vec![
                "generic semantic action".into(),
                "keep unrelated semantic fact".into(),
                "prefix generic semantic action suffix".into(),
            ],
            warnings: vec!["keep independent warning".into()],
            facts_title: if self.fee_facts {
                rtao::tx::FactsTitle::Fees
            } else {
                rtao::tx::FactsTitle::Quote
            },
            facts: if self.fee_facts {
                vec![("inclusion".to_owned(), "τ0.000020000".to_owned())]
            } else {
                vec![("expected receive".to_owned(), "~1.0000 β".to_owned())]
            },
            ..Default::default()
        })
    }
}
fn command_review(intent: &dyn rtao::tx::DynIntent) -> CommandReview {
    CommandReview::new(
        intent,
        "allocate τ2 into Chosen Fund at 2.0000 τ/β for ~1.0000 β",
        vec![ReviewSection {
            title: "Allocate".into(),
            rows: vec![
                ("validator name".into(), json!("Chosen Fund")),
                ("receive".into(), json!("~1.0000 β")),
                (
                    "note".into(),
                    json!("you bear entry slippage and swap fees"),
                ),
            ],
        }],
    )
}

#[tokio::test]
async fn command_review_direct_json_replaces_only_matching_effect_and_keeps_plan_identity()
-> TestResult {
    let (client, chain, keys) = fixture(1_000_000_000).await?;
    let intent = CommandFactsFixture { fee_facts: false };
    let review = command_review(&intent);
    let (ctx, sink, reads) = context(&["--dry-run", "--json", "--no-mev-shield"])?;
    let replay = "rtao root allocate --hotkey exact --amount 2 --dry-run";
    assert!(
        ctx.submit_with_keys_options(
            &client,
            &intent,
            &keys,
            SubmissionOptions {
                review: Some(&review),
                replay: Some(replay),
                ..Default::default()
            }
        )
        .await?
        .is_none()
    );
    let plan: Value = serde_json::from_str(&sink.text(Stream::Stdout)?)?;
    assert_eq!(plan["summary"], review.summary);
    assert_eq!(
        plan["effects"],
        json!([
            review.summary,
            "keep unrelated semantic fact",
            "prefix generic semantic action suffix"
        ])
    );
    assert_eq!(plan["warnings"], json!(["keep independent warning"]));
    assert_eq!(plan["op"], "add_stake");
    assert_eq!(plan["args"], json!({}));
    assert_eq!(plan["signer_address"], json!(keys.key.account()));
    assert_eq!(plan["command"], replay);
    assert!(!plan["fee_tao"].is_null());
    assert!(sink.text(Stream::Stderr)?.is_empty());
    assert_eq!(reads.load(Ordering::SeqCst), 0);
    assert_eq!(keys.unlocks.load(Ordering::SeqCst), 0);
    assert!(chain.mutate_state(|state| state.submissions.is_empty()));
    assert!(
        chain
            .requests()
            .iter()
            .all(|request| !request.method.starts_with("author_"))
    );
    client.close().await;
    Ok(())
}

#[tokio::test]
async fn command_review_human_final_card_and_question_precede_refusal_and_unlock() -> TestResult {
    let (client, chain, keys) = fixture(1_000_000_000).await?;
    let intent = CommandFactsFixture { fee_facts: false };
    let review = command_review(&intent);
    let (mut ctx, sink, _) = context(&["--no-mev-shield"])?;
    let questions = Arc::new(Mutex::new(Vec::new()));
    ctx.terminal = Arc::new(Mutex::new(Box::new(RegistrationConfirmation {
        questions: questions.clone(),
    })));
    let error = ctx
        .submit_with_keys_options(
            &client,
            &intent,
            &keys,
            SubmissionOptions {
                review: Some(&review),
                ..Default::default()
            },
        )
        .await
        .err()
        .ok_or("declined command review")?;
    assert_eq!(error.message, "aborted.");
    assert_eq!(
        questions.lock().map_err(|_| "questions")?.as_slice(),
        [format!("{}? [y/N]: ", review.summary)]
    );
    let card = sink.text(Stream::Stdout)?;
    assert!(card.starts_with("Allocate\n"));
    assert!(card.contains("Chosen Fund"));
    assert!(card.contains("~1.0000 β"));
    assert!(card.contains("you bear entry slippage and swap fees"));
    assert_eq!(
        review_stage_titles(&card),
        [
            "Allocate",
            "Effects",
            "Quote",
            "Fees",
            "Signer",
            "Transaction"
        ]
    );
    assert_eq!(
        review_stage_rows(&card, "Quote")?,
        [("expected receive".to_owned(), "~1.0000 β".to_owned())]
    );
    assert_eq!(
        review_stage_rows(&card, "Transaction")?,
        [("operation".to_owned(), "add_stake".to_owned())]
    );
    assert!(
        review_stage_rows(&card, "Fees")?
            .iter()
            .any(|(key, value)| key == "estimated fee" && value.starts_with("~τ"))
    );
    assert!(card.contains(&keys.key.account().to_string()));
    assert!(card.contains("keep unrelated semantic fact"));
    assert_eq!(keys.unlocks.load(Ordering::SeqCst), 0);
    assert!(chain.mutate_state(|state| state.submissions.is_empty()));
    assert!(
        chain
            .requests()
            .iter()
            .all(|request| !request.method.starts_with("author_"))
    );
    let address = keys.key.account().to_string();
    let label = "named\x1b[31m owner";
    let (mut named, sink, _) = context(&["--no-mev-shield"])?;
    named
        .books
        .addresses
        .push(json!({"address":address,"name":label}));
    named.output.update_subnet_names([(4, "four".into())])?;
    let mut named_review = command_review(&intent);
    named_review.summary = format!("Allocate from {address} on subnet 4");
    let named_questions = Arc::new(Mutex::new(Vec::new()));
    named.terminal = Arc::new(Mutex::new(Box::new(RegistrationConfirmation {
        questions: named_questions.clone(),
    })));
    let error = named
        .submit_with_keys_options(
            &client,
            &intent,
            &keys,
            SubmissionOptions {
                review: Some(&named_review),
                ..Default::default()
            },
        )
        .await
        .err()
        .ok_or("named review was not declined")?;
    assert_eq!(error.message, "aborted.");
    let rendered_name =
        rtao_cli::output::terminal_text(&format!("{label} ({address})")).into_owned();
    assert_eq!(
        named_questions
            .lock()
            .map_err(|_| "named questions")?
            .as_slice(),
        [format!(
            "Allocate from {rendered_name} on subnet 4 (four)? [y/N]: "
        )]
    );
    let card = sink.text(Stream::Stdout)?;
    assert!(card.contains(&rendered_name));
    assert!(!card.contains('\x1b'));
    assert_eq!(keys.unlocks.load(Ordering::SeqCst), 0);
    assert!(chain.mutate_state(|state| state.submissions.is_empty()));

    // The context formats transaction arguments before the output renders
    // them. A later label containing an earlier address detects two prose
    // passes even though each ordinary name is guarded against decoration.
    let earlier = AccountId([61; 32]).to_string();
    let destination = AccountId([62; 32]).to_string();
    let transfer = rtao_intents::AnyIntent::from_tool(
        "transfer",
        json!({"dest_ss58":destination,"amount_tao":"0.1"}),
    )?;
    let original_arguments = transfer.as_dyn().to_json_dyn();
    let (mut composed, sink, _) = context(&["--no-mev-shield"])?;
    composed.output.name_address(&earlier, "Earlier")?;
    composed.output.name_address(&destination, &earlier)?;
    let questions = Arc::new(Mutex::new(Vec::new()));
    composed.terminal = Arc::new(Mutex::new(Box::new(RegistrationConfirmation {
        questions: questions.clone(),
    })));
    let error = composed
        .submit_with_keys_options(
            &client,
            transfer.as_dyn(),
            &keys,
            SubmissionOptions::default(),
        )
        .await
        .err()
        .ok_or("composed transfer review was not declined")?;
    assert_eq!(error.message, "aborted.");
    let decorated = format!("{earlier} ({destination})");
    let card = sink.text(Stream::Stdout)?;
    let transaction = review_stage_rows(&card, "Transaction")?;
    assert_eq!(
        transaction
            .iter()
            .find(|(key, _)| key == "dest")
            .map(|(_, value)| value.as_str()),
        Some(decorated.as_str())
    );
    assert!(!card.contains(&format!("Earlier ({earlier}) ({destination})")));
    assert_eq!(questions.lock().map_err(|_| "composed questions")?.len(), 1);
    assert!(
        questions
            .lock()
            .map_err(|_| "composed questions")?
            .as_slice()
            .iter()
            .any(|question| question.contains(&decorated))
    );
    assert_eq!(transfer.as_dyn().to_json_dyn(), original_arguments);
    assert_eq!(keys.unlocks.load(Ordering::SeqCst), 0);
    assert!(chain.mutate_state(|state| state.submissions.is_empty()));
    assert!(
        chain
            .requests()
            .iter()
            .all(|request| !request.method.starts_with("author_"))
    );
    client.close().await;
    Ok(())
}

struct LabelOnlyKeys {
    public: Arc<dyn DynKeySource>,
}
impl KeyView for LabelOnlyKeys {
    fn coldkey_address(&self) -> Result<AccountId, SignerError> {
        self.public.coldkey_address()
    }
    fn hotkey_address(&self) -> Result<AccountId, SignerError> {
        self.public.hotkey_address()
    }
    fn hotkey_public_key(&self) -> Result<[u8; 32], SignerError> {
        self.public.hotkey_public_key()
    }
}
impl DynKeySource for LabelOnlyKeys {
    fn signer_for<'a>(
        &'a self,
        role: SignerRole,
    ) -> BoxFuture<'a, Result<Arc<dyn Signer>, SignerError>> {
        Box::pin(async move { Err(SignerError::KeyUnavailable(role)) })
    }
}
struct ReviewExternalFactory<'a> {
    inner: &'a DirectExternalFactory,
    labels: Mutex<Vec<String>>,
    label_only: bool,
}
impl rtao_cli::context::ExternalSignerFactory for ReviewExternalFactory<'_> {
    fn set_summary(&self, _: &dyn DynKeySource, summary: &str) {
        self.labels
            .lock()
            .expect("fixture labels")
            .push(summary.into());
    }
    fn keys<'a>(
        &'a self,
        globals: &'a Globals,
        role: SignerRole,
    ) -> BoxFuture<'a, Result<Arc<dyn DynKeySource>, rtao_cli::error::CliError>> {
        Box::pin(async move {
            let keys =
                rtao_cli::context::ExternalSignerFactory::keys(self.inner, globals, role).await?;
            Ok(if self.label_only {
                Arc::new(LabelOnlyKeys { public: keys }) as Arc<dyn DynKeySource>
            } else {
                keys
            })
        })
    }
    fn release<'a>(
        &'a self,
        globals: &'a Globals,
        keys: &'a dyn DynKeySource,
        success: bool,
    ) -> BoxFuture<'a, Result<(), rtao_cli::error::CliError>> {
        rtao_cli::context::ExternalSignerFactory::release(self.inner, globals, keys, success)
    }
}
#[tokio::test]
async fn command_review_external_label_retains_custom_action_proxy_and_shield_suffixes()
-> TestResult {
    // The injected backend is the same summary boundary used by Vault. No
    // browser/device or Ledger display behavior is exercised by this fixture.
    for shield in [false, true] {
        let (client, chain, keys) = fixture(1_000_000_000).await?;
        chain.mutate_state(|state| {
            state.seed_python("MevShield", "NextKey", &[], json!(vec![171u8; 1184]))
        })?;
        let inner = DirectExternalFactory::new(&keys, false);
        let factory = ReviewExternalFactory {
            inner: &inner,
            labels: Mutex::new(Vec::new()),
            label_only: true,
        };
        let flags = if shield {
            vec!["--yes", "--json", "--mev-shield", "--signer", "extension"]
        } else {
            vec![
                "--yes",
                "--json",
                "--no-mev-shield",
                "--signer",
                "extension",
            ]
        };
        let (mut ctx, _, _) = context(&flags)?;
        let proxy = AccountId([77; 32]);
        ctx.globals.proxy_for = Some(proxy.to_string());
        let intent = CommandFactsFixture { fee_facts: false };
        let review = command_review(&intent);
        let outcome = ctx
            .submit_with_options(
                &client,
                &intent,
                SubmissionOptions {
                    review: Some(&review),
                    ..Default::default()
                },
                Some(&factory),
                &mut rtao_cli::prompt::PromptHistory::default(),
            )
            .await;
        // This public-only backend refuses every private signer request. The
        // label must already describe the reviewed action/envelopes before it.
        let expected = format!(
            "{} [as {proxy} via proxy]{}",
            review.summary,
            if shield { " [MEV-shielded]" } else { "" }
        );
        assert_eq!(
            factory.labels.lock().map_err(|_| "labels")?.as_slice(),
            [expected]
        );
        assert_eq!(inner.acquired.load(Ordering::SeqCst), 1);
        assert_eq!(keys.unlocks.load(Ordering::SeqCst), 0);
        assert_eq!(inner.released.lock().map_err(|_| "released")?.len(), 1);
        assert!(outcome.is_err());
        assert!(chain.mutate_state(|state| state.submissions.is_empty()));
        assert!(
            chain
                .requests()
                .iter()
                .all(|request| !request.method.starts_with("author_"))
        );
        client.close().await;
    }
    Ok(())
}

struct ReviewMemberFactory<'a> {
    inner: &'a MemberFactory,
    labels: Mutex<Vec<String>>,
}
impl rtao_cli::call::RawSignerFactory for ReviewMemberFactory<'_> {
    fn set_summary(
        &self,
        _: &rtao_cli::call::SigningSelection,
        _: &dyn DynKeySource,
        summary: &str,
    ) {
        self.labels
            .lock()
            .expect("fixture labels")
            .push(summary.into());
    }
    fn source<'a>(
        &'a self,
        ctx: &'a AppContext,
        selection: &'a rtao_cli::call::SigningSelection,
    ) -> BoxFuture<'a, Result<Arc<dyn DynKeySource>, rtao_cli::error::CliError>> {
        rtao_cli::call::RawSignerFactory::source(self.inner, ctx, selection)
    }
    fn release<'a>(
        &'a self,
        selection: &'a rtao_cli::call::SigningSelection,
        keys: &'a dyn DynKeySource,
        success: bool,
    ) -> BoxFuture<'a, Result<(), rtao_cli::error::CliError>> {
        rtao_cli::call::RawSignerFactory::release(self.inner, selection, keys, success)
    }
}
struct ReviewAcceptTerminal {
    questions: Arc<Mutex<Vec<String>>>,
}
impl Terminal for ReviewAcceptTerminal {
    fn stdin_is_tty(&self) -> bool {
        true
    }
    fn stderr_is_tty(&self) -> bool {
        true
    }
    fn read_line(&mut self, question: &str) -> Result<String, PromptError> {
        self.questions
            .lock()
            .map_err(|_| PromptError::Io)?
            .push(question.into());
        Ok("yes".into())
    }
    fn read_secret(&mut self, _: &str) -> Result<rtao::wallet::SecretString, PromptError> {
        Err(PromptError::Unavailable)
    }
    fn message(&mut self, _: &str) -> Result<(), PromptError> {
        Ok(())
    }
}
#[tokio::test]
async fn command_review_repeats_semantic_stages_but_preserves_saved_outer_round_labels()
-> TestResult {
    let keys = members()?;
    let (client, chain, _) = fixture(1_000_000_000).await?;
    let group = saved_group(&client, &keys).await?;
    let call = rtao_runtime::calls::system::remark(vec![8, 9])
        .compose(&client.refresh_runtime().await?)?;
    let mut inner = factory(&keys);
    publish_pending_after_release(
        &mut inner,
        chain.clone(),
        &group,
        keys[0].key.account(),
        &call,
    );
    chain.mutate_state(|state| {
        state.queue_submit(rtao_testkit::SubmitScript::InBlockThenFinalized {
            events: success_events(Some((keys[1].key.account(), &group, call.hash()))),
        })
    });
    let factory = ReviewMemberFactory {
        inner: &inner,
        labels: Mutex::new(Vec::new()),
    };
    let (mut ctx, sink) = saved_context(&keys, &["--no-mev-shield"], 2)?;
    let questions = Arc::new(Mutex::new(Vec::new()));
    ctx.terminal = Arc::new(Mutex::new(Box::new(ReviewAcceptTerminal {
        questions: questions.clone(),
    })));
    let intent = Preview { blocked: false };
    let review = command_review(&intent);
    // A preview retains the signed outer summary and acquires no source.
    {
        let (preview_ctx, preview_sink) =
            saved_context(&keys, &["--dry-run", "--json", "--no-mev-shield"], 2)?;
        assert!(
            preview_ctx
                .submit_saved_with_options(
                    &client,
                    &intent,
                    SubmissionOptions {
                        review: Some(&review),
                        ..Default::default()
                    },
                    &factory,
                    &CallCache::default()
                )
                .await?
                .is_none()
        );
        let plan: Value = serde_json::from_str(&preview_sink.text(Stream::Stdout)?)?;
        assert!(
            plan["summary"]
                .as_str()
                .ok_or("outer summary")?
                .starts_with("multisig 3-of-3 execute:")
        );
        assert_ne!(plan["summary"], json!(review.summary));
        assert_eq!(plan["op"], "multisig_execute");
        assert_eq!(inner.acquired.load(Ordering::SeqCst), 0);
    }
    let receipt = ctx
        .submit_saved_with_options(
            &client,
            &intent,
            SubmissionOptions {
                review: Some(&review),
                ..Default::default()
            },
            &factory,
            &CallCache::default(),
        )
        .await?
        .ok_or("saved receipt")?;
    assert!(receipt.success);
    assert_eq!(receipt.data["multisig_followup"]["status"], "executed");
    let card = sink.text(Stream::Stdout)?;
    assert_eq!(card.lines().filter(|line| *line == "Allocate").count(), 2);
    assert_eq!(
        card.lines()
            .filter(|line| line.contains("Chosen Fund"))
            .count(),
        2
    );
    assert_eq!(
        card.lines()
            .filter(|line| line.contains("~1.0000 β"))
            .count(),
        2
    );
    assert!(card.contains("multisig 3-of-3 execute"));
    let transactions = card.split("Transaction\n").skip(1).collect::<Vec<_>>();
    assert_eq!(transactions.len(), 2);
    for transaction in transactions {
        let rows = transaction
            .lines()
            .take_while(|line| line.starts_with("  "))
            .collect::<Vec<_>>();
        assert_eq!(
            rows.len(),
            1,
            "saved approval must show semantic fields only"
        );
        assert!(rows[0].contains("operation") && rows[0].ends_with("add_stake"));
    }
    let signers = card.split("Signer\n").skip(1).collect::<Vec<_>>();
    assert_eq!(signers.len(), 2);
    for signer in signers {
        let text = signer
            .lines()
            .take_while(|line| line.starts_with("  "))
            .collect::<Vec<_>>()
            .join("\n");
        assert!(text.contains(&format!("team ({})", group.address)));
        assert!(text.contains("3 of 3"));
        assert!(text.contains("approval member") && text.contains("via extension"));
        assert!(text.contains("approval") && text.contains("multisig 3-of-3 execute"));
    }
    let titles = review_stage_titles(&card);
    assert_eq!(titles.iter().filter(|title| **title == "Fees").count(), 2);
    assert_eq!(titles.iter().filter(|title| **title == "Signer").count(), 2);
    assert_eq!(
        questions.lock().map_err(|_| "questions")?.as_slice(),
        [format!("{}? [y/N]: ", review.summary)]
    );
    let labels = factory.labels.lock().map_err(|_| "labels")?.clone();
    assert_eq!(labels.len(), 2);
    for label in &labels {
        assert!(label.starts_with("multisig 3-of-3 execute: review context fixture"));
        assert!(!label.contains("Chosen Fund"));
    }
    assert_eq!(inner.acquired.load(Ordering::SeqCst), 2);
    assert_eq!(keys[0].unlocks.load(Ordering::SeqCst), 1);
    assert_eq!(keys[1].unlocks.load(Ordering::SeqCst), 1);
    assert_eq!(chain.mutate_state(|state| state.submissions.len()), 2);
    client.close().await;
    Ok(())
}

#[tokio::test]
async fn mismatched_command_review_refuses_every_entry_before_rpc_or_source_acquisition()
-> TestResult {
    let (client, chain, keys) = fixture(1_000_000_000).await?;
    let bound = rtao_intents::AnyIntent::from_tool(
        "stake_into_basket",
        json!({"hotkey_ss58":keys.key.account(),"amount_tao":"1"}),
    )?;
    let changed = rtao_intents::AnyIntent::from_tool(
        "stake_into_basket",
        json!({"hotkey_ss58":keys.key.account(),"amount_tao":"2"}),
    )?;
    let review = command_review(bound.as_dyn());
    let options = SubmissionOptions {
        review: Some(&review),
        ..Default::default()
    };
    let (ctx, _, reads) = context(&["--no-mev-shield"])?;
    let start = chain.requests().len();
    let error = ctx
        .submit_with_keys_options(&client, changed.as_dyn(), &keys, options)
        .await
        .err()
        .ok_or("injected keys mismatch")?;
    assert_eq!(error.code, 2);
    assert!(error.message.contains("review does not match"));
    assert_eq!(chain.requests().len(), start);
    assert_eq!(reads.load(Ordering::SeqCst), 0);
    let inner = DirectExternalFactory::new(&keys, false);
    let external = ReviewExternalFactory {
        inner: &inner,
        labels: Mutex::new(Vec::new()),
        label_only: true,
    };
    let (mut ctx, _, reads) = context(&["--signer", "extension", "--no-mev-shield"])?;
    let error = ctx
        .submit_with_options(
            &client,
            changed.as_dyn(),
            options,
            Some(&external),
            &mut rtao_cli::prompt::PromptHistory::default(),
        )
        .await
        .err()
        .ok_or("external mismatch")?;
    assert_eq!(error.code, 2);
    assert_eq!(chain.requests().len(), start);
    assert_eq!(reads.load(Ordering::SeqCst), 0);
    assert_eq!(inner.acquired.load(Ordering::SeqCst), 0);
    assert!(inner.released.lock().map_err(|_| "released")?.is_empty());
    let members = members()?;
    let factory = factory(&members);
    let (ctx, _) = saved_context(&members, &["--no-mev-shield"], 2)?;
    let error = ctx
        .submit_saved_with_options(
            &client,
            changed.as_dyn(),
            options,
            &factory,
            &CallCache::default(),
        )
        .await
        .err()
        .ok_or("saved mismatch")?;
    assert_eq!(error.code, 2);
    assert_eq!(chain.requests().len(), start);
    assert_eq!(factory.acquired.load(Ordering::SeqCst), 0);
    assert!(
        members
            .iter()
            .all(|member| member.unlocks.load(Ordering::SeqCst) == 0)
    );
    assert_eq!(keys.unlocks.load(Ordering::SeqCst), 0);
    assert!(chain.mutate_state(|state| state.submissions.is_empty()));
    client.close().await;
    Ok(())
}

struct SavedDisplayFactory {
    inner: MemberFactory,
    seen: Mutex<Vec<(String, String, String)>>,
}
impl rtao_cli::call::RawSignerFactory for SavedDisplayFactory {
    fn source<'a>(
        &'a self,
        ctx: &'a AppContext,
        selection: &'a rtao_cli::call::SigningSelection,
    ) -> BoxFuture<'a, Result<Arc<dyn DynKeySource>, rtao_cli::error::CliError>> {
        Box::pin(async move {
            self.seen.lock().map_err(callback_lock)?.push((
                selection.wallet.clone(),
                selection.display_wallet().to_owned(),
                ctx.globals.wallet_name.clone(),
            ));
            rtao_cli::call::RawSignerFactory::source(&self.inner, ctx, selection).await
        })
    }
    fn release<'a>(
        &'a self,
        selection: &'a rtao_cli::call::SigningSelection,
        keys: &'a dyn DynKeySource,
        success: bool,
    ) -> BoxFuture<'a, Result<(), rtao_cli::error::CliError>> {
        rtao_cli::call::RawSignerFactory::release(&self.inner, selection, keys, success)
    }
}

#[tokio::test]
async fn saved_automatic_alias_display_preserves_the_canonical_source_and_json_receipt()
-> TestResult {
    for json_mode in [false, true] {
        let root = CacheRoot::new()?;
        let wallet_root = root.0.join("wallets");
        let keys = members()?;
        for (name, key) in [
            ("wallet10", &keys[0]),
            ("wallet2", &keys[0]),
            ("wallet3", &keys[1]),
        ] {
            let directory = wallet_root.join(name);
            std::fs::create_dir_all(&directory)?;
            std::fs::write(
                directory.join("coldkeypub.txt"),
                serde_json::to_vec(&json!({"ss58Address":key.key.account(),"cryptoType":0}))?,
            )?;
            // Public existence-only planner sentinel. The injected source owns
            // all actual signing; this text must never be read as private key.
            std::fs::write(directory.join("coldkey"), b"public planner fixture")?;
        }
        let (client, chain, _) = fixture(1_000_000_000).await?;
        let runtime = client.refresh_runtime().await?;
        let group =
            rtao::tx::Multisig::new(&runtime, &[keys[0].key.account(), keys[1].key.account()], 2)?;
        let inner = rtao_runtime::calls::system::remark(vec![8, 9]).compose(&runtime)?;
        chain.mutate_state(|state| {
            state.queue_submit(rtao_testkit::SubmitScript::InBlockThenFinalized {
                events: success_events(Some((keys[0].key.account(), &group, inner.hash()))),
            })
        });
        let flags: &[&str] = if json_mode {
            &["--yes", "--json", "--no-mev-shield"]
        } else {
            &["--yes", "--no-mev-shield"]
        };
        let (mut ctx, sink, reads) = context(flags)?;
        ctx.globals.wallet_path = wallet_root.clone();
        ctx.globals.wallet_name = "team".into();
        ctx.globals.wallet_given = true;
        ctx.globals.signatories.clear();
        ctx.books = Books::from_values(
            json!([]),
            json!([]),
            json!([{"name":"team","threshold":2,"signatories":["wallet2","wallet3"]}]),
        );
        let factory = SavedDisplayFactory {
            inner: factory(&keys),
            seen: Mutex::new(Vec::new()),
        };
        let result = ctx
            .submit_saved_with_factory(
                &client,
                &Preview { blocked: false },
                false,
                None,
                &factory,
                &CallCache::default(),
            )
            .await?
            .ok_or("receipt")?;
        assert!(result.success);
        assert_eq!(result.data["multisig_followup"]["status"], "executed");
        assert_eq!(
            *factory.seen.lock().map_err(|_| "selection capture")?,
            vec![("wallet2".into(), "wallet10".into(), "wallet2".into())]
        );
        assert_eq!(factory.inner.acquired.load(Ordering::SeqCst), 1);
        assert_eq!(
            *factory.inner.releases.lock().map_err(|_| "releases")?,
            vec![true]
        );
        assert_eq!(keys[0].unlocks.load(Ordering::SeqCst), 1);
        assert_eq!(keys[1].unlocks.load(Ordering::SeqCst), 0);
        assert_eq!(keys[2].unlocks.load(Ordering::SeqCst), 0);
        assert_eq!(
            chain.state().lock().map_err(|_| "state")?.submissions.len(),
            1
        );
        assert_eq!(reads.load(Ordering::SeqCst), 0);
        assert_eq!(
            std::fs::read(wallet_root.join("wallet2/coldkey"))?,
            b"public planner fixture"
        );
        let stdout = sink.text(Stream::Stdout)?;
        let stderr = sink.text(Stream::Stderr)?;
        if json_mode {
            assert_eq!(serde_json::from_str::<Value>(&stdout)?, result.to_json());
            assert!(stderr.is_empty());
        } else {
            assert!(stdout.contains("approval member"));
            assert!(stdout.contains("wallet10 via wallet"));
            assert!(stderr.contains("approval 1 of 2  wallet10 via wallet"));
            assert!(!stderr.contains("approval 2 of 2"));
        }
        client.close().await;
    }
    Ok(())
}

#[tokio::test]
async fn external_carrier_shortfall_downgrades_default_refuses_forced_and_keeps_preview_public()
-> TestResult {
    // Exercise the real context/factory/executor with production intents.
    // Fake inclusion does not assert runtime fee payment or stake admission.
    for (case, required, flag_forced, config_forced, dry_run) in [
        ("default", false, false, false, false),
        ("flag", false, true, false, false),
        ("config", false, false, true, false),
        ("required", true, false, false, false),
        ("flag preview", false, true, false, true),
        ("required preview", true, false, false, true),
    ] {
        let (client, chain, keys) = fixture(0).await?;
        chain.mutate_state(|state| -> TestResult {
            state.fee = 100_000;
            state.seed_python("MevShield", "NextKey", &[], json!(vec![171u8; 1184]))?;
            state.seed_python("SubtensorModule", "Burn", &[json!(3)], json!(1u64))?;
            state.seed_python(
                "SubtensorModule",
                "CollateralLockShare",
                &[json!(3)],
                json!(0u16),
            )?;
            Ok(())
        })?;
        let mut flags = vec!["--yes", "--signer", "extension"];
        if flag_forced {
            flags.push("--mev-shield");
        }
        if dry_run {
            flags.extend(["--dry-run", "--json"]);
        }
        let (mut ctx, sink, reads) = context(&flags)?;
        if config_forced {
            ctx.config.insert("mev_shield".into(), json!(true));
        }
        let intent = if required {
            rtao_intents::AnyIntent::from_tool(
                "burned_register",
                json!({"netuid":3,"hotkey_ss58":keys.key.account()}),
            )?
        } else {
            rtao_intents::AnyIntent::from_tool(
                "swap_stake",
                json!({
                    "hotkey_ss58":keys.key.account(),"origin_netuid":1,"dest_netuid":2,
                    "amount_alpha":"1.0","slippage_protection":false
                }),
            )?
        };
        let factory = DirectExternalFactory::new(&keys, false);
        let hooks = Hooks {
            before: AtomicUsize::new(0),
            after: AtomicUsize::new(0),
            failed: AtomicUsize::new(0),
        };
        let outcome = ctx
            .submit_with_options(
                &client,
                intent.as_dyn(),
                SubmissionOptions {
                    hooks: Some(&hooks),
                    ..Default::default()
                },
                Some(&factory),
                &mut rtao_cli::prompt::PromptHistory::default(),
            )
            .await;
        let submits = chain.mutate_state(|state| state.submissions.clone());
        let author_calls = chain
            .requests()
            .iter()
            .filter(|request| request.method.starts_with("author_"))
            .count();
        if dry_run {
            assert!(outcome?.is_none(), "{case}");
            let plan: Value = serde_json::from_str(&sink.text(Stream::Stdout)?)?;
            assert_eq!(plan["op"], intent.as_dyn().spec().name);
            assert_eq!(plan["ok"], true);
            for (field, text) in [
                ("effects", "submitted MEV-shielded"),
                ("warnings", "cannot cover the MEV-shield outer carrier fee"),
            ] {
                assert!(
                    plan[field]
                        .as_array()
                        .ok_or("preview list")?
                        .iter()
                        .any(|value| value.as_str().is_some_and(|value| value.contains(text))),
                    "{case}: {field}"
                );
            }
        } else if required || flag_forced || config_forced {
            let error = outcome.err().ok_or("external carrier refusal")?;
            assert_eq!(error.code, 1, "{case}");
            assert_eq!(
                error.message,
                "MEV-shielded submission needs free TAO for the outer carrier fee"
            );
            assert_eq!(
                error.help.as_deref(),
                Some(if required {
                    "burned_register cannot submit unshielded"
                } else {
                    "pass --no-mev-shield to submit unshielded (alpha fees work on the bare call)"
                })
            );
        } else {
            let receipt = outcome?.ok_or("default downgraded receipt")?;
            assert!(receipt.success);
            assert_eq!(submits.len(), 1);
            assert_eq!(
                chain
                    .requests()
                    .iter()
                    .filter(|request| matches!(
                        request.method.as_str(),
                        "author_submitExtrinsic" | "author_submitAndWatchExtrinsic"
                    ))
                    .count(),
                1,
            );
            let runtime = client.refresh_runtime().await?;
            let decoded =
                rtao::core::DynValue::new(runtime.core().decode_extrinsic(&submits[0], true)?)
                    .to_json();
            assert_eq!(decoded["call"]["call_module"], "SubtensorModule");
            assert_eq!(decoded["call"]["call_function"], "swap_stake");
            assert!(
                sink.text(Stream::Stderr)?
                    .contains("MEV shield skipped: free TAO cannot cover the outer carrier fee")
            );
        }
        let submitted = case == "default";
        assert_eq!(
            factory.acquired.load(Ordering::SeqCst),
            1,
            "one public source: {case}"
        );
        assert_eq!(
            *factory.released.lock().map_err(|_| "release")?,
            vec![submitted]
        );
        assert_eq!(keys.unlocks.load(Ordering::SeqCst), usize::from(submitted));
        assert_eq!(hooks.before.load(Ordering::SeqCst), usize::from(submitted));
        assert_eq!(hooks.after.load(Ordering::SeqCst), usize::from(submitted));
        assert_eq!(hooks.failed.load(Ordering::SeqCst), 0);
        assert_eq!(reads.load(Ordering::SeqCst), 0);
        if !submitted {
            assert!(submits.is_empty(), "{case}");
            assert_eq!(author_calls, 0, "{case}");
        }
        client.close().await;
    }
    Ok(())
}

// Inspect actual human stages without depending on renderer padding widths.
fn review_stage_titles(card: &str) -> Vec<&str> {
    card.lines()
        .filter(|line| !line.is_empty() && !line.starts_with(' '))
        .collect()
}
fn review_stage_rows(
    card: &str,
    title: &str,
) -> Result<Vec<(String, String)>, Box<dyn std::error::Error>> {
    card.lines()
        .skip_while(|line| *line != title)
        .skip(1)
        .take_while(|line| line.starts_with("  "))
        .map(|line| {
            let (key, value) = line
                .trim_start()
                .split_once("  ")
                .ok_or("review row separator")?;
            Ok((key.trim_end().to_owned(), value.trim_start().to_owned()))
        })
        .collect()
}

#[tokio::test]
async fn universal_review_stages_keep_real_semantics_and_fee_failures_public_before_refusal()
-> TestResult {
    for unavailable in [false, true] {
        for batch in [false, true] {
            let (client, chain, keys) = fixture(1_000_000_000).await?;
            chain.mutate_state(|state| {
                state.fee = 100_000;
                if unavailable {
                    state.set_handler("state_call", |state, request| {
                        if request.params[0] == "TransactionPaymentApi_query_info" {
                            rtao_testkit::Reply::error(
                                -32603,
                                "review fee fixture unavailable",
                                None,
                            )
                        } else {
                            state.dispatch(request)
                        }
                    });
                }
            });
            let dest = AccountId([91; 32]);
            let first =
                json!({"op":"transfer","dest_ss58":dest,"amount_tao":"0.1","keep_alive":true});
            let second =
                json!({"op":"transfer","dest_ss58":dest,"amount_tao":"0.2","keep_alive":false});
            let op = if batch { "batch" } else { "transfer" };
            let intent = rtao_intents::AnyIntent::from_tool(
                op,
                if batch {
                    json!({"intents":[first,second]})
                } else {
                    first
                },
            )?;
            let original = intent.to_json();
            let (mut ctx, sink, _) = context(&["--no-mev-shield"])?;
            let proxy = AccountId([55; 32]);
            ctx.globals.proxy_for = Some(proxy.to_string());
            let questions = Arc::new(Mutex::new(Vec::new()));
            ctx.terminal = Arc::new(Mutex::new(Box::new(RegistrationConfirmation {
                questions: questions.clone(),
            })));
            let error = ctx
                .submit_with(&client, intent.as_dyn(), &keys, false, None)
                .await
                .err()
                .ok_or("declined review")?;
            assert_eq!(error.message, "aborted.");
            let card = sink.text(Stream::Stdout)?;
            assert_eq!(
                review_stage_titles(&card),
                [
                    if batch { "Batch" } else { "Transfer" },
                    "Effects",
                    "Fees",
                    "Signer",
                    "Transaction"
                ]
            );
            assert_eq!(
                review_stage_rows(&card, "Fees")?,
                [(
                    "estimated fee".to_owned(),
                    if unavailable {
                        "unavailable".to_owned()
                    } else {
                        "~τ0.000100000".to_owned()
                    }
                )]
            );
            let signer = review_stage_rows(&card, "Signer")?;
            assert!(signer.contains(&("signer".into(), keys.key.account().to_string())));
            assert!(signer.contains(&("role".into(), "coldkey".into())));
            assert!(signer.contains(&("proxy for".into(), proxy.to_string())));
            assert!(signer.contains(&(
                "mev shield".into(),
                "off — no protection (call is visible in the mempool)".into()
            )));
            let transaction = review_stage_rows(&card, "Transaction")?;
            assert!(transaction.contains(&("operation".into(), op.into())));
            if batch {
                assert_eq!(
                    transaction,
                    vec![
                        ("operation".into(), "batch".into()),
                        ("call 1".into(), format!("transfer τ0.100000000 to {dest}")),
                        ("call 2".into(), format!("transfer τ0.200000000 to {dest}")),
                    ]
                );
            } else {
                assert!(transaction.contains(&("dest".into(), dest.to_string())));
                assert!(transaction.contains(&("amount tao".into(), "τ0.100000000".into())));
                assert!(transaction.contains(&("keep alive".into(), "yes".into())));
                assert_eq!(transaction.len(), 4);
            }
            let stderr = sink.text(Stream::Stderr)?;
            assert_eq!(stderr.contains("could not estimate fee"), unavailable);
            if unavailable {
                assert!(stderr.contains("review fee fixture unavailable"));
            }
            assert!(
                chain
                    .requests()
                    .iter()
                    .any(|request| request.method == "state_call"
                        && request.params[0] == "TransactionPaymentApi_query_info")
            );
            assert_eq!(questions.lock().map_err(|_| "questions")?.len(), 1);
            assert_eq!(intent.to_json(), original);
            assert_eq!(keys.unlocks.load(Ordering::SeqCst), 0);
            assert!(chain.mutate_state(|state| state.submissions.is_empty()));
            assert!(
                !chain
                    .requests()
                    .iter()
                    .any(|request| request.method.starts_with("author_"))
            );
            assert_eq!(
                client
                    .nonces()
                    .cached(&keys.key.account().to_string())
                    .await,
                None
            );
            client.close().await;
        }
    }
    Ok(())
}

#[tokio::test]
async fn command_fee_facts_replace_estimate_without_suppressing_failed_probe_warnings() -> TestResult
{
    let (client, chain, keys) = fixture(1_000_000_000).await?;
    chain.mutate_state(|state| {
        state.set_handler("state_call", |state, request| {
            if request.params[0] == "TransactionPaymentApi_query_info" {
                rtao_testkit::Reply::error(-32603, "structured fee probe unavailable", None)
            } else {
                state.dispatch(request)
            }
        })
    });
    let intent = CommandFactsFixture { fee_facts: true };
    let review = command_review(&intent);
    let (mut ctx, sink, _) = context(&["--no-mev-shield"])?;
    let questions = Arc::new(Mutex::new(Vec::new()));
    ctx.terminal = Arc::new(Mutex::new(Box::new(RegistrationConfirmation { questions })));
    let error = ctx
        .submit_with_keys_options(
            &client,
            &intent,
            &keys,
            SubmissionOptions {
                review: Some(&review),
                ..Default::default()
            },
        )
        .await
        .err()
        .ok_or("declined structured fees")?;
    assert_eq!(error.message, "aborted.");
    let card = sink.text(Stream::Stdout)?;
    assert_eq!(
        review_stage_titles(&card),
        ["Allocate", "Effects", "Fees", "Signer", "Transaction"]
    );
    assert_eq!(
        review_stage_rows(&card, "Fees")?,
        [("inclusion".to_owned(), "τ0.000020000".to_owned())]
    );
    assert!(!card.contains("estimated fee"));
    assert!(
        sink.text(Stream::Stderr)?
            .contains("could not estimate fee")
    );
    assert!(
        sink.text(Stream::Stderr)?
            .contains("structured fee probe unavailable")
    );
    assert_eq!(keys.unlocks.load(Ordering::SeqCst), 0);
    assert!(chain.mutate_state(|state| state.submissions.is_empty()));
    assert!(
        !chain
            .requests()
            .iter()
            .any(|request| request.method.starts_with("author_"))
    );
    client.close().await;
    Ok(())
}

struct RootYieldHooks {
    chain: FakeChain,
    hotkey: AccountId,
    owner: AccountId,
    payout: Option<u64>,
    before: AtomicUsize,
    after: AtomicUsize,
    payout_reads_at_receipt: AtomicUsize,
    raw: Mutex<Option<Value>>,
}
impl RootYieldHooks {
    fn new(chain: &FakeChain, hotkey: AccountId, owner: AccountId, payout: Option<u64>) -> Self {
        Self {
            chain: chain.clone(),
            hotkey,
            owner,
            payout,
            before: AtomicUsize::new(0),
            after: AtomicUsize::new(0),
            payout_reads_at_receipt: AtomicUsize::new(0),
            raw: Mutex::new(None),
        }
    }
    fn payout_reads(&self) -> usize {
        self.chain
            .requests()
            .iter()
            .filter(|request| {
                request.method == "state_call"
                    && request.params[0] == "BetaBasketRuntimeApi_get_basket_payout"
            })
            .count()
    }
    fn assert_original(&self, result: &rtao::tx::ExtrinsicResult) -> TestResult {
        assert_eq!(self.before.load(Ordering::SeqCst), 1);
        assert_eq!(self.after.load(Ordering::SeqCst), 1);
        assert_eq!(
            *self.raw.lock().map_err(|_| "root raw")?,
            Some(result.to_json())
        );
        Ok(())
    }
}
impl rtao_cli::context::SubmissionHooks for RootYieldHooks {
    fn before_submit(&self) -> BoxFuture<'_, Result<(), rtao_cli::error::CliError>> {
        Box::pin(async move {
            self.before.fetch_add(1, Ordering::SeqCst);
            Ok(())
        })
    }
    fn on_result<'a>(
        &'a self,
        result: &'a rtao::tx::ExtrinsicResult,
    ) -> BoxFuture<'a, Result<(), rtao_cli::error::CliError>> {
        Box::pin(async move {
            self.after.fetch_add(1, Ordering::SeqCst);
            self.payout_reads_at_receipt
                .store(self.payout_reads(), Ordering::SeqCst);
            *self
                .raw
                .lock()
                .map_err(|_| rtao_cli::error::CliError::failure("root raw"))? =
                Some(result.to_json());
            let input = [self.hotkey.0.as_slice(), self.owner.0.as_slice()].concat();
            let output = self
                .payout
                .map_or_else(|| vec![255], |value| value.to_le_bytes().to_vec());
            self.chain.mutate_state(|state| {
                state.seed_runtime_encoded("BetaBasketRuntimeApi_get_basket_payout", input, output)
            });
            Ok(())
        })
    }
}
fn seed_root_yield(chain: &FakeChain) -> TestResult {
    chain.mutate_state(|state| -> TestResult {
        state.seed_runtime("BetaBasketRuntimeApi", "get_basket_payout", json!(0u64))?;
        state.seed_runtime("BetaBasketRuntimeApi", "get_validator_basket", json!([]))?;
        state.seed_python("SubtensorModule", "NetworksAdded", &[json!(0)], json!(true))?;
        state.seed_python(
            "SubtensorModule",
            "RootStakeUnlockInterval",
            &[],
            json!(0u64),
        )?;
        Ok(())
    })
}
fn root_exit_intent(
    op: &str,
    hotkey: AccountId,
    claim: bool,
    netuid: u16,
) -> Result<rtao_intents::AnyIntent, Box<dyn std::error::Error>> {
    let mut args = json!({"hotkey_ss58":hotkey,"claim":claim});
    if matches!(op, "remove_stake" | "remove_stake_limit") {
        args["netuid"] = json!(netuid);
        args["amount_alpha"] = json!("1.0");
        if op == "remove_stake" {
            args["slippage_protection"] = json!(false);
        } else {
            args["limit_price_rao"] = json!(0u64);
        }
    }
    Ok(rtao_intents::AnyIntent::from_tool(op, args)?)
}
fn assert_submitted_root_claim(
    bytes: &[u8],
    runtime: &rtao_runtime::RuntimeHandle,
    hotkey: AccountId,
) -> TestResult {
    let decoded =
        rtao::core::DynValue::new(runtime.core().decode_extrinsic(bytes, true)?).to_json();
    assert_eq!(decoded["call"]["call_module"], "SubtensorModule");
    assert_eq!(decoded["call"]["call_function"], "claim_root_with_hotkey");
    let args = decoded["call"]["call_args"]
        .as_array()
        .ok_or("root claim args")?;
    assert_eq!(
        args.iter()
            .find(|a| a["name"] == "hotkey")
            .ok_or("hotkey")?["value"],
        json!(hotkey)
    );
    Ok(())
}

#[tokio::test]
async fn post_root_unstake_hints_use_fresh_yield_without_automatic_claims() -> TestResult {
    for op in ["remove_stake", "remove_stake_limit", "unstake_all"] {
        for mode in ["yes", "json", "quiet", "piped"] {
            let (client, chain, keys) = fixture(1_000_000_000_000).await?;
            seed_root_yield(&chain)?;
            let mut flags = vec!["--yes", "--no-mev-shield"];
            if mode == "json" {
                flags.push("--json");
            }
            if mode == "quiet" {
                flags.push("--quiet");
            }
            let (mut ctx, sink, reads) = context(&flags)?;
            if mode == "piped" {
                ctx.terminal = Arc::new(Mutex::new(Box::new(TerminalFixture {
                    tty: false,
                    lines: VecDeque::new(),
                    reads: reads.clone(),
                })));
            }
            let hotkey = AccountId([79; 32]);
            let intent = root_exit_intent(op, hotkey, false, 0)?;
            let hooks = RootYieldHooks::new(&chain, hotkey, keys.key.account(), Some(900_000));
            let result = ctx
                .submit_with_keys_options(
                    &client,
                    intent.as_dyn(),
                    &keys,
                    SubmissionOptions {
                        hooks: Some(&hooks),
                        ..Default::default()
                    },
                )
                .await?
                .ok_or("root receipt")?;
            assert!(result.success);
            hooks.assert_original(&result)?;
            assert_eq!(
                hooks.payout_reads() - hooks.payout_reads_at_receipt.load(Ordering::SeqCst),
                1
            );
            let stderr = sink.text(Stream::Stderr)?;
            if mode == "json" || mode == "quiet" {
                assert!(stderr.is_empty());
            } else {
                assert!(stderr.contains(&format!("τ0.000900000 remains claimable via \u{0060}rtao root claim --hotkey {hotkey} --coldkey {} --proxy-for self\u{0060}",keys.key.account())));
                assert!(stderr.contains("unstaking root principal does not claim basket yield"));
                assert!(stderr.contains("whole entitlement, not a slice of the unstake"));
            }
            if mode == "json" {
                assert_eq!(
                    serde_json::from_str::<Value>(&sink.text(Stream::Stdout)?)?,
                    result.to_json()
                );
            }
            assert_eq!(reads.load(Ordering::SeqCst), 0);
            assert_eq!(keys.unlocks.load(Ordering::SeqCst), 1);
            assert_eq!(chain.mutate_state(|state| state.submissions.len()), 1);
            client.close().await;
        }
    }
    Ok(())
}

struct RootYieldTerminal {
    inner: TerminalFixture,
    questions: Arc<Mutex<Vec<String>>>,
}
impl Terminal for RootYieldTerminal {
    fn stdin_is_tty(&self) -> bool {
        self.inner.stdin_is_tty()
    }
    fn stderr_is_tty(&self) -> bool {
        self.inner.stderr_is_tty()
    }
    fn read_line(&mut self, question: &str) -> Result<String, PromptError> {
        self.questions
            .lock()
            .map_err(|_| PromptError::Unavailable)?
            .push(question.into());
        self.inner.read_line(question)
    }
    fn read_secret(&mut self, prompt: &str) -> Result<rtao::wallet::SecretString, PromptError> {
        self.inner.read_secret(prompt)
    }
    fn message(&mut self, message: &str) -> Result<(), PromptError> {
        self.inner.message(message)
    }
}

#[tokio::test]
async fn post_root_unstake_offer_keeps_the_original_receipt_and_requires_normal_claim_review()
-> TestResult {
    for (answers, claimed, error_code) in [
        (vec!["yes", "no"], false, None),
        (vec!["yes", "yes", "no"], false, Some(1)),
        (vec!["yes", "yes", "yes"], true, None),
        (vec!["yes"], false, Some(130)),
    ] {
        let (client, chain, keys) = fixture(1_000_000_000_000).await?;
        seed_root_yield(&chain)?;
        let (mut ctx, sink, reads) = context(&["--no-mev-shield"])?;
        let questions = Arc::new(Mutex::new(Vec::new()));
        ctx.terminal = Arc::new(Mutex::new(Box::new(RootYieldTerminal {
            inner: TerminalFixture {
                tty: true,
                lines: answers.iter().map(|s| (*s).to_owned()).collect(),
                reads: reads.clone(),
            },
            questions: questions.clone(),
        })));
        let hotkey = AccountId([79; 32]);
        let intent = root_exit_intent("remove_stake", hotkey, false, 0)?;
        let hooks = RootYieldHooks::new(&chain, hotkey, keys.key.account(), Some(900_000));
        let outcome = ctx
            .submit_with_keys_options(
                &client,
                intent.as_dyn(),
                &keys,
                SubmissionOptions {
                    hooks: Some(&hooks),
                    ..Default::default()
                },
            )
            .await;
        if let Some(code) = error_code {
            let error = outcome.err().ok_or("followup refusal")?;
            assert_eq!(error.code, code);
            assert!(
                error
                    .help
                    .as_deref()
                    .is_some_and(|help| help.contains("transaction was already submitted"))
            );
        } else {
            let original = outcome?.ok_or("original unstake receipt")?;
            hooks.assert_original(&original)?;
        }
        assert_eq!(
            hooks.before.load(Ordering::SeqCst),
            1,
            "claim must not reuse the unstake journal"
        );
        assert_eq!(hooks.after.load(Ordering::SeqCst), 1);
        assert_eq!(
            keys.unlocks.load(Ordering::SeqCst),
            1 + usize::from(claimed)
        );
        assert_eq!(
            reads.load(Ordering::SeqCst),
            if answers.len() == 1 { 2 } else { answers.len() }
        );
        assert_eq!(
            questions.lock().map_err(|_| "questions")?[1],
            "τ0.000900000 remains claimable. Claim this validator's whole basket entitlement now? [y/N]: "
        );
        let submissions = chain.mutate_state(|state| state.submissions.clone());
        assert_eq!(submissions.len(), 1 + usize::from(claimed));
        if claimed {
            assert_submitted_root_claim(&submissions[1], &client.refresh_runtime().await?, hotkey)?;
        } else if error_code.is_none() {
            assert!(
                sink.text(Stream::Stderr)?
                    .contains("whole entitlement, not a slice of the unstake")
            );
        }
        client.close().await;
    }
    Ok(())
}

struct ReleasedBeforeRootClaim<'a> {
    inner: &'a DirectExternalFactory,
}
impl rtao_cli::context::ExternalSignerFactory for ReleasedBeforeRootClaim<'_> {
    fn keys<'a>(
        &'a self,
        globals: &'a Globals,
        role: SignerRole,
    ) -> BoxFuture<'a, Result<Arc<dyn DynKeySource>, rtao_cli::error::CliError>> {
        Box::pin(async move {
            if self.inner.acquired.load(Ordering::SeqCst) > 0 {
                assert_eq!(
                    *self
                        .inner
                        .released
                        .lock()
                        .map_err(|_| rtao_cli::error::CliError::failure("release"))?,
                    vec![true]
                );
            }
            rtao_cli::context::ExternalSignerFactory::keys(self.inner, globals, role).await
        })
    }
    fn release<'a>(
        &'a self,
        globals: &'a Globals,
        keys: &'a dyn DynKeySource,
        success: bool,
    ) -> BoxFuture<'a, Result<(), rtao_cli::error::CliError>> {
        rtao_cli::context::ExternalSignerFactory::release(self.inner, globals, keys, success)
    }
}
#[tokio::test]
async fn external_root_claim_reacquires_after_release_and_uses_the_effective_proxy_owner()
-> TestResult {
    for (proxy, automatic) in [(false, false), (true, false), (false, true), (true, true)] {
        let (client, chain, keys) = fixture(1_000_000_000_000).await?;
        seed_root_yield(&chain)?;
        let mut flags = vec!["--no-mev-shield", "--signer", "extension"];
        if automatic {
            flags.push("--yes");
        }
        let (mut ctx, sink, reads) = context(&flags)?;
        ctx.terminal = Arc::new(Mutex::new(Box::new(TerminalFixture {
            tty: true,
            lines: VecDeque::from(["yes".into(), "yes".into(), "yes".into()]),
            reads: reads.clone(),
        })));
        let owner = if proxy {
            AccountId([88; 32])
        } else {
            keys.key.account()
        };
        if proxy {
            ctx.globals.proxy_for = Some(owner.to_string());
        }
        let hotkey = AccountId([79; 32]);
        let intent = root_exit_intent("remove_stake", hotkey, false, 0)?;
        let inner = DirectExternalFactory::new(&keys, false);
        let factory = ReleasedBeforeRootClaim { inner: &inner };
        let hooks = RootYieldHooks::new(&chain, hotkey, owner, Some(900_000));
        let result = ctx
            .submit_with_options(
                &client,
                intent.as_dyn(),
                SubmissionOptions {
                    hooks: Some(&hooks),
                    ..Default::default()
                },
                Some(&factory),
                &mut rtao_cli::prompt::PromptHistory::default(),
            )
            .await?
            .ok_or("external original receipt")?;
        hooks.assert_original(&result)?;
        let expected = if automatic { 1 } else { 2 };
        assert_eq!(inner.acquired.load(Ordering::SeqCst), expected);
        assert_eq!(
            *inner.released.lock().map_err(|_| "released")?,
            vec![true; expected]
        );
        assert_eq!(keys.unlocks.load(Ordering::SeqCst), expected);
        assert_eq!(reads.load(Ordering::SeqCst), if automatic { 0 } else { 3 });
        let submissions = chain.mutate_state(|state| state.submissions.clone());
        assert_eq!(submissions.len(), expected);
        if automatic {
            let proxy_flag = if proxy {
                owner.to_string()
            } else {
                "self".to_owned()
            };
            let command = format!(
                "rtao root claim --hotkey {hotkey} --coldkey {owner} --proxy-for {proxy_flag}"
            );
            let stderr = sink.text(Stream::Stderr)?;
            assert!(stderr.contains(&command));
            assert!(stderr.contains("same signing wallet, backend and proxy type"));
            // Use the real installed tree and Request::Claim parser. A wrong
            // configured proxy must not override this hint's explicit route.
            let parsed = rtao_cli::commands::command_tree()?
                .try_get_matches_from(command.split_ascii_whitespace())?;
            let group = parsed.subcommand().ok_or("root group")?.1;
            let leaf = group.subcommand().ok_or("claim leaf")?.1;
            let mut replay = ctx.clone();
            replay.globals.proxy_for = None;
            replay
                .config
                .insert("proxy_for".into(), json!(AccountId([99; 32])));
            replay
                .globals
                .apply(leaf, &rtao_cli::commands::root::reserved(group)?)?;
            let request = rtao_cli::commands::root::Request::from_matches(
                &mut replay,
                group,
                &mut rtao_cli::prompt::PromptHistory::default(),
            )?;
            let rtao_cli::commands::root::Request::Claim {
                hotkey: replay_hotkey,
                owner: replay_owner,
            } = request
            else {
                return Err("hint is not a root claim request".into());
            };
            assert_eq!(replay_hotkey, Some(hotkey));
            assert_eq!(replay_owner, owner);
            assert_eq!(replay.dispatch_proxy(None)?, proxy.then_some(owner));
            client.close().await;
            continue;
        }
        let runtime = client.refresh_runtime().await?;
        let decoded =
            rtao::core::DynValue::new(runtime.core().decode_extrinsic(&submissions[1], true)?)
                .to_json();
        let call = if proxy {
            assert_eq!(decoded["call"]["call_module"], "Proxy");
            let args = decoded["call"]["call_args"]
                .as_array()
                .ok_or("proxy args")?;
            assert_eq!(
                args.iter().find(|a| a["name"] == "real").ok_or("real")?["value"],
                json!(owner)
            );
            args.iter().find(|a| a["name"] == "call").ok_or("inner")?["value"].clone()
        } else {
            decoded["call"].clone()
        };
        assert_eq!(call["call_function"], "claim_root_with_hotkey");
        let args = call["call_args"].as_array().ok_or("claim args")?;
        assert_eq!(
            args.iter()
                .find(|a| a["name"] == "hotkey")
                .ok_or("claim hotkey")?["value"],
            json!(hotkey)
        );
        client.close().await;
    }
    Ok(())
}

#[tokio::test]
async fn post_root_yield_skips_empty_failed_nontrigger_and_saved_approval_outcomes() -> TestResult {
    for (op, claim, netuid, payout, failed) in [
        ("remove_stake", false, 0, Some(0), false),
        ("remove_stake", false, 0, None, false),
        ("remove_stake", true, 0, Some(900_000), false),
        ("remove_stake_limit", false, 1, Some(900_000), false),
        ("remove_stake", false, 0, Some(900_000), true),
    ] {
        let (client, chain, keys) = fixture(1_000_000_000_000).await?;
        seed_root_yield(&chain)?;
        if failed {
            chain.mutate_state(|state|state.queue_submit(rtao_testkit::SubmitScript::InBlockThenFinalized{
            events:json!([{"phase":"ApplyExtrinsic","extrinsic_idx":0,"module_id":"System","event_id":"ExtrinsicFailed","attributes":{"dispatch_error":{"BadOrigin":null},"dispatch_info":{"weight":{"ref_time":1,"proof_size":0},"class":"Normal","pays_fee":"Yes"}},"topics":[]}]),
        }));
        }
        let (ctx, sink, reads) = context(&["--yes", "--no-mev-shield"])?;
        let hotkey = AccountId([79; 32]);
        let intent = root_exit_intent(op, hotkey, claim, netuid)?;
        let hooks = RootYieldHooks::new(&chain, hotkey, keys.key.account(), payout);
        let outcome = ctx
            .submit_with_keys_options(
                &client,
                intent.as_dyn(),
                &keys,
                SubmissionOptions {
                    hooks: Some(&hooks),
                    ..Default::default()
                },
            )
            .await;
        if failed {
            assert!(outcome.is_err());
        } else {
            hooks.assert_original(&outcome?.ok_or("original")?)?;
        }
        assert_eq!(
            hooks.payout_reads() - hooks.payout_reads_at_receipt.load(Ordering::SeqCst),
            usize::from(!claim && netuid == 0 && !failed)
        );
        assert!(
            !sink
                .text(Stream::Stderr)?
                .contains("via \u{0060}rtao root claim --hotkey")
        );
        assert_eq!(reads.load(Ordering::SeqCst), 0);
        assert_eq!(chain.mutate_state(|state| state.submissions.len()), 1);
        client.close().await;
    }
    let (client, chain, keys) = fixture(1_000_000_000_000).await?;
    seed_root_yield(&chain)?;
    let (ctx, sink, reads) = context(&["--dry-run", "--json", "--no-mev-shield"])?;
    let hotkey = AccountId([79; 32]);
    let intent = root_exit_intent("remove_stake", hotkey, false, 0)?;
    let hooks = RootYieldHooks::new(&chain, hotkey, keys.key.account(), Some(900_000));
    assert!(
        ctx.submit_with_keys_options(
            &client,
            intent.as_dyn(),
            &keys,
            SubmissionOptions {
                hooks: Some(&hooks),
                ..Default::default()
            },
        )
        .await?
        .is_none()
    );
    assert_eq!(hooks.before.load(Ordering::SeqCst), 0);
    assert_eq!(hooks.after.load(Ordering::SeqCst), 0);
    assert_eq!(keys.unlocks.load(Ordering::SeqCst), 0);
    assert_eq!(reads.load(Ordering::SeqCst), 0);
    assert!(chain.mutate_state(|state| state.submissions.is_empty()));
    assert_eq!(
        serde_json::from_str::<Value>(&sink.text(Stream::Stdout)?)?["op"],
        "remove_stake"
    );
    client.close().await;
    // A successful approval has not executed the root unstake. Preserve the
    // saved multisig driver without interpreting a pending round as success.
    let keys = members()?;
    let (client, chain, _) = fixture(1_000_000_000_000).await?;
    seed_root_yield(&chain)?;
    let (ctx, sink) = saved_context(&keys, &["--yes", "--json", "--no-mev-shield"], 1)?;
    let group = saved_group(&client, &keys).await?;
    let hotkey = AccountId([79; 32]);
    let hooks = RootYieldHooks::new(&chain, hotkey, group.address, Some(900_000));
    let intent = root_exit_intent("remove_stake", hotkey, false, 0)?;
    let factory = factory(&keys);
    let result = ctx
        .submit_saved_with_options(
            &client,
            intent.as_dyn(),
            SubmissionOptions {
                hooks: Some(&hooks),
                ..Default::default()
            },
            &factory,
            &CallCache::default(),
        )
        .await?
        .ok_or("saved approval")?;
    assert!(result.success);
    assert_eq!(
        hooks.payout_reads(),
        hooks.payout_reads_at_receipt.load(Ordering::SeqCst)
    );
    assert_eq!(factory.acquired.load(Ordering::SeqCst), 1);
    assert_eq!(chain.mutate_state(|state| state.submissions.len()), 1);
    assert_eq!(
        serde_json::from_str::<Value>(&sink.text(Stream::Stdout)?)?,
        result.to_json()
    );
    client.close().await;
    Ok(())
}

#[tokio::test]
async fn registration_read_failure_persists_direct_and_saved_receipts_before_release() -> TestResult
{
    use rtao::primitives::{IntoCoreValue, NetUid};
    for saved in [false, true] {
        for fail_result in [false, true] {
            for connection_closed in [false, true] {
                let (client, chain, keys) = fixture(1_000_000_000).await?;
                let members = members()?;
                let direct = DirectExternalFactory::new(&keys, true);
                let mut member = factory(&members);
                member.fail_release = true;
                let (mut ctx, mut sink, _) = context(&[
                    "--yes",
                    "--json",
                    "--no-mev-shield",
                    "--signer",
                    "extension",
                ])?;
                if saved {
                    (ctx, sink) =
                        saved_context(&members, &["--yes", "--json", "--no-mev-shield"], 1)?;
                    ctx.books.multisigs[0]["threshold"] = json!(1);
                }
                let interruption = rtao_cli::prompt::TerminalCancel::default();
                ctx = ctx.with_interruption(interruption.clone());
                let releases = if saved {
                    &member.releases
                } else {
                    &direct.released
                };
                let unlocks = if saved {
                    members[0].unlocks.as_ref()
                } else {
                    keys.unlocks.as_ref()
                };
                let hooks = DurablePlanHooks {
                    unlocks,
                    source_acquisitions: saved.then_some(&member.acquired),
                    releases,
                    plans: Mutex::new(Vec::new()),
                    raw: Mutex::new(Vec::new()),
                    enriched: Mutex::new(Vec::new()),
                    fail_result,
                };
                let completion_read = Arc::new(tokio::sync::Notify::new());
                chain.mutate_state(|state| -> TestResult {
                let info = state.runtime.storage_entry("SubtensorModule", "SubnetOwner").ok_or("owner storage")?;
                let key = format!("0x{}", hex::encode(state.runtime.storage_key(info, &[NetUid(9).into_core_value()])?));
                state.set_handler("state_getStorage", {
                    let key = key.clone();
                    let completion_read = completion_read.clone();
                    move |state, request| {
                        if !state.submissions.is_empty() && request.params[0] == key {
                            if connection_closed {
                                completion_read.notify_one();
                                rtao_testkit::Reply::silent()
                            } else {
                                rtao_testkit::Reply::error(-32603, "registration completion read sentinel", None)
                            }
                        } else { state.dispatch(request) }
                    }
                });
                state.set_handler("state_queryStorageAt", {
                  let completion_read = completion_read.clone();
                  move |state, request| {
                    if !state.submissions.is_empty() && request.params[0].as_array().is_some_and(|keys| keys.iter().any(|value| value == &json!(key))) {
                        if connection_closed {
                            completion_read.notify_one();
                            rtao_testkit::Reply::silent()
                        } else {
                            rtao_testkit::Reply::error(-32603, "registration completion read sentinel", None)
                        }
                    } else { state.dispatch(request) }
                  }
                });
                state.queue_submit(rtao_testkit::SubmitScript::InBlockThenFinalized {
                    events: json!([
                        {"phase":"ApplyExtrinsic","extrinsic_idx":0,"module_id":"SubtensorModule","event_id":"NetworkAdded","attributes":[9,0],"topics":[]},
                        {"phase":"ApplyExtrinsic","extrinsic_idx":0,"module_id":"System","event_id":"ExtrinsicSuccess","attributes":{"dispatch_info":{"weight":{"ref_time":1,"proof_size":0},"class":"Normal","pays_fee":"Yes"}},"topics":[]}
                    ])
                });
                Ok(())
            })?;
                let intent = rtao_intents::RegisterSubnet {
                    hotkey_ss58: Some(AccountId([8; 32]).into()),
                };
                let options = SubmissionOptions {
                    hooks: Some(&hooks),
                    ..Default::default()
                };
                let runner = ctx.clone();
                let connector = Connected(client.clone());
                let context = &mut ctx;
                let member_ref = &member;
                let direct_ref = &direct;
                let operation = runner.run_with(&connector, |connected| async move {
                    if saved {
                        context
                            .submit_saved_with_options(
                                &connected,
                                &intent,
                                options,
                                member_ref,
                                &CallCache::default(),
                            )
                            .await
                    } else {
                        context
                            .submit_with_options(
                                &connected,
                                &intent,
                                options,
                                Some(direct_ref),
                                &mut Default::default(),
                            )
                            .await
                    }
                });
                let close = async {
                    if connection_closed {
                        tokio::time::timeout(
                            std::time::Duration::from_secs(2),
                            completion_read.notified(),
                        )
                        .await?;
                        interruption.cancel();
                    }
                    Ok::<(), Box<dyn std::error::Error>>(())
                };
                let (result, closed) = tokio::join!(operation, close);
                closed?;
                assert!(client.is_closed());
                let error = result
                    .err()
                    .ok_or("post-inclusion completion must remain an error")?;
                assert_eq!(error.code, 1);
                assert_eq!(
                    interruption.exit_code(error.code),
                    if connection_closed { 130 } else { 1 }
                );
                if connection_closed {
                    assert_eq!(error.message, "connection is not ready");
                    assert!(matches!(
                        &error.diagnostic,
                        Some(rtao_cli::error::CliDiagnostic::Connection)
                    ));
                } else {
                    assert!(
                        error
                            .message
                            .contains("registration completion read sentinel")
                    );
                }
                assert!(!error.message.contains("could not reach"));
                let retained = error.submitted_receipt.as_deref().ok_or("native receipt")?;
                assert!(retained.success);
                assert_eq!(retained.extrinsic_id.as_deref(), Some("101-0000"));
                assert!(retained.data.get("netuid").is_none());
                assert!(retained.data.get("registered_at_block").is_none());
                let hash = retained.extrinsic_hash.ok_or("exact submitted hash")?;
                let submitted = chain
                    .state()
                    .lock()
                    .map_err(|_| "state")?
                    .submissions
                    .clone();
                assert_eq!(submitted.len(), 1);
                assert_eq!(
                    rtao::core::ops::extrinsic_lookup::find(
                        &json!({"block":{"extrinsics":[format!("0x{}", hex::encode(&submitted[0]))]}}),
                        hash.0
                    )?,
                    Some(0)
                );
                let raw = hooks.raw.lock().map_err(|_| "raw")?.clone();
                assert_eq!(raw.len(), 1);
                assert_eq!(raw[0].extrinsic_hash, Some(hash));
                assert_eq!(raw[0].to_json(), retained.to_json());
                assert!(hooks.enriched.lock().map_err(|_| "enriched")?.is_empty());
                assert_eq!(*releases.lock().map_err(|_| "release")?, vec![true]);
                assert_eq!(unlocks.load(Ordering::SeqCst), 1);
                let help = error.help.as_deref().ok_or("receipt help")?;
                assert!(help.contains(&hex::encode(hash.0)));
                assert!(help.contains("already submitted"));
                assert!(!help.contains("check the endpoint"));
                assert!(!help.contains("then retry"));
                if fail_result {
                    assert!(help.contains("journal receipt persistence failure"));
                }
                assert!(sink.text(Stream::Stdout)?.is_empty());
                client.close().await;
            }
        }
    }
    Ok(())
}

struct CancellingConnector {
    client: Client,
    cancel: rtao_cli::prompt::TerminalCancel,
    calls: AtomicUsize,
    pending: bool,
}
impl rtao_cli::context::ConnectionFactory for CancellingConnector {
    fn connect<'a>(
        &'a self,
        _: ClientConfig,
    ) -> BoxFuture<'a, Result<Client, rtao_cli::error::CliError>> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Box::pin(async move {
            self.cancel.cancel();
            if self.pending {
                std::future::pending::<()>().await;
            }
            Ok(self.client.clone())
        })
    }
}

#[tokio::test]
async fn invocation_interrupt_refuses_preconnect_pending_and_ready_clients_without_work()
-> TestResult {
    for (pre_cancelled, pending) in [(true, false), (false, true), (false, false)] {
        let (client, chain, keys) = fixture(1_000_000_000).await?;
        let (ctx, sink, reads) = context(&["--signer", "extension"])?;
        let cancel = rtao_cli::prompt::TerminalCancel::default();
        let ctx = ctx.with_interruption(cancel.clone());
        let factory = DirectExternalFactory::new(&keys, false);
        let connector = CancellingConnector {
            client: client.clone(),
            cancel: cancel.clone(),
            calls: AtomicUsize::new(0),
            pending,
        };
        if pre_cancelled {
            cancel.cancel();
        }
        let work = AtomicUsize::new(0);
        let error = tokio::time::timeout(
            std::time::Duration::from_secs(2),
            ctx.run_with(&connector, |_| async {
                work.fetch_add(1, Ordering::SeqCst);
                let _ = ctx
                    .signing_keys(SignerRole::Coldkey, Some(&factory))
                    .await?;
                Ok(())
            }),
        )
        .await?
        .err()
        .ok_or("interrupt refusal")?;
        assert_eq!(error.code, 130);
        assert_eq!(
            connector.calls.load(Ordering::SeqCst),
            usize::from(!pre_cancelled)
        );
        assert_eq!(work.load(Ordering::SeqCst), 0);
        assert_eq!(factory.acquired.load(Ordering::SeqCst), 0);
        assert_eq!(keys.unlocks.load(Ordering::SeqCst), 0);
        assert_eq!(reads.load(Ordering::SeqCst), 0);
        assert!(sink.text(Stream::Stdout)?.is_empty());
        assert!(sink.text(Stream::Stderr)?.is_empty());
        assert!(chain.mutate_state(|state| state.submissions.is_empty()));
        if !pre_cancelled && !pending {
            assert!(client.is_closed());
        }
        // The private API independently refuses an already interrupted context.
        assert_eq!(
            ctx.signing_keys(SignerRole::Coldkey, Some(&factory))
                .await
                .err()
                .ok_or("private refusal")?
                .code,
            130
        );
        assert_eq!(factory.acquired.load(Ordering::SeqCst), 0);
        client.close().await;
    }
    Ok(())
}

#[tokio::test]
async fn interrupted_watched_submission_retains_exact_hash_hooks_and_signer_release() -> TestResult
{
    let (client, chain, keys) = fixture(1_000_000_000).await?;
    let submitted = Arc::new(tokio::sync::Notify::new());
    chain.mutate_state(|state| {
        state.queue_submit(rtao_testkit::SubmitScript::Statuses(vec![json!("ready")]));
        let submitted = submitted.clone();
        state.set_handler("author_submitAndWatchExtrinsic", move |state, request| {
            let result = state.dispatch(request);
            submitted.notify_one();
            result
        });
    });
    let (mut ctx, sink, _) = context(&[
        "--yes",
        "--json",
        "--no-mev-shield",
        "--signer",
        "extension",
    ])?;
    let cancel = rtao_cli::prompt::TerminalCancel::default();
    ctx = ctx.with_interruption(cancel.clone());
    let factory = DirectExternalFactory::new(&keys, false);
    let hooks = DurablePlanHooks {
        unlocks: &keys.unlocks,
        source_acquisitions: None,
        releases: &factory.released,
        plans: Mutex::new(Vec::new()),
        raw: Mutex::new(Vec::new()),
        enriched: Mutex::new(Vec::new()),
        fail_result: false,
    };
    let runner = ctx.clone();
    let connector = Connected(client.clone());
    let context = &mut ctx;
    let factory_ref = &factory;
    let hooks_ref = &hooks;
    let operation = Box::pin(runner.run_with(&connector, |connected| async move {
        context
            .submit_with_options(
                &connected,
                &Preview { blocked: false },
                SubmissionOptions {
                    hooks: Some(hooks_ref),
                    deadline: Some(std::time::Duration::from_secs(5)),
                    ..Default::default()
                },
                Some(factory_ref),
                &mut Default::default(),
            )
            .await
    }));
    let interrupt = async {
        tokio::time::timeout(std::time::Duration::from_secs(2), submitted.notified()).await?;
        cancel.cancel();
        Ok::<(), Box<dyn std::error::Error>>(())
    };
    let (result, interrupted) = tokio::time::timeout(std::time::Duration::from_secs(8), async {
        tokio::join!(operation, interrupt)
    })
    .await?;
    interrupted?;
    let error = result.err().ok_or("unknown fate remains an error")?;
    assert_eq!(error.code, 1);
    assert_eq!(cancel.exit_code(error.code), 130);
    assert!(client.is_closed());
    assert_eq!(factory.acquired.load(Ordering::SeqCst), 1);
    assert_eq!(keys.unlocks.load(Ordering::SeqCst), 1);
    assert_eq!(
        *factory.released.lock().map_err(|_| "release")?,
        vec![false]
    );
    let raw = hooks.raw.lock().map_err(|_| "raw")?;
    assert_eq!(raw.len(), 1);
    let receipt = &raw[0];
    let hash = receipt.fate_unknown.ok_or("unknown-fate identity")?;
    assert!(!receipt.success);
    let bytes = chain.mutate_state(|state| state.submissions.clone());
    assert_eq!(bytes.len(), 1);
    assert_eq!(
        chain
            .requests()
            .iter()
            .filter(|request| request.method.starts_with("author_submit"))
            .count(),
        1
    );
    assert_eq!(
        rtao::core::ops::extrinsic_lookup::find(
            &json!({"block":{"extrinsics":[format!("0x{}", hex::encode(&bytes[0]))]}}),
            hash.0
        )?,
        Some(0)
    );
    assert_eq!(
        serde_json::from_str::<Value>(&sink.text(Stream::Stdout)?)?,
        receipt.to_json()
    );
    let recovery = format!("{} {:?}", sink.text(Stream::Stderr)?, error.help);
    assert!(recovery.contains(&hex::encode(hash.0)));
    assert!(recovery.contains("do not resubmit until its outcome is known"));
    assert!(!recovery.contains("check the endpoint"));
    Ok(())
}

#[tokio::test]
async fn interrupted_success_keeps_value_and_drains_cosmetic_names_without_replay() -> TestResult {
    let (client, chain, _) = fixture(1_000_000_000).await?;
    let (ctx, sink, _) = context(&[])?;
    let cancel = rtao_cli::prompt::TerminalCancel::default();
    let ctx = ctx.with_interruption(cancel.clone());
    let pending_names = Arc::new(tokio::sync::Notify::new());
    chain.mutate_state(|state| {
        let pending_names = pending_names.clone();
        state.set_handler("state_getKeysPaged", move |_, _| {
            pending_names.notify_one();
            rtao_testkit::Reply::silent()
        });
    });
    let connector = Connected(client.clone());
    let operation = ctx.run_with(&connector, |connected| async move {
        connected.closed().await;
        Ok(77)
    });
    let interrupt = async {
        tokio::time::timeout(std::time::Duration::from_secs(2), pending_names.notified()).await?;
        cancel.cancel();
        Ok::<(), Box<dyn std::error::Error>>(())
    };
    let (result, interrupted) = tokio::time::timeout(std::time::Duration::from_secs(3), async {
        tokio::join!(operation, interrupt)
    })
    .await?;
    interrupted?;
    assert_eq!(result?, 77);
    assert!(client.is_closed());
    assert_eq!(cancel.exit_code(0), 130);
    let mut history = rtao_cli::prompt::PromptHistory::default();
    history.record_answers(vec!["--amount".into(), "1".into()]);
    ctx.finish_prompts(
        &mut history,
        &["rtao".into(), "tx".into(), "transfer".into()],
        true,
    )?;
    assert!(sink.text(Stream::Stdout)?.is_empty());
    assert!(sink.text(Stream::Stderr)?.is_empty());
    Ok(())
}
