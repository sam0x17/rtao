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

    for json_mode in [false, true] {
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
        let mut flags = vec!["--yes", "--no-mev-shield", "--quiet"];
        if json_mode {
            flags.push("--json");
        }
        let (ctx, sink, reads) = context(&flags)?;
        let receipt = tokio::time::timeout(
            std::time::Duration::from_secs(10),
            ctx.submit_with(&client, &intent, &keys, false, None),
        )
        .await??
        .ok_or("registration receipt")?;
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
            assert!(!stdout.contains("Transaction"));
        }
        assert!(sink.text(Stream::Stderr)?.is_empty());
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

#[test]
fn confirmed_wallet_and_hotkey_cache_the_selected_names_by_account() -> TestResult {
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
    let factory = factory(&keys);
    let cache = CallCache::default();
    let (ctx, sink) = saved_context(&keys, &["--yes", "--json", "--no-mev-shield"], 3)?;
    let result = ctx
        .submit_saved_with_factory(
            &client,
            &Preview { blocked: false },
            false,
            None,
            &factory,
            &cache,
        )
        .await?
        .ok_or("receipt")?;
    assert!(result.success);
    assert_eq!(factory.acquired.load(Ordering::SeqCst), 1);
    assert_eq!(*factory.releases.lock().map_err(|_| "release")?, vec![true]);
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
    assert!(error.help.is_none());
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
            return Err(IntentError::Validation(
                "direct primary execution sentinel".into(),
            ));
        }
        Preview { blocked: false }.preflight(cx).await
    }
}

#[tokio::test]
async fn direct_external_primary_execution_error_wins_over_release_failure() -> TestResult {
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
struct CommandFactsFixture;
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
    let intent = CommandFactsFixture;
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
    let intent = CommandFactsFixture;
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
    assert!(card.contains("Transaction"));
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
        let intent = CommandFactsFixture;
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
