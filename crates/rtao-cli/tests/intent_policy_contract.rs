// Derived from RaoFoundation/subtensor sdk/python/bittensor/cli/intent_prompts.py @ 850f2755bb05a3851f948489f0ab27b0645cebce (Apache-2.0); modified by rtao.
//! Intent policy notices and required-after-prompt help through actual requests.

use std::{
    io,
    path::Path,
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
};

use clap::ArgMatches;
use rtao::{
    client::{Client, ClientConfig, Network},
    primitives::AccountId,
    wallet::{PasswordPrompt, SecretString, WalletError},
};
use rtao_cli::{
    application,
    books::Books,
    commands::generated::{self, TxRequest},
    context::AppContext,
    globals::Globals,
    output::{MemorySink, Output, OutputOptions, OutputSink, Stream},
    prompt::{PromptError, PromptHistory, Terminal},
};
use rtao_testkit::{FakeChain, FakeState, rpc::RpcClient};

const NOTICE: &str = "note: `--amount all` unstakes everything on a single subnet (--netuid); to unstake every position across all subnets, use `rtao stake unstake-all`";
const ROOT_HELP: &str = "pass it explicitly, or run on a terminal to pick one";
type TestResult = Result<(), Box<dyn std::error::Error>>;

#[derive(Clone, Debug)]
enum Event {
    Message(String),
    Prompt(String),
    Secret,
}
type Events = Arc<Mutex<Vec<Event>>>;
type ContextFixture = (AppContext, Events, MemorySink, Arc<AtomicUsize>);

struct Input {
    tty: bool,
    events: Events,
}
impl Terminal for Input {
    fn stdin_is_tty(&self) -> bool {
        self.tty
    }
    fn stderr_is_tty(&self) -> bool {
        self.tty
    }
    fn read_line(&mut self, label: &str) -> Result<String, PromptError> {
        self.events
            .lock()
            .map_err(|_| PromptError::Io)?
            .push(Event::Prompt(label.into()));
        Err(PromptError::Aborted)
    }
    fn read_secret(&mut self, _: &str) -> Result<SecretString, PromptError> {
        self.events
            .lock()
            .map_err(|_| PromptError::Io)?
            .push(Event::Secret);
        Err(PromptError::Aborted)
    }
    fn message(&mut self, message: &str) -> Result<(), PromptError> {
        self.events
            .lock()
            .map_err(|_| PromptError::Io)?
            .push(Event::Message(message.into()));
        Ok(())
    }
}
struct Capture {
    events: Events,
    sink: MemorySink,
}
impl OutputSink for Capture {
    fn write(&self, stream: Stream, bytes: &[u8]) -> io::Result<()> {
        self.sink.write(stream, bytes)?;
        if stream == Stream::Stderr {
            self.events
                .lock()
                .map_err(|_| io::Error::other("event capture poisoned"))?
                .push(Event::Message(String::from_utf8_lossy(bytes).into_owned()));
        }
        Ok(())
    }
}
struct Unlock(Arc<AtomicUsize>);
impl PasswordPrompt for Unlock {
    fn is_terminal(&self) -> bool {
        true
    }
    fn prompt(&self, _: &str) -> Result<SecretString, WalletError> {
        self.0.fetch_add(1, Ordering::SeqCst);
        Err(WalletError::PromptUnavailable)
    }
}

fn leaf(mut matches: &ArgMatches) -> &ArgMatches {
    while let Some((_, child)) = matches.subcommand() {
        matches = child;
    }
    matches
}
fn context(
    matches: &ArgMatches,
    op: &str,
    tty: bool,
) -> Result<ContextFixture, Box<dyn std::error::Error>> {
    let config = Default::default();
    let mut globals = Globals::resolve(
        matches,
        &config,
        Path::new("/isolated/intent-policy-no-user-home"),
        |_| None,
    )?;
    globals.apply_with_env(leaf(matches), &generated::intent_reserved(op)?, |_| None)?;
    // No local key material is present or needed by these request boundaries.
    globals.wallet_path = "/isolated/intent-policy-no-wallets".into();
    let events = Events::default();
    let mut app = AppContext::new(
        globals,
        config,
        Books::default(),
        Arc::new(Mutex::new(Box::new(Input {
            tty,
            events: events.clone(),
        }))),
    );
    let sink = MemorySink::default();
    app.output = Output::new(
        OutputOptions {
            json_mode: app.globals.json,
            quiet: app.globals.quiet,
            network: None,
        },
        Arc::new(Capture {
            events: events.clone(),
            sink: sink.clone(),
        }),
    );
    let unlocks = Arc::new(AtomicUsize::new(0));
    app.passwords.prompt_callback = Some(Arc::new(Unlock(unlocks.clone())));
    app.passwords.native_provider = None;
    Ok((app, events, sink, unlocks))
}
async fn client() -> Result<(Client, FakeChain), Box<dyn std::error::Error>> {
    let chain = FakeChain::from_state(FakeState::try_default()?);
    let mut config = ClientConfig::new(Network::resolve_with("local", |_| None)?)?;
    config.track_heads = false;
    config.runtime_cache_dir = None;
    config.config_store = None;
    let client = Client::from_rpc(RpcClient::new(chain.clone()), config).await?;
    Ok((client, chain))
}
fn no_chain_or_secret_work(
    chain: &FakeChain,
    before: usize,
    events: &[Event],
    unlocks: &AtomicUsize,
) -> TestResult {
    assert_eq!(chain.requests().len(), before);
    assert!(
        chain
            .state()
            .lock()
            .map_err(|_| "chain state")?
            .submissions
            .is_empty()
    );
    assert_eq!(unlocks.load(Ordering::SeqCst), 0);
    assert!(!events.iter().any(|event| matches!(event, Event::Secret)));
    Ok(())
}
fn notice_count(events: &[Event]) -> usize {
    events
        .iter()
        .filter(|event| matches!(event, Event::Message(text) if text.trim_end() == NOTICE))
        .count()
}

#[tokio::test]
async fn generated_unstake_notice_precedes_the_first_signing_wallet_question() -> TestResult {
    let parsed = application::command_tree([generated::tx_command()?]).try_get_matches_from([
        "rtao",
        "tx",
        "remove-stake",
        "--amount",
        "all",
    ])?;
    let (mut app, events, sink, unlocks) = context(&parsed, "remove_stake", true)?;
    assert!(!app.globals.wallet_given);
    let (client, chain) = client().await?;
    let before = chain.requests().len();
    let error = TxRequest::from_matches_with_client(
        &mut app,
        &client,
        "remove_stake",
        leaf(&parsed),
        &mut PromptHistory::default(),
    )
    .await
    .err()
    .ok_or("wallet confirmation must abort")?;
    assert_eq!(error.code, 130);
    {
        let events = events.lock().map_err(|_| "events")?;
        assert_eq!(notice_count(&events), 1);
        let notice = events
            .iter()
            .position(|event| matches!(event, Event::Message(text) if text.trim_end() == NOTICE))
            .ok_or("unstake notice")?;
        let question = events
            .iter()
            .position(
                |event| matches!(event, Event::Prompt(label) if label.starts_with("  wallet [")),
            )
            .ok_or("signing wallet question")?;
        assert!(notice < question);
        assert_eq!(
            events
                .iter()
                .filter(|event| matches!(event, Event::Prompt(_)))
                .count(),
            1
        );
        assert!(sink.text(Stream::Stdout)?.is_empty());
        no_chain_or_secret_work(&chain, before, &events, &unlocks)?;
    }
    client.close().await;
    Ok(())
}

#[tokio::test]
async fn generated_unstake_notice_respects_output_modes_and_flag_only_refusals() -> TestResult {
    let command = application::command_tree([generated::tx_command()?]);
    let address = AccountId([37; 32]).to_string();
    let (client, chain) = client().await?;
    for (flags, tty, amount, netuid, expected_notice) in [
        (vec![], false, "all", None, true),
        (vec!["--yes"], false, "all", None, true),
        (vec!["--signer", "extension"], false, "all", None, true),
        (vec!["--json"], true, "all", None, false),
        (vec!["--quiet"], true, "all", None, false),
        (vec!["--json", "--quiet"], true, "all", None, false),
        (vec![], false, "all", Some("0"), false),
        (vec![], false, "1", None, false),
    ] {
        let mut argv = vec!["rtao", "tx", "remove-stake", "--hotkey", &address];
        argv.extend(flags);
        argv.extend(["--amount", amount]);
        if let Some(netuid) = netuid {
            argv.extend(["--netuid", netuid]);
        }
        let parsed = command.clone().try_get_matches_from(argv)?;
        let (mut app, events, sink, unlocks) = context(&parsed, "remove_stake", tty)?;
        let before = chain.requests().len();
        let result = TxRequest::from_matches_with_client(
            &mut app,
            &client,
            "remove_stake",
            leaf(&parsed),
            &mut PromptHistory::default(),
        )
        .await;
        if netuid.is_none() {
            let error = result.err().ok_or("missing netuid must refuse")?;
            assert_eq!(error.code, 2);
            assert_eq!(error.message, "missing required option: `--netuid`");
            assert_eq!(error.help, None);
        } else {
            let request = result?;
            assert_eq!(request.args["netuid"], 0);
            assert_eq!(request.args["hotkey_ss58"], address);
            assert_eq!(request.args["amount_alpha"], "all");
        }
        let events = events.lock().map_err(|_| "events")?;
        assert_eq!(notice_count(&events), usize::from(expected_notice));
        assert!(!events.iter().any(|event| matches!(event, Event::Prompt(_))));
        assert!(sink.text(Stream::Stdout)?.is_empty());
        if app.globals.json || app.globals.quiet {
            assert!(events.is_empty());
            assert!(sink.text(Stream::Stderr)?.is_empty());
        }
        no_chain_or_secret_work(&chain, before, &events, &unlocks)?;
    }
    client.close().await;
    Ok(())
}

#[tokio::test]
async fn generated_root_claim_missing_hotkey_retains_exact_policy_remediation() -> TestResult {
    let command = application::command_tree([generated::tx_command()?]);
    let (client, chain) = client().await?;
    for (flags, tty) in [
        (vec![], false),
        (vec!["--yes"], true),
        (vec!["--signer", "extension"], true),
        (vec!["--json"], true),
        (vec!["--quiet"], true),
    ] {
        let mut argv = vec!["rtao", "tx", "claim-root-with-hotkey"];
        argv.extend(flags);
        let parsed = command.clone().try_get_matches_from(argv)?;
        let (mut app, events, sink, unlocks) = context(&parsed, "claim_root_with_hotkey", tty)?;
        let before = chain.requests().len();
        let error = TxRequest::from_matches_with_client(
            &mut app,
            &client,
            "claim_root_with_hotkey",
            leaf(&parsed),
            &mut PromptHistory::default(),
        )
        .await
        .err()
        .ok_or("missing root hotkey must refuse")?;
        assert_eq!(error.code, 2);
        assert_eq!(error.message, "missing required option: `--hotkey`");
        assert_eq!(error.help.as_deref(), Some(ROOT_HELP));
        let events = events.lock().map_err(|_| "events")?;
        assert!(events.is_empty());
        assert!(sink.text(Stream::Stdout)?.is_empty());
        assert!(sink.text(Stream::Stderr)?.is_empty());
        no_chain_or_secret_work(&chain, before, &events, &unlocks)?;
    }
    client.close().await;
    Ok(())
}
