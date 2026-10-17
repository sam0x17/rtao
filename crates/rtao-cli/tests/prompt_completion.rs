//! Complete parser/command/finish boundaries for public prompt guidance.

use rtao::{
    client::{Client, ClientConfig, Network},
    primitives::NetUid,
};
use rtao_cli::{
    application,
    books::Books,
    commands::{misc, subnets},
    context::{AppContext, SubmissionOptions},
    globals::Globals,
    output::{DiagnosticContext, MemorySink, Output, OutputOptions, Stream},
    prompt::{PromptContext, PromptError, PromptHistory, Terminal, prompt_password},
};
use rtao_testkit::{FakeChain, FakeState, rpc::RpcClient};
use serde_json::json;
use std::{
    collections::VecDeque,
    path::Path,
    sync::{Arc, Mutex},
};

type TestResult = Result<(), Box<dyn std::error::Error>>;

#[derive(Default)]
struct Input {
    lines: VecDeque<String>,
    hidden: VecDeque<rtao::wallet::SecretString>,
}
impl Input {
    fn lines(lines: &[&str]) -> Self {
        Self {
            lines: lines.iter().map(|line| (*line).to_owned()).collect(),
            ..Default::default()
        }
    }
}
impl Terminal for Input {
    fn stdin_is_tty(&self) -> bool {
        true
    }
    fn stderr_is_tty(&self) -> bool {
        true
    }
    fn read_line(&mut self, _: &str) -> Result<String, PromptError> {
        self.lines.pop_front().ok_or(PromptError::Aborted)
    }
    fn read_secret(&mut self, _: &str) -> Result<rtao::wallet::SecretString, PromptError> {
        self.hidden.pop_front().ok_or(PromptError::Aborted)
    }
    fn message(&mut self, _: &str) -> Result<(), PromptError> {
        Ok(())
    }
}

fn context(
    matches: &clap::ArgMatches,
    reserved: &[&str],
) -> Result<(AppContext, MemorySink), Box<dyn std::error::Error>> {
    let mut leaf = matches;
    while let Some((_, child)) = leaf.subcommand() {
        leaf = child;
    }
    let mut globals = Globals::resolve(
        matches,
        &Default::default(),
        Path::new("/fixture/no-user-home"),
        |_| None,
    )?;
    globals.apply_with_env(leaf, reserved, |_| None)?;
    let mut ctx = AppContext::new(
        globals,
        Default::default(),
        Books::default(),
        Arc::new(Mutex::new(Box::new(Input::default()))),
    );
    let sink = MemorySink::default();
    ctx.output = Output::new(
        OutputOptions {
            json_mode: ctx.globals.json,
            quiet: ctx.globals.quiet,
            network: None,
        },
        Arc::new(sink.clone()),
    );
    Ok((ctx, sink))
}

#[tokio::test]
async fn successful_prompted_conversion_finishes_once_without_duplicate_or_hidden_answers()
-> TestResult {
    let original = ["/usr/bin/rtao", "misc", "convert"].map(String::from);
    let mut input = Input::lines(&["1.25"]);
    input.hidden.push_back(rtao::wallet::SecretString::new(
        "private-password-sentinel".into(),
    ));
    let mut invocation = application::parse_with_prompts(
        application::command_tree([misc::command()]),
        original.iter().map(Into::into).collect(),
        &PromptContext::default(),
        &mut input,
    )?;
    let _password = prompt_password(&PromptContext::default(), &mut input, "Unlock: ", false)?;
    assert_eq!(
        invocation.argv.last().and_then(|arg| arg.to_str()),
        Some("1.25")
    );
    let (ctx, sink) = context(&invocation.matches, &misc::reserved("convert"))?;
    let leaf = invocation
        .matches
        .subcommand()
        .and_then(|(_, group)| group.subcommand())
        .ok_or("convert leaf")?
        .1;
    misc::run(&ctx, "convert", leaf).await?;
    application::finish_prompts(&ctx.output, &mut invocation.history, &original, true)?;
    assert!(sink.text(Stream::Stdout)?.contains("1.25"));
    let hints = sink.text(Stream::Stderr)?;
    assert_eq!(
        hints,
        "skip the prompts next time: rtao misc convert 1.25\n"
    );
    assert!(!hints.contains("private-password-sentinel"));
    application::finish_prompts(&ctx.output, &mut invocation.history, &original, true)?;
    assert_eq!(sink.text(Stream::Stderr)?, hints);
    Ok(())
}

#[tokio::test]
async fn failed_conversion_keeps_its_diagnostic_and_has_no_success_hint() -> TestResult {
    let original = ["rtao", "misc", "convert"].map(String::from);
    let mut input = Input::lines(&["invalid"]);
    let mut invocation = application::parse_with_prompts(
        application::command_tree([misc::command()]),
        original.iter().map(Into::into).collect(),
        &PromptContext::default(),
        &mut input,
    )?;
    let (ctx, sink) = context(&invocation.matches, &misc::reserved("convert"))?;
    let leaf = invocation
        .matches
        .subcommand()
        .and_then(|(_, group)| group.subcommand())
        .ok_or("convert leaf")?
        .1;
    let result = misc::run(&ctx, "convert", leaf).await;
    let error = result.as_ref().err().ok_or("conversion must fail")?;
    assert_eq!(error.code, 2);
    ctx.output
        .error(&error.message, DiagnosticContext::default())?;
    application::finish_prompts(
        &ctx.output,
        &mut invocation.history,
        &original,
        result.is_ok(),
    )?;
    assert!(sink.text(Stream::Stdout)?.is_empty());
    assert!(
        sink.text(Stream::Stderr)?
            .contains("amount must be a finite number")
    );
    assert!(!sink.text(Stream::Stderr)?.contains("skip the prompts"));
    Ok(())
}

#[tokio::test]
async fn dry_conversion_consumes_guidance_and_json_quiet_finish_never_contaminates_streams()
-> TestResult {
    let original = ["rtao", "--dry-run", "misc", "convert"].map(String::from);
    let mut input = Input::lines(&["1.25"]);
    let mut invocation = application::parse_with_prompts(
        application::command_tree([misc::command()]),
        original.iter().map(Into::into).collect(),
        &PromptContext::default(),
        &mut input,
    )?;
    let (ctx, sink) = context(&invocation.matches, &misc::reserved("convert"))?;
    let leaf = invocation
        .matches
        .subcommand()
        .and_then(|(_, group)| group.subcommand())
        .ok_or("convert leaf")?
        .1;
    misc::run(&ctx, "convert", leaf).await?;
    application::finish_prompts(&ctx.output, &mut invocation.history, &original, true)?;
    assert!(sink.text(Stream::Stderr)?.is_empty());
    assert!(invocation.history.flush_hint(&["rtao".into()], 0).is_none());
    for (json_mode, quiet) in [(true, false), (false, true), (true, true)] {
        let sink = MemorySink::default();
        let output = Output::new(
            OutputOptions {
                json_mode,
                quiet,
                network: None,
            },
            Arc::new(sink.clone()),
        );
        let mut history = PromptHistory::default();
        history.record_answers(vec!["--netuid".into(), "7".into()]);
        output.value(&json!({"primary":7}))?;
        application::finish_prompts(&output, &mut history, &["rtao".into()], true)?;
        assert!(sink.text(Stream::Stdout)?.contains("7"));
        assert!(sink.text(Stream::Stderr)?.is_empty());
    }
    Ok(())
}

#[test]
fn finish_keeps_public_newlines_inside_escaped_hint_lines_and_omits_secret_flags() -> TestResult {
    let sink = MemorySink::default();
    let output = Output::new(OutputOptions::default(), Arc::new(sink.clone()));
    let original = [
        "rtao",
        "--network",
        "wss://rpc-user:rpc-password@private.invalid/secret-path?token=secret-query",
        "wallet",
        "regen-coldkey",
        "--mnemonic",
        "private-phrase",
        "--json-password=private-json",
    ]
    .map(String::from);
    let mut history = PromptHistory::default();
    history.record_answers(vec!["--wallet".into(), "public\nforged\x1b[2J".into()]);
    history.record_sdk_hint("native sdk expression\nforged".into());
    application::finish_prompts(&output, &mut history, &original, true)?;
    let hints = sink.text(Stream::Stderr)?;
    assert_eq!(hints.lines().count(), 2);
    assert!(hints.contains(r"public\nforged\x1b[2J"));
    assert!(hints.contains(r"native sdk expression\nforged"));
    assert!(!hints.contains("private-phrase") && !hints.contains("private-json"));
    assert!(hints.contains(
        r#"BT_NETWORK="${BT_NETWORK:?set BT_NETWORK to your configured RPC endpoint}" rtao wallet"#
    ));
    for secret in [
        "rpc-user",
        "rpc-password",
        "private.invalid",
        "secret-path",
        "secret-query",
    ] {
        assert!(!hints.contains(secret));
    }
    assert!(!hints.contains("bt.Subtensor"));
    assert!(sink.text(Stream::Stdout)?.is_empty());
    Ok(())
}

#[tokio::test]
async fn actual_prompted_price_caller_teaches_the_typed_native_read_without_author_calls()
-> TestResult {
    let mut state = FakeState::try_default()?;
    state.seed_runtime(
        "SwapRuntimeApi",
        "current_alpha_price",
        json!(1_500_000_000_u64),
    )?;
    let chain = FakeChain::from_state(state);
    let mut config = ClientConfig::new(Network::resolve_with("local", |_| None)?)?;
    config.track_heads = false;
    config.runtime_cache_dir = None;
    config.config_store = None;
    let client = Client::from_rpc(RpcClient::new(chain.clone()), config).await?;
    let original = ["rtao", "subnets", "price"].map(String::from);
    let mut input = Input::lines(&["7"]);
    let mut invocation = application::parse_with_prompts(
        application::command_tree([subnets::command()?]),
        original.iter().map(Into::into).collect(),
        &PromptContext::default(),
        &mut input,
    )?;
    let group = invocation.matches.subcommand().ok_or("subnets group")?.1;
    let (mut ctx, sink) = context(&invocation.matches, &subnets::reserved(group)?)?;
    let request = subnets::Request::from_matches(&mut ctx, group, &mut invocation.history)?;
    subnets::execute(
        &mut ctx,
        &client,
        &request,
        SubmissionOptions::default(),
        None,
        &mut invocation.history,
    )
    .await?;
    application::finish_prompts(&ctx.output, &mut invocation.history, &original, true)?;
    let expected = "rtao::reads::generated::interfaces::alpha_price(&client, rtao::primitives::NetUid(7)).await?";
    let hints = sink.text(Stream::Stderr)?;
    assert_eq!(
        hints,
        format!(
            "skip the prompts next time: rtao subnets price --netuid 7\nthe same read from the Rust SDK: {expected}\n"
        )
    );
    // Exercise the exact API named in the teaching line on the same fake view.
    let price = rtao::reads::generated::interfaces::alpha_price(&client, NetUid(7)).await?;
    assert_eq!(
        serde_json::to_value(price)?["price_rao"],
        json!(1_500_000_000_u64)
    );
    assert!(
        !chain
            .requests()
            .iter()
            .any(|request| request.method.starts_with("author_"))
    );
    client.close().await;
    Ok(())
}
