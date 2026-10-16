//! Public discovery, reviewed identity and lazy native factory lifecycle fixtures.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::{
    collections::VecDeque,
    path::Path,
    sync::{Arc, Mutex},
    task::Poll,
};

use rtao::{
    client::{Client, ClientConfig, Network, View},
    primitives::{AccountId, CryptoType},
    signer::{
        BoxFuture, Coldkey, KeypairSigner, MultiSignature, Signer, SignerError, SignerRole,
        SigningRequest,
    },
    tx::{
        BuildCx, Built, Intent, IntentError, IntentInterface, IntentPreflight, PreflightCx, Signed,
    },
};
use rtao_cli::{
    application::command_tree,
    books::Books,
    call::{RawSignerFactory, SigningSelection},
    context::{AppContext, ExternalSignerFactory, SubmissionHooks, SubmissionOptions},
    error::CliError,
    globals::{Globals, Tier, with_globals},
    output::{MemorySink, Output, OutputOptions, Stream},
    prompt::{PromptError, PromptHistory, Terminal},
    signer_backends::{BackendRequest, BackendSession, NativeSignerFactory, SigningBackend},
};
use rtao_testkit::{FakeChain, FakeState, rpc::RpcClient};
use serde_json::{Value, json};

type Result<T = ()> = std::result::Result<T, Box<dyn std::error::Error>>;
type Trace = Arc<Mutex<Vec<&'static str>>>;

struct TestTerminal {
    tty: bool,
    lines: VecDeque<String>,
    trace: Trace,
}
impl Terminal for TestTerminal {
    fn stdin_is_tty(&self) -> bool {
        self.tty
    }
    fn stderr_is_tty(&self) -> bool {
        self.tty
    }
    fn read_line(&mut self, _: &str) -> std::result::Result<String, PromptError> {
        self.trace.lock().expect("trace").push("confirm");
        self.lines.pop_front().ok_or(PromptError::Aborted)
    }
    fn read_secret(
        &mut self,
        _: &str,
    ) -> std::result::Result<rtao::wallet::SecretString, PromptError> {
        self.trace.lock().expect("trace").push("wallet-secret");
        Err(PromptError::Unavailable)
    }
    fn message(&mut self, _: &str) -> std::result::Result<(), PromptError> {
        Ok(())
    }
}
fn context(flags: &[&str], trace: &Trace) -> Result<(AppContext, MemorySink)> {
    let root = command_tree([with_globals(clap::Command::new("submit"), Tier::Tx)])
        .try_get_matches_from(
            std::iter::once("rtao")
                .chain(std::iter::once("submit"))
                .chain(flags.iter().copied()),
        )?;
    let mut globals = Globals::resolve(
        &root,
        &serde_json::Map::new(),
        Path::new("/isolated/m7/home"),
        |_| None,
    )?;
    globals.apply(root.subcommand().ok_or("submit")?.1, &[])?;
    let mut context = AppContext::new(
        globals,
        serde_json::Map::new(),
        Books::default(),
        Arc::new(Mutex::new(Box::new(TestTerminal {
            tty: true,
            lines: VecDeque::from(["yes".into()]),
            trace: trace.clone(),
        }))),
    );
    let sink = MemorySink::default();
    context.output = Output::new(
        OutputOptions {
            json_mode: context.globals.json,
            quiet: context.globals.quiet,
            ..Default::default()
        },
        Arc::new(sink.clone()),
    );
    Ok((context, sink))
}
fn key(seed: u8) -> Result<KeypairSigner> {
    Ok(KeypairSigner::from_seed(&[seed; 32], CryptoType::Ed25519)?)
}

#[derive(Clone)]
struct TracedSigner {
    key: KeypairSigner,
    trace: Trace,
}
impl Signer for TracedSigner {
    fn account(&self) -> AccountId {
        self.key.account()
    }
    fn crypto_type(&self) -> CryptoType {
        self.key.crypto_type()
    }
    fn sign<'a>(
        &'a self,
        request: &'a SigningRequest<'a>,
    ) -> BoxFuture<'a, std::result::Result<MultiSignature, SignerError>> {
        Box::pin(async move {
            self.trace.lock().expect("trace").push("sign");
            self.key.sign(request).await
        })
    }
}
struct TestSession {
    signer: Arc<TracedSigner>,
    trace: Trace,
    block_release: bool,
}
impl BackendSession for TestSession {
    fn signer(&self) -> Arc<dyn Signer> {
        self.signer.clone()
    }
    fn release(&self, success: bool) -> BoxFuture<'_, std::result::Result<(), CliError>> {
        Box::pin(async move {
            self.trace.lock().expect("trace").push(if success {
                "release-ok"
            } else {
                "release-failed"
            });
            if self.block_release {
                std::future::pending::<()>().await;
            }
            Ok(())
        })
    }
}
impl Drop for TestSession {
    fn drop(&mut self) {
        self.trace.lock().expect("trace").push("dispose");
    }
}
struct TestBackend {
    trace: Trace,
    discovered: AccountId,
    signer: KeypairSigner,
    block_acquire: bool,
    block_release: bool,
    requests: Mutex<Vec<BackendRequest>>,
}
impl TestBackend {
    fn new(trace: &Trace, seed: u8) -> Result<Self> {
        let signer = key(seed)?;
        Ok(Self {
            trace: trace.clone(),
            discovered: signer.account(),
            signer,
            block_acquire: false,
            block_release: false,
            requests: Mutex::new(Vec::new()),
        })
    }
}
impl SigningBackend for TestBackend {
    fn discover<'a>(
        &'a self,
        request: &'a BackendRequest,
    ) -> BoxFuture<'a, std::result::Result<AccountId, CliError>> {
        Box::pin(async move {
            self.trace.lock().expect("trace").push("discover-public");
            self.requests
                .lock()
                .expect("requests")
                .push(request.clone());
            Ok(self.discovered)
        })
    }
    fn acquire<'a>(
        &'a self,
        request: &'a BackendRequest,
        reviewed: AccountId,
    ) -> BoxFuture<'a, std::result::Result<Arc<dyn BackendSession>, CliError>> {
        Box::pin(async move {
            assert_eq!(request.expected, Some(reviewed));
            self.trace.lock().expect("trace").push("acquire");
            self.requests
                .lock()
                .expect("requests")
                .push(request.clone());
            let session: Arc<dyn BackendSession> = Arc::new(TestSession {
                signer: Arc::new(TracedSigner {
                    key: self.signer.clone(),
                    trace: self.trace.clone(),
                }),
                trace: self.trace.clone(),
                block_release: self.block_release,
            });
            if self.block_acquire {
                std::future::pending::<()>().await;
            }
            Ok(session)
        })
    }
}
fn phases(trace: &Trace) -> Vec<&'static str> {
    trace.lock().expect("trace").clone()
}

async fn chain_fixture() -> Result<(Client, FakeChain)> {
    let mut state = FakeState::default();
    for seed in [19, 20] {
        state.seed_python(
            "System",
            "Account",
            &[json!(key(seed)?.account())],
            json!({"data":{"free":1_000_000_000_000u64}}),
        )?;
    }
    let chain = FakeChain::from_state(state);
    let mut config = ClientConfig::new(Network::resolve_with("local", |_| None)?)?;
    config.track_heads = false;
    config.runtime_cache_dir = None;
    config.config_store = None;
    Ok((
        Client::from_rpc(RpcClient::new(chain.clone()), config).await?,
        chain,
    ))
}
struct Remark {
    blocked: bool,
    trace: Trace,
}
impl IntentInterface for Remark {
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
impl Intent for Remark {
    fn summary(&self) -> String {
        "M7 reviewed remark".into()
    }
    async fn build<V: View + ?Sized>(
        &self,
        cx: &BuildCx<'_, V>,
    ) -> std::result::Result<Built, IntentError> {
        Ok(Built::new(
            rtao_runtime::calls::system::remark(vec![7]).compose(cx.runtime)?,
        ))
    }
    async fn preflight<V: View + ?Sized>(
        &self,
        _: &PreflightCx<'_, V>,
    ) -> std::result::Result<IntentPreflight, IntentError> {
        self.trace.lock().expect("trace").push("preflight");
        Ok(IntentPreflight {
            blocks: if self.blocked {
                vec!["M7 preflight refuses".into()]
            } else {
                vec![]
            },
            ..Default::default()
        })
    }
}
struct Hooks {
    trace: Trace,
    fail: bool,
}
impl SubmissionHooks for Hooks {
    fn before_submit(&self) -> BoxFuture<'_, std::result::Result<(), CliError>> {
        Box::pin(async move {
            self.trace.lock().expect("trace").push("durable-plan");
            if self.fail {
                Err(CliError::failure("M7 durable storage refuses"))
            } else {
                Ok(())
            }
        })
    }
    fn on_result<'a>(
        &'a self,
        _: &'a rtao::tx::ExtrinsicResult,
    ) -> BoxFuture<'a, std::result::Result<(), CliError>> {
        Box::pin(async move {
            self.trace.lock().expect("trace").push("durable-receipt");
            Ok(())
        })
    }
}

// The real context/executor uses the native factory's injected boundary;
// these regression fixtures make no device or cross-language parity claim.
#[tokio::test]
async fn pinned_previews_and_preflight_refusal_never_acquire_or_discover() -> Result {
    let (client, chain) = chain_fixture().await?;
    let account = key(19)?.account().to_string();
    for backend_name in ["ledger", "vault", "extension"] {
        for (dry_run, blocked) in [(true, false), (false, true)] {
            let trace = Trace::default();
            let mut flags = vec![
                "--json",
                "--yes",
                "--no-mev-shield",
                "--signer",
                backend_name,
                "--signer-address",
                &account,
            ];
            if dry_run {
                flags.push("--dry-run");
            }
            let (mut ctx, _) = context(&flags, &trace)?;
            let backend = Arc::new(TestBackend::new(&trace, 19)?);
            let factory = NativeSignerFactory::with_backend(&ctx, backend);
            let outcome = ctx
                .submit(
                    &client,
                    &Remark {
                        blocked,
                        trace: trace.clone(),
                    },
                    false,
                    None,
                    Some(&factory),
                    &mut PromptHistory::default(),
                )
                .await;
            assert_eq!(outcome.is_err(), blocked);
            assert_eq!(phases(&trace), vec!["preflight"]);
        }
    }
    assert!(chain.state().lock().expect("chain").submissions.is_empty());
    client.close().await;
    Ok(())
}

#[tokio::test]
async fn confirmation_and_durable_plan_precede_acquisition_receipt_precedes_release() -> Result {
    for backend_name in ["ledger", "extension"] {
        let (client, chain) = chain_fixture().await?;
        let trace = Trace::default();
        let account = key(19)?.account().to_string();
        let (mut ctx, _) = context(
            &[
                "--no-mev-shield",
                "--signer",
                backend_name,
                "--signer-address",
                &account,
            ],
            &trace,
        )?;
        let backend = Arc::new(TestBackend::new(&trace, 19)?);
        let factory = NativeSignerFactory::with_backend(&ctx, backend.clone());
        let hooks = Hooks {
            trace: trace.clone(),
            fail: false,
        };
        let outcome = ctx
            .submit_with_options(
                &client,
                &Remark {
                    blocked: false,
                    trace: trace.clone(),
                },
                SubmissionOptions {
                    hooks: Some(&hooks),
                    ..Default::default()
                },
                Some(&factory),
                &mut PromptHistory::default(),
            )
            .await?;
        assert!(outcome.is_some_and(|result| result.success));
        let phases = phases(&trace);
        let position = |event| {
            phases
                .iter()
                .position(|phase| *phase == event)
                .expect("phase")
        };
        assert!(position("preflight") < position("confirm"));
        assert!(position("confirm") < position("durable-plan"));
        assert!(position("durable-plan") < position("acquire"));
        assert!(position("acquire") < position("sign"));
        assert!(position("sign") < position("durable-receipt"));
        assert!(position("durable-receipt") < position("release-ok"));
        assert_eq!(
            phases.iter().filter(|phase| **phase == "dispose").count(),
            1
        );
        assert_eq!(chain.state().lock().expect("chain").submissions.len(), 1);
        {
            let requests = backend.requests.lock().expect("requests");
            assert_eq!(requests.len(), 1);
            assert_eq!(requests[0].summary.as_deref(), Some("M7 reviewed remark"));
        }
        client.close().await;
    }
    Ok(())
}

#[tokio::test]
async fn vault_display_summary_is_scoped_to_each_reviewed_source() -> Result {
    let trace = Trace::default();
    let account = key(19)?.account().to_string();
    let (ctx, _) = context(&["--signer", "vault", "--signer-address", &account], &trace)?;
    let backend = Arc::new(TestBackend::new(&trace, 19)?);
    let factory = NativeSignerFactory::with_backend(&ctx, backend.clone());
    let first = factory.keys(&ctx.globals, SignerRole::Coldkey).await?;
    let second = factory.keys(&ctx.globals, SignerRole::Coldkey).await?;
    ExternalSignerFactory::set_summary(&factory, first.as_ref(), "first reviewed transaction");
    ExternalSignerFactory::set_summary(&factory, second.as_ref(), "second reviewed transaction");
    assert!(phases(&trace).is_empty());
    // Reverse acquisition exposes a factory-global label being reused.
    second.signer_for(SignerRole::Coldkey).await?;
    first.signer_for(SignerRole::Coldkey).await?;
    {
        let requests = backend.requests.lock().expect("requests");
        assert_eq!(requests.len(), 2);
        assert_eq!(
            requests[0].summary.as_deref(),
            Some("second reviewed transaction")
        );
        assert_eq!(
            requests[1].summary.as_deref(),
            Some("first reviewed transaction")
        );
    }
    ExternalSignerFactory::release(&factory, &ctx.globals, first.as_ref(), false).await?;
    ExternalSignerFactory::release(&factory, &ctx.globals, second.as_ref(), false).await?;
    Ok(())
}

#[tokio::test]
async fn raw_vault_submission_forwards_the_reviewed_wrapper_label() -> Result {
    struct NoCache;
    impl rtao_cli::call::RawCallCache for NoCache {
        fn save(&self, _: String, _: Value) -> BoxFuture<'_, std::result::Result<(), CliError>> {
            Box::pin(async {
                Err(CliError::failure(
                    "single raw submission must not save a multisig spec",
                ))
            })
        }
    }
    let (client, chain) = chain_fixture().await?;
    let trace = Trace::default();
    let account = key(19)?.account().to_string();
    let root = command_tree([rtao_cli::call::command()]).try_get_matches_from([
        "rtao",
        "call",
        "System.remark",
        "--args",
        "{\"remark\":\"0x07\"}",
        "--sudo",
        "--no-mev-shield",
        "--signer",
        "vault",
        "--signer-address",
        &account,
    ])?;
    let leaf = root.subcommand().ok_or("raw call")?.1;
    let (mut ctx, _) = context(&["--signer", "vault", "--signer-address", &account], &trace)?;
    ctx.globals = Globals::resolve(
        &root,
        &Default::default(),
        Path::new("/isolated/m7/home"),
        |_| None,
    )?;
    ctx.globals.apply(leaf, &["signer_backend"])?;
    let request = rtao_cli::call::RawCallRequest::from_matches(&ctx, leaf)?;
    let backend = Arc::new(TestBackend::new(&trace, 19)?);
    let factory = NativeSignerFactory::with_backend(&ctx, backend.clone());
    let result = rtao_cli::call::execute_rounds_with(
        &ctx,
        &client,
        &request,
        "raw replay",
        &factory,
        &NoCache,
    )
    .await?;
    assert!(result.is_some_and(|result| result.success));
    {
        let requests = backend.requests.lock().expect("requests");
        assert_eq!(requests.len(), 1);
        assert_eq!(
            requests[0].summary.as_deref(),
            Some("System.remark via Sudo.sudo")
        );
    }
    assert_eq!(chain.state().lock().expect("chain").submissions.len(), 1);
    assert!(!phases(&trace).contains(&"discover-public"));
    client.close().await;
    Ok(())
}

#[tokio::test]
async fn durable_storage_failure_and_confirmation_refusal_leave_backend_untouched() -> Result {
    let (client, chain) = chain_fixture().await?;
    let account = key(19)?.account().to_string();
    for fail_durable in [false, true] {
        let trace = Trace::default();
        let mut flags = vec![
            "--json",
            "--no-mev-shield",
            "--signer",
            "extension",
            "--signer-address",
            &account,
        ];
        if fail_durable {
            flags.push("--yes");
        }
        let (mut ctx, _) = context(&flags, &trace)?;
        let factory =
            NativeSignerFactory::with_backend(&ctx, Arc::new(TestBackend::new(&trace, 19)?));
        let hooks = Hooks {
            trace: trace.clone(),
            fail: fail_durable,
        };
        let error = ctx
            .submit_with_options(
                &client,
                &Remark {
                    blocked: false,
                    trace: trace.clone(),
                },
                SubmissionOptions {
                    hooks: Some(&hooks),
                    ..Default::default()
                },
                Some(&factory),
                &mut PromptHistory::default(),
            )
            .await
            .err()
            .ok_or("expected refusal")?;
        assert!(error.message.contains(if fail_durable {
            "durable storage"
        } else {
            "confirmation"
        }));
        assert!(!phases(&trace).contains(&"acquire"));
        assert!(!phases(&trace).contains(&"discover-public"));
    }
    assert!(chain.state().lock().expect("chain").submissions.is_empty());
    client.close().await;
    Ok(())
}

#[tokio::test]
async fn unpinned_public_discovery_is_distinct_from_signer_acquisition() -> Result {
    for backend_name in ["ledger", "extension"] {
        let trace = Trace::default();
        let (ctx, _) = context(&["--signer", backend_name], &trace)?;
        let backend = Arc::new(TestBackend::new(&trace, 19)?);
        let account = backend.discovered;
        let factory = NativeSignerFactory::with_backend(&ctx, backend);
        let keys = factory.keys(&ctx.globals, SignerRole::Hotkey).await?;
        assert_eq!(keys.bare_signer_account(), Some(account));
        assert!(keys.coldkey_address().is_err());
        assert_eq!(phases(&trace), vec!["discover-public"]);
        assert_eq!(
            keys.signer_for(SignerRole::Hotkey).await?.account(),
            account
        );
        assert_eq!(phases(&trace), vec!["discover-public", "acquire"]);
        ExternalSignerFactory::release(&factory, &ctx.globals, keys.as_ref(), false).await?;
        assert_eq!(
            phases(&trace),
            vec!["discover-public", "acquire", "release-failed", "dispose"]
        );
    }
    Ok(())
}

#[tokio::test]
async fn wrong_device_account_is_closed_without_signing_and_role_is_enforced() -> Result {
    let trace = Trace::default();
    let account = key(19)?.account().to_string();
    let (ctx, _) = context(
        &[
            "--signer",
            "ledger",
            "--signer-address",
            &account,
            "--ledger-account",
            "3",
            "--ledger-index",
            "8",
        ],
        &trace,
    )?;
    let backend = Arc::new(TestBackend::new(&trace, 20)?);
    let factory = NativeSignerFactory::with_backend(&ctx, backend.clone());
    let keys = factory.keys(&ctx.globals, SignerRole::Hotkey).await?;
    assert!(matches!(
        keys.signer_for(SignerRole::Coldkey).await,
        Err(SignerError::RoleUnavailable(SignerRole::Coldkey))
    ));
    assert!(phases(&trace).is_empty());
    let error = keys
        .signer_for(SignerRole::Hotkey)
        .await
        .err()
        .ok_or("account mismatch")?;
    assert!(error.to_string().contains("reviewed account"));
    assert_eq!(phases(&trace), vec!["acquire", "release-failed", "dispose"]);
    let requests = backend.requests.lock().expect("requests");
    assert_eq!(requests[0].globals.ledger_account, 3);
    assert_eq!(requests[0].globals.ledger_index, 8);
    Ok(())
}

#[tokio::test]
async fn cancelled_acquisition_and_release_dispose_owned_session() -> Result {
    for cancel_release in [false, true] {
        let trace = Trace::default();
        let account = key(19)?.account().to_string();
        let (ctx, _) = context(&["--signer", "vault", "--signer-address", &account], &trace)?;
        let mut backend = TestBackend::new(&trace, 19)?;
        backend.block_acquire = !cancel_release;
        backend.block_release = cancel_release;
        let factory = NativeSignerFactory::with_backend(&ctx, Arc::new(backend));
        let keys = factory.keys(&ctx.globals, SignerRole::Coldkey).await?;
        if cancel_release {
            drop(keys.signer_for(SignerRole::Coldkey).await?);
            let mut future =
                ExternalSignerFactory::release(&factory, &ctx.globals, keys.as_ref(), false);
            assert!(matches!(futures::poll!(&mut future), Poll::Pending));
            drop(future);
            assert_eq!(phases(&trace), vec!["acquire", "release-failed", "dispose"]);
            assert!(keys.signer_for(SignerRole::Coldkey).await.is_err());
        } else {
            let mut future = keys.signer_for(SignerRole::Coldkey);
            assert!(matches!(futures::poll!(&mut future), Poll::Pending));
            drop(future);
            assert_eq!(phases(&trace), vec!["acquire", "dispose"]);
        }
    }
    Ok(())
}

#[tokio::test]
async fn dropping_source_disposes_session_even_while_factory_lives() -> Result {
    let trace = Trace::default();
    let account = key(19)?.account().to_string();
    let (ctx, _) = context(
        &["--signer", "extension", "--signer-address", &account],
        &trace,
    )?;
    let factory = NativeSignerFactory::with_backend(&ctx, Arc::new(TestBackend::new(&trace, 19)?));
    let keys = factory.keys(&ctx.globals, SignerRole::Coldkey).await?;
    drop(keys.signer_for(SignerRole::Coldkey).await?);
    drop(keys);
    assert_eq!(phases(&trace), vec!["acquire", "dispose"]);
    Ok(())
}

#[tokio::test]
async fn raw_member_review_and_saved_round_source_bind_same_discovered_account() -> Result {
    let trace = Trace::default();
    let (ctx, _) = context(&["--signer", "extension"], &trace)?;
    let backend = Arc::new(TestBackend::new(&trace, 19)?);
    let account = backend.discovered;
    let factory = NativeSignerFactory::with_backend(&ctx, backend);
    let selected = SigningSelection {
        display_name: None,
        wallet: "public-external-member".into(),
        backend: "extension".into(),
        address: None,
    };
    let reviewed = factory.public_selection(&ctx, &selected).await?;
    assert_eq!(reviewed.address, Some(account));
    assert_eq!(phases(&trace), vec!["discover-public"]);
    let keys = factory.source(&ctx, &reviewed).await?;
    assert_eq!(phases(&trace), vec!["discover-public"]);
    assert_eq!(
        keys.signer_for(SignerRole::Coldkey).await?.account(),
        account
    );
    RawSignerFactory::release(&factory, &reviewed, keys.as_ref(), true).await?;
    assert_eq!(
        phases(&trace),
        vec!["discover-public", "acquire", "release-ok", "dispose"]
    );
    Ok(())
}

#[tokio::test]
async fn missing_vault_identity_and_invalid_explicit_identity_never_fall_back_to_wallet() -> Result
{
    for (backend_name, explicit) in [
        ("vault", None),
        ("extension", Some("not-a-known-account")),
        ("ledger", Some("not-a-known-account")),
    ] {
        let trace = Trace::default();
        let mut flags = vec!["--signer", backend_name];
        if let Some(address) = explicit {
            flags.extend(["--signer-address", address]);
        }
        let (ctx, _) = context(&flags, &trace)?;
        let factory =
            NativeSignerFactory::with_backend(&ctx, Arc::new(TestBackend::new(&trace, 19)?));
        assert!(
            factory
                .keys(&ctx.globals, SignerRole::Coldkey)
                .await
                .is_err()
        );
        assert!(phases(&trace).is_empty());
    }
    Ok(())
}

#[tokio::test]
async fn quiet_json_and_terminal_policy_is_forwarded_without_opening_ui() -> Result {
    let trace = Trace::default();
    let account = key(19)?.account().to_string();
    let (ctx, sink) = context(
        &[
            "--json",
            "--quiet",
            "--signer",
            "extension",
            "--signer-address",
            &account,
            "--extension-source",
            "test-source",
        ],
        &trace,
    )?;
    let backend = Arc::new(TestBackend::new(&trace, 19)?);
    let factory = NativeSignerFactory::with_backend(&ctx, backend.clone());
    let keys = factory.keys(&ctx.globals, SignerRole::Coldkey).await?;
    assert!(phases(&trace).is_empty());
    drop(keys.signer_for(SignerRole::Coldkey).await?);
    let requests = backend.requests.lock().expect("requests");
    assert!(!requests[0].open_browser);
    assert!(!requests[0].interactive);
    assert_eq!(
        requests[0].globals.extension_source.as_deref(),
        Some("test-source")
    );
    assert!(sink.text(Stream::Stdout)?.is_empty());
    assert!(sink.text(Stream::Stderr)?.is_empty());
    Ok(())
}

#[tokio::test]
async fn unpinned_semantic_preview_refuses_before_device_or_browser_discovery() -> Result {
    let (client, chain) = chain_fixture().await?;
    for backend_name in ["ledger", "extension"] {
        let trace = Trace::default();
        let (mut ctx, _) = context(&["--dry-run", "--signer", backend_name], &trace)?;
        let factory =
            NativeSignerFactory::with_backend(&ctx, Arc::new(TestBackend::new(&trace, 19)?));
        let error = ctx
            .submit(
                &client,
                &Remark {
                    blocked: false,
                    trace: trace.clone(),
                },
                false,
                None,
                Some(&factory),
                &mut PromptHistory::default(),
            )
            .await
            .err()
            .ok_or("public identity required")?;
        assert!(error.message.contains("public account address"));
        assert!(phases(&trace).is_empty());
    }
    assert!(chain.state().lock().expect("chain").submissions.is_empty());
    client.close().await;
    Ok(())
}

#[tokio::test]
async fn saved_extension_account_is_a_filtered_preference_before_review() -> Result {
    let trace = Trace::default();
    let saved = key(20)?.account();
    let (mut ctx, _) = context(
        &[
            "--signer",
            "extension",
            "--extension-source",
            "selected-source",
        ],
        &trace,
    )?;
    ctx.config.insert("signer_address".into(), json!(saved));
    let backend = Arc::new(TestBackend::new(&trace, 19)?);
    let discovered = backend.discovered;
    let factory = NativeSignerFactory::with_backend(&ctx, backend.clone());
    let keys = factory.keys(&ctx.globals, SignerRole::Coldkey).await?;
    assert_eq!(keys.bare_signer_account(), Some(discovered));
    let requests = backend.requests.lock().expect("requests");
    assert_eq!(requests[0].expected, None);
    assert_eq!(requests[0].default_address, Some(saved));
    assert_eq!(
        requests[0].globals.extension_source.as_deref(),
        Some("selected-source")
    );
    assert_eq!(phases(&trace), vec!["discover-public"]);
    Ok(())
}

#[tokio::test]
async fn raw_unresolved_explicit_account_cannot_be_erased_by_public_selection() -> Result {
    for backend_name in ["ledger", "extension"] {
        let trace = Trace::default();
        let (ctx, _) = context(
            &[
                "--signer",
                backend_name,
                "--signer-address",
                "not-a-known-account",
            ],
            &trace,
        )?;
        let factory =
            NativeSignerFactory::with_backend(&ctx, Arc::new(TestBackend::new(&trace, 19)?));
        let selection = SigningSelection {
            display_name: None,
            wallet: ctx.globals.wallet_name.clone(),
            backend: backend_name.into(),
            address: None,
        };
        assert!(factory.public_selection(&ctx, &selection).await.is_err());
        assert!(phases(&trace).is_empty());
    }
    Ok(())
}

#[tokio::test]
async fn real_raw_parser_keeps_saved_extension_address_as_preference() -> Result {
    let trace = Trace::default();
    let root = command_tree([rtao_cli::call::command()]).try_get_matches_from([
        "rtao",
        "call",
        "System.remark",
        "--args",
        "{\"remark\":\"0x07\"}",
        "--signer",
        "extension",
        "--extension-source",
        "selected-source",
    ])?;
    let config = serde_json::Map::from_iter([("signer_address".into(), json!(key(20)?.account()))]);
    let mut globals = Globals::resolve(&root, &config, Path::new("/isolated/m7/home"), |_| None)?;
    let leaf = root.subcommand().ok_or("raw call")?.1;
    globals.apply(leaf, &["signer_backend"])?;
    let ctx = AppContext::new(
        globals,
        config,
        Books::default(),
        Arc::new(Mutex::new(Box::new(TestTerminal {
            tty: true,
            lines: VecDeque::new(),
            trace: trace.clone(),
        }))),
    );
    let request = rtao_cli::call::RawCallRequest::from_matches(&ctx, leaf)?;
    assert_eq!(request.rounds[0].address, None);
    let backend = Arc::new(TestBackend::new(&trace, 19)?);
    let discovered = backend.discovered;
    let factory = NativeSignerFactory::with_backend(&ctx, backend.clone());
    let selection = factory.public_selection(&ctx, &request.rounds[0]).await?;
    assert_eq!(selection.address, Some(discovered));
    let requests = backend.requests.lock().expect("requests");
    assert_eq!(requests[0].expected, None);
    assert_eq!(requests[0].default_address, Some(key(20)?.account()));
    assert_eq!(
        requests[0].globals.extension_source.as_deref(),
        Some("selected-source")
    );
    assert_eq!(phases(&trace), vec!["discover-public"]);
    Ok(())
}

#[tokio::test]
async fn unavailable_native_features_refuse_without_a_wallet_fallback() -> Result {
    let account = key(19)?.account().to_string();
    for (name, enabled) in [
        ("ledger", cfg!(feature = "ledger")),
        ("vault", cfg!(feature = "vault")),
        ("extension", cfg!(feature = "extension")),
    ] {
        if enabled {
            continue;
        }
        let trace = Trace::default();
        let (ctx, _) = context(&["--signer", name, "--signer-address", &account], &trace)?;
        let error = ctx
            .signing_keys(SignerRole::Coldkey, None)
            .await
            .err()
            .ok_or("missing native feature")?;
        assert!(error.message.contains("corresponding CLI feature"));
        assert!(phases(&trace).is_empty());
    }
    Ok(())
}

#[test]
fn invalid_raw_preview_address_refuses_during_public_argument_resolution() -> Result {
    for name in ["ledger", "extension"] {
        let trace = Trace::default();
        let root = command_tree([rtao_cli::call::command()]).try_get_matches_from([
            "rtao",
            "call",
            "System.remark",
            "--args",
            "{\"remark\":\"0x07\"}",
            "--dry-run",
            "--signer",
            name,
            "--signer-address",
            "not-a-known-account",
        ])?;
        let mut globals = Globals::resolve(
            &root,
            &Default::default(),
            Path::new("/isolated/m7/home"),
            |_| None,
        )?;
        let leaf = root.subcommand().ok_or("raw call")?.1;
        globals.apply(leaf, &["signer_backend"])?;
        let ctx = AppContext::new(
            globals,
            Default::default(),
            Books::default(),
            Arc::new(Mutex::new(Box::new(TestTerminal {
                tty: true,
                lines: VecDeque::new(),
                trace: trace.clone(),
            }))),
        );
        let error = rtao_cli::call::RawCallRequest::from_matches(&ctx, leaf)
            .err()
            .ok_or("invalid account")?;
        assert!(error.message.contains("--signer-address"));
        assert!(phases(&trace).is_empty());
    }
    Ok(())
}
