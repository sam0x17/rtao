// Derived from RaoFoundation/subtensor sdk/python/bittensor/cli/call.py @ 850f2755bb05a3851f948489f0ab27b0645cebce (Apache-2.0); modified by rtao.
// Derived from RaoFoundation/subtensor sdk/python/bittensor/cli/multisig_helpers.py @ 850f2755bb05a3851f948489f0ab27b0645cebce (Apache-2.0); modified by rtao.
//! Raw-call composition and signing discipline through real production paths.

use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
};

use rtao::{
    client::{Client, ClientConfig, Inclusion, Network, View},
    primitives::{AccountId, CryptoType},
    signer::{BoxFuture, DynKeySource, KeyView, KeypairSigner, Signer, SignerError, SignerRole},
    tx::{ApproveOptions, Multisig},
};
use rtao_cli::{
    application::command_tree,
    books::{AccountKind, Books},
    call::{
        self, FsRawCallCache, MultisigCallSpec, MultisigFollowup, RawCallCache, RawCallRequest,
        RawMultisig, RawSignerFactory, SigningSelection,
    },
    context::AppContext,
    error::CliError,
    globals::Globals,
    output::{MemorySink, Output, OutputOptions, Stream},
    prompt::{PromptError, Terminal},
};
use rtao_testkit::{FakeChain, FakeState, SubmitScript, rpc::RpcClient};
use serde_json::{Value, json};

type TestResult = Result<(), Box<dyn std::error::Error>>;

struct NoTerminal;
impl Terminal for NoTerminal {
    fn stdin_is_tty(&self) -> bool {
        false
    }
    fn stderr_is_tty(&self) -> bool {
        false
    }
    fn read_line(&mut self, _: &str) -> Result<String, PromptError> {
        Err(PromptError::Unavailable)
    }
    fn read_secret(&mut self, _: &str) -> Result<rtao::wallet::SecretString, PromptError> {
        Err(PromptError::Unavailable)
    }
    fn message(&mut self, _: &str) -> Result<(), PromptError> {
        Ok(())
    }
}

struct ReviewedQuestion {
    question: String,
    review: String,
}

struct ReviewRefusal {
    sink: MemorySink,
    questions: Arc<Mutex<Vec<ReviewedQuestion>>>,
}

impl Terminal for ReviewRefusal {
    fn stdin_is_tty(&self) -> bool {
        true
    }
    fn stderr_is_tty(&self) -> bool {
        true
    }
    fn read_line(&mut self, question: &str) -> Result<String, PromptError> {
        let review = self
            .sink
            .text(Stream::Stdout)
            .map_err(|_| PromptError::Io)?;
        self.questions
            .lock()
            .map_err(|_| PromptError::Io)?
            .push(ReviewedQuestion {
                question: question.into(),
                review,
            });
        Ok("n".into())
    }
    fn read_secret(&mut self, _: &str) -> Result<rtao::wallet::SecretString, PromptError> {
        Err(PromptError::Unavailable)
    }
    fn message(&mut self, _: &str) -> Result<(), PromptError> {
        Ok(())
    }
}

fn context(flags: &[&str]) -> Result<(AppContext, MemorySink), Box<dyn std::error::Error>> {
    let argv = std::iter::once("rtao")
        .chain(std::iter::once("call"))
        .chain(std::iter::once("System.remark"))
        .chain(flags.iter().copied());
    let parsed = command_tree([call::command()]).try_get_matches_from(argv)?;
    let config = serde_json::Map::new();
    let mut globals = Globals::resolve(
        &parsed,
        &config,
        Path::new("/isolated/nonexistent-home"),
        |_| None,
    )?;
    globals.apply(parsed.subcommand().ok_or("call")?.1, &["signer_backend"])?;
    let mut ctx = AppContext::new(
        globals,
        config,
        Books::default(),
        Arc::new(Mutex::new(Box::new(NoTerminal))),
    );
    let sink = MemorySink::default();
    ctx.output = Output::new(
        OutputOptions {
            json_mode: ctx.globals.json,
            quiet: ctx.globals.quiet,
            network: Some(ctx.globals.network.clone()),
        },
        Arc::new(sink.clone()),
    );
    Ok((ctx, sink))
}

fn request() -> RawCallRequest {
    RawCallRequest {
        target: "System.remark".into(),
        params: serde_json::Map::from_iter([("remark".into(), json!("0x0102"))]),
        args_file: None,
        sudo: false,
        role: SignerRole::Coldkey,
        backend: None,
        proxy_for: None,
        force_proxy_type: None,
        multisig: None,
        rounds: vec![SigningSelection {
            display_name: None,
            wallet: "alice".into(),
            backend: "wallet".into(),
            address: None,
        }],
    }
}

#[derive(Clone)]
struct Keys {
    key: KeypairSigner,
    unlocks: Arc<AtomicUsize>,
}
impl Keys {
    fn new(seed: u8) -> Result<Self, SignerError> {
        Ok(Self {
            key: KeypairSigner::from_seed(&[seed; 32], CryptoType::Ed25519)?,
            unlocks: Arc::new(AtomicUsize::new(0)),
        })
    }
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
        Box::pin(async {
            self.unlocks.fetch_add(1, Ordering::SeqCst);
            Ok(Arc::new(self.key.clone()) as Arc<dyn Signer>)
        })
    }
}

struct Factory {
    keys: BTreeMap<String, Keys>,
    acquired: AtomicUsize,
    released: Mutex<Vec<bool>>,
}
impl Factory {
    fn new(keys: impl IntoIterator<Item = (String, Keys)>) -> Self {
        Self {
            keys: keys.into_iter().collect(),
            acquired: AtomicUsize::new(0),
            released: Mutex::new(Vec::new()),
        }
    }
}
impl RawSignerFactory for Factory {
    fn source<'a>(
        &'a self,
        _: &'a AppContext,
        selected: &'a SigningSelection,
    ) -> BoxFuture<'a, Result<Arc<dyn DynKeySource>, CliError>> {
        Box::pin(async move {
            self.acquired.fetch_add(1, Ordering::SeqCst);
            self.keys
                .get(&selected.wallet)
                .cloned()
                .map(|keys| Arc::new(keys) as Arc<dyn DynKeySource>)
                .ok_or_else(|| CliError::input("fixture missing source"))
        })
    }
    fn release<'a>(
        &'a self,
        _: &'a SigningSelection,
        _: &'a dyn DynKeySource,
        success: bool,
    ) -> BoxFuture<'a, Result<(), CliError>> {
        Box::pin(async move {
            self.released
                .lock()
                .map_err(|_| CliError::failure("capture poisoned"))?
                .push(success);
            Ok(())
        })
    }
}

struct RefuseFollowupAfterRelease<'a> {
    inner: &'a Factory,
    chain: &'a FakeChain,
    policy: rtao_rpc::PolicyRefusal,
}
impl RawSignerFactory for RefuseFollowupAfterRelease<'_> {
    fn source<'a>(
        &'a self,
        ctx: &'a AppContext,
        selection: &'a SigningSelection,
    ) -> BoxFuture<'a, Result<Arc<dyn DynKeySource>, CliError>> {
        self.inner.source(ctx, selection)
    }
    fn release<'a>(
        &'a self,
        selection: &'a SigningSelection,
        keys: &'a dyn DynKeySource,
        success: bool,
    ) -> BoxFuture<'a, Result<(), CliError>> {
        Box::pin(async move {
            self.inner.release(selection, keys, success).await?;
            assert!(
                success,
                "policy refusal must follow a successful submission"
            );
            self.chain.queue_transport_error(
                "chain_getBlockHash",
                rtao_testkit::rpc::Error::Client(Box::new(rtao_rpc::RpcError::Policy(Box::new(
                    self.policy.clone(),
                )))),
            );
            Ok(())
        })
    }
}

#[derive(Default)]
struct Cache(Mutex<BTreeMap<String, Value>>);
impl RawCallCache for Cache {
    fn save(&self, hash: String, spec: Value) -> BoxFuture<'_, Result<(), CliError>> {
        Box::pin(async move {
            self.0
                .lock()
                .map_err(|_| CliError::failure("cache capture poisoned"))?
                .insert(hash, spec);
            Ok(())
        })
    }
}

async fn fixture(keys: &[Keys]) -> Result<(Client, FakeChain), Box<dyn std::error::Error>> {
    let mut state = FakeState::try_default()?;
    for keys in keys {
        state.seed_python(
            "System",
            "Account",
            &[json!(keys.key.account().to_string())],
            json!({"data":{"free":10_000_000_000_u64}}),
        )?;
    }
    let chain = FakeChain::from_state(state);
    let mut config = ClientConfig::new(Network::resolve_with("local", |_| None)?)?;
    config.track_heads = false;
    config.runtime_cache_dir = None;
    config.config_store = None;
    let client = Client::from_rpc(RpcClient::new(chain.clone()), config).await?;
    Ok((client, chain))
}

#[test]
fn actual_parser_owns_role_backend_and_rejects_conflicting_inputs() -> TestResult {
    for selected in [
        "coldkey",
        "hotkey",
        "wallet",
        "vault",
        "ledger",
        "extension",
    ] {
        let parsed = command_tree([call::command()]).try_get_matches_from([
            "rtao",
            "call",
            "System.remark",
            "--key",
            selected,
            "--args",
            "{\"remark\":\"0x0102\"}",
        ])?;
        let matches = parsed.subcommand().ok_or("call")?.1;
        assert_eq!(
            matches
                .get_one::<String>("signer_backend")
                .map(String::as_str),
            Some(selected)
        );
    }
    for flags in [
        vec!["--signer", "unknown"],
        vec!["--multisig-threshold", "-1"],
    ] {
        let argv = vec!["rtao", "call", "System.remark"]
            .into_iter()
            .chain(flags);
        assert!(
            command_tree([call::command()])
                .try_get_matches_from(argv)
                .is_err()
        );
    }
    for target in ["System", "System.", "Unknown.remark", "System.unknown"] {
        assert!(call::resolve_builder(target).is_err());
    }
    assert!(call::resolve_builder("System.remark").is_ok());
    assert!(call::load_params(Some("{}"), Some(Path::new("missing"))).is_err());
    Ok(())
}

#[test]
fn input_parsing_retains_full_width_integers_and_rejects_nonobjects() -> TestResult {
    let params = call::load_params(
        Some(
            "{\"unsigned\":340282366920938463463374607431768211455,\"signed\":-170141183460469231731687303715884105728}",
        ),
        None,
    )?;
    assert_eq!(
        params["unsigned"]
            .as_number()
            .and_then(serde_json::Number::as_u128),
        Some(u128::MAX)
    );
    assert_eq!(
        params["signed"]
            .as_number()
            .and_then(serde_json::Number::as_i128),
        Some(i128::MIN)
    );
    assert!(call::load_params(Some("[]"), None).is_err());
    assert!(call::load_params(Some("{bad}"), None).is_err());
    for raw in [
        "  \n",
        "\u{1c}",
        "\u{1d}",
        "\u{1e}",
        "\u{1f}",
        "\t\u{1c}\u{1d}\u{1e}\u{1f}\n",
    ] {
        assert_eq!(call::load_params(Some(raw), None)?, serde_json::Map::new());
    }
    // Python strips only to detect an empty input; JSON still parses the
    // original text, so these separators cannot wrap a nonempty object.
    for separator in ['\u{1c}', '\u{1d}', '\u{1e}', '\u{1f}'] {
        assert!(call::load_params(Some(&format!("{separator}{{}}{separator}")), None).is_err());
    }
    Ok(())
}

#[test]
fn programmatic_raw_call_json_depth_refuses_before_runtime_encoding() -> TestResult {
    let runtime = rtao_runtime::RuntimeHandle::parse(
        include_bytes!("../../../vendor/metadata/finney/478.v15.scale"),
        478,
        1,
        42,
    )?;
    let leaf = json!({"module":"System","function":"remark","params":{"remark":"0x0102"}});
    let mut raw = request();
    raw.target = "Sudo.sudo".into();
    raw.params = serde_json::Map::from_iter([("call".into(), leaf.clone())]);
    let expected = rtao_runtime::calls::sudo::sudo(
        rtao_runtime::calls::system::remark(vec![1, 2]).compose(&runtime)?,
    )
    .compose(&runtime)?;
    assert_eq!(call::compose(&runtime, &raw)?, expected);

    // Build Values directly so serde_json's text parser cannot supply the refusal.
    // Every nested target is valid; the explicit conversion budget must reject it.
    for wrappers in [128, 129] {
        let mut nested = leaf.clone();
        for _ in 0..wrappers {
            nested = json!({"module":"Sudo","function":"sudo","params":{"call":nested}});
        }
        raw.params = serde_json::Map::from_iter([("call".into(), nested)]);
        let error = call::compose(&runtime, &raw)
            .err()
            .ok_or("over-deep programmatic raw call accepted")?;
        assert_eq!(error.code, 2);
        assert_eq!(
            error.message,
            "call JSON nesting exceeds parser's 128-level bound"
        );
    }
    Ok(())
}

#[test]
fn semantic_routes_resolve_public_members_without_a_raw_target_or_key_source() -> TestResult {
    let alice = AccountId([7; 32]);
    let bob = AccountId([8; 32]);
    let (mut ctx, sink) = context(&["--yes", "--signatory", "bob=ledger", "--signatory", "alice"])?;
    ctx.books = Books::from_values(
        json!([{"name":"alice","address":alice,"signer":"vault"}, {"name":"bob","address":bob,"signer":"extension"}]),
        json!([]),
        json!([{"name":"team","threshold":2,"signatories":["alice","bob","alice"]}]),
    );
    let group = call::resolve_preset(&ctx, "team")?;
    assert_eq!(group.signatories, vec![alice, bob]);
    assert_eq!(group.refs, vec!["alice", "bob", "alice"]);
    let rounds = call::plan_rounds(&ctx, Some(&group), SignerRole::Coldkey, None)?;
    assert_eq!(
        rounds,
        vec![
            SigningSelection {
                display_name: None,
                wallet: "bob".into(),
                address: Some(bob),
                backend: "ledger".into()
            },
            SigningSelection {
                display_name: None,
                wallet: "alice".into(),
                address: Some(alice),
                backend: "vault".into()
            },
        ]
    );
    assert!(call::select_member(&ctx, &group, SignerRole::Coldkey, None).is_err());
    ctx.globals.signatories = vec!["alice".into()];
    assert_eq!(
        call::select_member(&ctx, &group, SignerRole::Coldkey, None)?.address,
        Some(alice)
    );
    ctx.globals.signatories = vec!["alice".into(), "alice=ledger".into()];
    assert!(call::plan_rounds(&ctx, Some(&group), SignerRole::Coldkey, None).is_err());
    assert!(sink.text(Stream::Stderr)?.is_empty());
    ctx.globals.signatories.clear();
    let defaults = call::plan_rounds(&ctx, Some(&group), SignerRole::Coldkey, None)?;
    assert_eq!(
        sink.text(Stream::Stderr)?,
        "no --signatory given — approvals planned from local wallets and address-book signer tags\n"
    );
    assert_eq!(
        defaults
            .iter()
            .filter_map(|round| round.address)
            .collect::<Vec<_>>(),
        vec![alice, bob]
    );
    assert_eq!(
        defaults
            .iter()
            .map(|round| round.backend.as_str())
            .collect::<Vec<_>>(),
        vec!["vault", "extension"]
    );
    // A single empty suffix is rejected by globals, while the repeated-member
    // planner treats an empty normalized suffix as an instruction to infer.
    assert!(context(&["--yes", "--signatory", "alice="]).is_err());
    for root_wallet in [false, true] {
        for members in [
            vec![],
            vec!["--signatory", "alice"],
            vec!["--signatory", "alice=", "--signatory", "bob="],
            vec!["--signatory", "alice=ledger"],
        ] {
            let automatic = !root_wallet && members.is_empty();
            let expected = if root_wallet && (members.is_empty() || members[1] == "alice") {
                None
            } else if members.len() == 4 || members.is_empty() {
                Some(vec!["vault", "extension"])
            } else if members[1] == "alice=ledger" {
                Some(vec!["ledger"])
            } else {
                Some(vec!["vault"])
            };
            let mut argv = vec!["rtao"];
            if root_wallet {
                argv.extend(["--signer", "wallet"]);
            }
            argv.extend(["call", "System.remark", "--multisig", "team", "--yes"]);
            if !root_wallet {
                argv.extend(["--signer", "wallet"]);
            }
            argv.extend(members);
            let parsed = command_tree([call::command()]).try_get_matches_from(argv)?;
            let leaf = parsed.subcommand().ok_or("call command")?.1;
            let config = serde_json::Map::new();
            let mut globals = Globals::resolve(
                &parsed,
                &config,
                Path::new("/isolated/nonexistent-home"),
                |_| None,
            )?;
            globals.apply_with_env(leaf, &["signer_backend"], |_| None)?;
            let mut selected = AppContext::new(
                globals,
                config,
                ctx.books.clone(),
                Arc::new(Mutex::new(Box::new(NoTerminal))),
            );
            let sink = MemorySink::default();
            selected.output = Output::new(OutputOptions::default(), Arc::new(sink.clone()));
            let result = RawCallRequest::from_matches(&selected, leaf);
            if let Some(expected) = expected {
                assert_eq!(
                    result?
                        .rounds
                        .iter()
                        .map(|round| round.backend.as_str())
                        .collect::<Vec<_>>(),
                    expected
                );
            } else {
                let error = result
                    .err()
                    .ok_or("root wallet must not use external tags")?;
                assert_eq!(error.code, 2);
                assert!(error.message.contains("local"));
            }
            assert_eq!(
                sink.text(Stream::Stderr)?.contains("approvals planned"),
                automatic
            );
        }
    }
    ctx.globals.signatories = vec!["alice=\u{1c}\u{1f}".into(), "bob".into()];
    assert_eq!(
        call::plan_rounds(&ctx, Some(&group), SignerRole::Coldkey, None)?
            .iter()
            .map(|round| round.backend.as_str())
            .collect::<Vec<_>>(),
        ["vault", "extension"]
    );
    assert!(call::resolve_preset(&ctx, "missing").is_err());
    ctx.books.multisigs[0]["threshold"] = json!(0);
    assert!(call::resolve_preset(&ctx, "team").is_err());
    Ok(())
}

#[test]
fn command_owned_external_backend_uses_shared_account_precedence_without_tags_or_prompts()
-> TestResult {
    let alice = AccountId([7; 32]);
    let bob = AccountId([8; 32]);
    let (mut ctx, _) = context(&["--yes", "--signatory", "alice"])?;
    ctx.books = Books::from_values(
        json!([{"name":"alice","address":alice},{"name":"bob","address":bob}]),
        json!([]),
        json!([{"name":"team","threshold":2,"signatories":["alice","bob"]}]),
    );
    let group = call::resolve_preset(&ctx, "team")?;
    // An explicitly selected backend needs no persisted backend tag.
    assert_eq!(
        call::select_member(&ctx, &group, SignerRole::Coldkey, Some("vault"))?.address,
        Some(alice)
    );
    // Explicit address wins over the selected member.
    ctx.globals.signer_address = Some(bob.to_string());
    assert_eq!(
        call::select_member(&ctx, &group, SignerRole::Coldkey, Some("ledger"))?.address,
        Some(bob)
    );
    ctx.globals.signatories.clear();
    ctx.globals.signer_address = None;
    ctx.config.insert("signer_address".into(), json!("bob"));
    // Persisted address is a fallback without changing globals provenance.
    assert_eq!(
        call::select_member(&ctx, &group, SignerRole::Coldkey, Some("extension"))?.address,
        Some(bob)
    );
    assert!(ctx.globals.signer_address.is_none());
    // The extension picker receives the persisted address as a preference;
    // ordinary planning must not turn it into an explicit source-filter pin.
    let ordinary = call::plan_rounds(&ctx, None, SignerRole::Coldkey, Some("extension"))?;
    assert_eq!(ordinary[0].address, None);
    assert_eq!(ordinary[0].backend, "extension");
    for backend in ["vault", "ledger"] {
        let ordinary = call::plan_rounds(&ctx, None, SignerRole::Coldkey, Some(backend))?;
        assert_eq!(ordinary[0].address, Some(bob));
    }
    ctx.globals.signer_address = Some("bob".into());
    let explicit = call::plan_rounds(&ctx, None, SignerRole::Coldkey, Some("extension"))?;
    assert_eq!(explicit[0].address, Some(bob));
    ctx.globals.signer_address = None;
    ctx.config
        .insert("signer_address".into(), json!(AccountId([9; 32])));
    assert!(call::select_member(&ctx, &group, SignerRole::Coldkey, Some("vault")).is_err());
    Ok(())
}

#[test]
fn multisig_replay_uses_rtao_and_preserves_btcli_inside_user_arguments() -> TestResult {
    let (mut ctx, _) = context(&[])?;
    ctx.globals.network = "finney".into();
    let alice = AccountId([7; 32]);
    let bob = AccountId([8; 32]);
    let group = RawMultisig {
        threshold: 2,
        signatories: vec![alice, bob],
        preset: Some("btcli team".into()),
        refs: vec![alice.to_string(), bob.to_string()],
    };
    let labels = BTreeMap::from([(alice, "btcli member".into())]);
    let mut spec = MultisigCallSpec {
        target: Some("System.remark".into()),
        params: serde_json::Map::from_iter([("remark".into(), json!("btcli stays unchanged"))]),
        ..MultisigCallSpec::unknown(SignerRole::Coldkey)
    };
    let command = call::multisig_replay_command(&ctx, &spec, &group, alice, &labels)?;
    assert_eq!(
        command,
        "rtao call System.remark --args '{\"remark\":\"btcli stays unchanged\"}' -w 'btcli team' --signatory 'btcli member'"
    );
    assert_eq!(spec.params["remark"], json!("btcli stays unchanged"));

    spec.args_file = Some(PathBuf::from("btcli user arguments.json"));
    let command = call::multisig_replay_command(&ctx, &spec, &group, alice, &labels)?;
    assert_eq!(
        command,
        "rtao call System.remark --args-file 'btcli user arguments.json' -w 'btcli team' --signatory 'btcli member'"
    );

    let ordinary = command;
    for (network, _) in rtao_catalog::py::NETWORKS {
        ctx.globals.network = (*network).into();
        let replay = call::multisig_replay_command(&ctx, &spec, &group, alice, &labels)?;
        let expected = if *network == "finney" {
            ordinary.clone()
        } else {
            ordinary.replacen("rtao ", &format!("rtao -n {network} "), 1)
        };
        assert_eq!(replay, expected);
        assert!(!replay.contains("BT_NETWORK"));
        assert_eq!(ctx.globals.network, *network);
    }
    let environment =
        r#"BT_NETWORK="${BT_NETWORK:?set BT_NETWORK to your configured RPC endpoint}" "#;
    for endpoint in [
        "wss://url-user:url-password@rpc.invalid:9944/",
        "ws://rpc.invalid:9944/private-path-token",
        "wss://rpc.invalid/?api_key=query-token",
        "wss://rpc.invalid/#fragment-token",
        "wss://url-user%40team:url-password%21@rpc.invalid/private-path-token?api_key=query-token#fragment-token",
    ] {
        ctx.globals.network = endpoint.into();
        let before = ctx.client_config()?.network.endpoint().to_owned();
        let replay = call::multisig_replay_command(&ctx, &spec, &group, alice, &labels)?;
        assert_eq!(replay, format!("{environment}{ordinary}"));
        for private in [
            endpoint,
            "rpc.invalid",
            "url-user",
            "url-password",
            "private-path-token",
            "query-token",
            "fragment-token",
        ] {
            assert!(!replay.contains(private));
        }
        assert_eq!(ctx.globals.network, endpoint);
        assert_eq!(ctx.client_config()?.network.endpoint(), before);

        // Hosted Unix fixtures intercept the executable with a shell function;
        // no real CLI, endpoint, signer or transaction is invoked.
        #[cfg(unix)]
        {
            let script = format!(r#"rtao() {{ printf '%s\n' "$BT_NETWORK"; }}; {replay}"#);
            for value in [None, Some("")] {
                let mut shell = std::process::Command::new("sh");
                shell.arg("-c").arg(&script).env_remove("BT_NETWORK");
                if let Some(value) = value {
                    shell.env("BT_NETWORK", value);
                }
                let result = shell.output()?;
                assert!(!result.status.success());
                assert!(result.stdout.is_empty());
                assert!(
                    String::from_utf8(result.stderr)?
                        .contains("set BT_NETWORK to your configured RPC endpoint")
                );
            }
            let result = std::process::Command::new("sh")
                .arg("-c")
                .arg(&script)
                .env("BT_NETWORK", endpoint)
                .output()?;
            assert!(result.status.success());
            assert_eq!(String::from_utf8(result.stdout)?, format!("{endpoint}\n"));
        }
    }
    Ok(())
}

#[tokio::test]
async fn composition_uses_live_core_bytes_sudo_then_proxy_and_runtime_stamps() -> TestResult {
    let keys = Keys::new(7)?;
    let (client, chain) = fixture(std::slice::from_ref(&keys)).await?;
    let runtime = View::runtime(&client).await?;
    let mut raw = request();
    let plain = call::compose(&runtime, &raw)?;
    let expected = rtao_runtime::calls::system::remark(vec![1, 2]).compose(&runtime)?;
    assert_eq!(plain, expected);
    raw.sudo = true;
    raw.proxy_for = Some(AccountId([9; 32]));
    let composed = call::compose(&runtime, &raw)?;
    let expected = rtao::client::compose::proxy(
        &runtime,
        AccountId([9; 32]),
        None,
        rtao::client::compose::sudo(&runtime, plain)?,
    )?;
    assert_eq!(composed, expected);
    let decoded = client
        .decode_scale("Call", composed.as_bytes())
        .await?
        .to_json();
    assert_eq!(decoded["call_module"], json!("Proxy"));
    assert_eq!(decoded["call_function"], json!("proxy"));
    assert_eq!(composed.stamp(), runtime.stamp());
    let decoded_spec =
        call::decode_multisig_call_spec(&client, &composed, SignerRole::Coldkey).await;
    assert_eq!(decoded_spec.target.as_deref(), Some("System.remark"));
    assert!(decoded_spec.sudo);
    assert_eq!(decoded_spec.proxy_for, Some(AccountId([9; 32])));
    let rebuilt = RawCallRequest {
        target: decoded_spec.target.ok_or("decoded target")?,
        params: decoded_spec.params,
        sudo: decoded_spec.sudo,
        proxy_for: decoded_spec.proxy_for,
        force_proxy_type: decoded_spec.force_proxy_type,
        ..request()
    };
    assert_eq!(call::compose(&runtime, &rebuilt)?, composed);
    let changed = rtao_runtime::RuntimeHandle::parse(
        include_bytes!("../../../vendor/metadata/finney/478.v15.scale"),
        478,
        2,
        42,
    )?;
    assert!(composed.ensure_runtime(&changed).is_err());
    assert!(
        chain
            .state()
            .lock()
            .map_err(|_| "state capture")?
            .submissions
            .is_empty()
    );
    client.close().await;
    Ok(())
}

#[tokio::test]
async fn nested_specs_compose_in_all_python_shapes_without_float_coercion() -> TestResult {
    let (client, _) = fixture(&[]).await?;
    let runtime = View::runtime(&client).await?;
    let expected = rtao_runtime::calls::sudo::sudo(
        rtao_runtime::calls::system::remark(vec![1, 2]).compose(&runtime)?,
    )
    .compose(&runtime)?;
    for nested in [
        json!({"module":"System","function":"remark","params":{"remark":"0x0102"}}),
        json!({"call_module":"System","call_function":"remark","call_args":{"remark":"0x0102"}}),
        json!({"System":{"remark":{"remark":"0x0102"}}}),
    ] {
        let mut raw = request();
        raw.target = "Sudo.sudo".into();
        raw.params = serde_json::Map::from_iter([("call".into(), nested)]);
        assert_eq!(call::compose(&runtime, &raw)?, expected);
    }
    let mut raw = request();
    raw.params.insert("unknown".into(), json!(1));
    assert!(call::compose(&runtime, &raw).is_err());
    let mut raw = request();
    raw.target = "SubtensorModule.sudo_set_tx_childkey_take_rate_limit".into();
    raw.params = serde_json::Map::from_iter([("tx_rate_limit".into(), json!(1.25))]);
    assert!(call::compose(&runtime, &raw).is_err());
    raw.params.insert("tx_rate_limit".into(), json!(u64::MAX));
    call::compose(&runtime, &raw)?;
    raw.params
        .insert("tx_rate_limit".into(), json!(u128::from(u64::MAX) + 1));
    assert!(call::compose(&runtime, &raw).is_err());
    client.close().await;
    Ok(())
}

#[tokio::test]
async fn dry_run_validates_composition_without_acquiring_or_unlocking_a_signer() -> TestResult {
    let keys = Keys::new(7)?;
    let (client, chain) = fixture(std::slice::from_ref(&keys)).await?;
    let (ctx, sink) = context(&["--json", "--dry-run"])?;
    let factory = Factory::new([("alice".into(), keys.clone())]);
    let result = call::execute_rounds_with(
        &ctx,
        &client,
        &request(),
        "rtao call System.remark --args '{}'",
        &factory,
        &Cache::default(),
    )
    .await?;
    assert!(result.is_none());
    assert_eq!(factory.acquired.load(Ordering::SeqCst), 0);
    assert_eq!(keys.unlocks.load(Ordering::SeqCst), 0);
    assert!(
        chain
            .state()
            .lock()
            .map_err(|_| "state capture")?
            .submissions
            .is_empty()
    );
    let payload: Value = serde_json::from_str(&sink.text(Stream::Stdout)?)?;
    assert_eq!(payload["target"], json!("System.remark"));
    assert_eq!(payload["params"], json!({"remark":"0x0102"}));
    assert!(payload.get("fee_tao").is_none());
    assert!(sink.bytes(Stream::Stderr)?.is_empty());
    let mut invalid = request();
    invalid.params = serde_json::Map::new();
    assert!(
        call::execute_rounds_with(
            &ctx,
            &client,
            &invalid,
            "rtao call",
            &factory,
            &Cache::default()
        )
        .await
        .is_err()
    );
    assert_eq!(factory.acquired.load(Ordering::SeqCst), 0);
    client.close().await;
    Ok(())
}

#[tokio::test]
// py: cli/call.py::call (ordered name/backend review before show_review and confirmation)
async fn raw_round_review_lists_member_backends_before_confirmation_without_private_sources()
-> TestResult {
    for flags in [vec![], vec!["--quiet"], vec!["--json"]] {
        let alice = Keys::new(7)?;
        let bob = Keys::new(8)?;
        let (client, chain) = fixture(&[alice.clone(), bob.clone()]).await?;
        let checked: TestResult = async {
            let (mut ctx, sink) = context(&flags)?;
            let questions = Arc::new(Mutex::new(Vec::new()));
            ctx.terminal = Arc::new(Mutex::new(Box::new(ReviewRefusal {
                sink: sink.clone(),
                questions: questions.clone(),
            })));
            let mut raw = request();
            raw.multisig = Some(RawMultisig {
                threshold: 2,
                signatories: vec![alice.key.account(), bob.key.account()],
                preset: Some("team".into()),
                refs: vec!["alice".into(), "bob".into()],
            });
            // Approval order can differ from the group's canonical member order.
            raw.rounds = vec![
                SigningSelection {
                    display_name: None,
                    wallet: "bob".into(),
                    backend: "ledger".into(),
                    address: Some(bob.key.account()),
                },
                SigningSelection {
                    display_name: None,
                    wallet: "alice".into(),
                    backend: "vault".into(),
                    address: Some(alice.key.account()),
                },
            ];
            let factory =
                Factory::new([("alice".into(), alice.clone()), ("bob".into(), bob.clone())]);
            let cache = Cache::default();
            let error =
                call::execute_rounds_with(&ctx, &client, &raw, "rtao call", &factory, &cache)
                    .await
                    .err()
                    .ok_or("confirmation must refuse")?;
            assert_eq!(error.code, 1);
            let questions = questions.lock().map_err(|_| "question capture")?;
            if ctx.globals.json {
                assert!(questions.is_empty());
                assert!(sink.text(Stream::Stdout)?.is_empty());
                assert_eq!(error.message, "refusing to submit without confirmation");
            } else {
                assert_eq!(questions.len(), 1);
                let captured = questions.first().ok_or("confirmation question")?;
                assert!(captured.question.contains("2 approvals"));
                assert!(captured.question.contains("System.remark"));
                assert_eq!(error.message, "aborted.");
                if ctx.globals.quiet {
                    assert!(captured.review.is_empty());
                } else {
                    let bob = captured.review.find("bob (ledger)").ok_or("first member")?;
                    let alice = captured
                        .review
                        .find("alice (vault)")
                        .ok_or("second member")?;
                    assert!(bob < alice);
                    assert!(captured.review.contains("approval 1 account"));
                    assert!(captured.review.contains("approval 2 account"));
                    assert!(
                        captured
                            .review
                            .contains(&raw.rounds[0].address.ok_or("bob account")?.to_string())
                    );
                    assert!(
                        captured
                            .review
                            .contains(&raw.rounds[1].address.ok_or("alice account")?.to_string())
                    );
                }
            }
            assert_eq!(factory.acquired.load(Ordering::SeqCst), 0);
            assert!(factory.released.lock().map_err(|_| "releases")?.is_empty());
            assert_eq!(alice.unlocks.load(Ordering::SeqCst), 0);
            assert_eq!(bob.unlocks.load(Ordering::SeqCst), 0);
            assert!(cache.0.lock().map_err(|_| "cache")?.is_empty());
            assert!(
                chain
                    .state()
                    .lock()
                    .map_err(|_| "state")?
                    .submissions
                    .is_empty()
            );
            assert!(
                chain
                    .requests()
                    .iter()
                    .all(|request| !request.method.starts_with("author_"))
            );
            Ok(())
        }
        .await;
        client.close().await;
        checked?;
    }
    Ok(())
}

#[tokio::test]
async fn refusal_and_required_shield_fail_before_any_private_key_acquisition() -> TestResult {
    let keys = Keys::new(7)?;
    let (client, chain) = fixture(std::slice::from_ref(&keys)).await?;
    let (ctx, _) = context(&["--json"])?;
    let factory = Factory::new([("alice".into(), keys.clone())]);
    let error = call::execute_rounds_with(
        &ctx,
        &client,
        &request(),
        "rtao call",
        &factory,
        &Cache::default(),
    )
    .await
    .err()
    .ok_or("must refuse")?;
    assert_eq!(error.code, 1);
    let (mut ctx, _) = context(&["--yes", "--no-mev-shield"])?;
    let mut required = request();
    required.target = "SubtensorModule.burned_register".into();
    required.params = serde_json::Map::from_iter([
        ("netuid".into(), json!(1)),
        ("hotkey".into(), json!(keys.key.account())),
    ]);
    ctx.globals.dry_run = true;
    assert_eq!(
        call::shield_policy_for_target(&required.target),
        (true, true)
    );
    let error = call::execute_rounds_with(
        &ctx,
        &client,
        &required,
        "rtao call",
        &factory,
        &Cache::default(),
    )
    .await
    .err()
    .ok_or("required shield")?;
    assert_eq!(error.code, 2);
    assert_eq!(factory.acquired.load(Ordering::SeqCst), 0);
    assert_eq!(keys.unlocks.load(Ordering::SeqCst), 0);
    assert!(
        chain
            .state()
            .lock()
            .map_err(|_| "state capture")?
            .submissions
            .is_empty()
    );
    client.close().await;
    Ok(())
}

#[tokio::test]
async fn direct_submission_has_no_default_library_policy_and_releases_source() -> TestResult {
    let keys = Keys::new(7)?;
    let (client, chain) = fixture(std::slice::from_ref(&keys)).await?;
    let (ctx, sink) = context(&["--yes", "--json"])?;
    let factory = Factory::new([("alice".into(), keys.clone())]);
    let receipt = call::execute_rounds_with(
        &ctx,
        &client,
        &request(),
        "rtao call",
        &factory,
        &Cache::default(),
    )
    .await?
    .ok_or("receipt")?;
    assert!(receipt.success);
    assert_eq!(factory.acquired.load(Ordering::SeqCst), 1);
    assert_eq!(keys.unlocks.load(Ordering::SeqCst), 1);
    assert_eq!(
        *factory.released.lock().map_err(|_| "release capture")?,
        vec![true]
    );
    assert_eq!(
        chain
            .state()
            .lock()
            .map_err(|_| "state capture")?
            .submissions
            .len(),
        1
    );
    assert_eq!(
        serde_json::from_str::<Value>(&sink.text(Stream::Stdout)?)?,
        receipt.to_json()
    );
    client.close().await;
    Ok(())
}

fn success_and_executed(approving: AccountId, group: &Multisig, hash: [u8; 32]) -> Value {
    json!([
        {"phase":"ApplyExtrinsic","extrinsic_idx":0,"module_id":"System","event_id":"ExtrinsicSuccess","attributes":{"dispatch_info":{"weight":{"ref_time":1000,"proof_size":0},"class":"Normal","pays_fee":"Yes"}},"topics":[]},
        {"phase":"ApplyExtrinsic","extrinsic_idx":0,"module_id":"Multisig","event_id":"MultisigExecuted","attributes":{"approving":approving,"timepoint":{"height":10,"index":1},"multisig":group.address,"call_hash":format!("0x{}",hex::encode(hash)),"result":{"Ok":[]}},"topics":[]}
    ])
}

fn approval_events(
    approving: AccountId,
    group: &Multisig,
    hash: [u8; 32],
    opening: Option<rtao_runtime::models::MultisigTimepoint>,
) -> Value {
    let mut attributes = json!({"approving":approving,"multisig":group.address,"call_hash":format!("0x{}",hex::encode(hash))});
    let event = if let Some(opening) = opening {
        attributes["timepoint"] = json!(opening);
        "MultisigApproval"
    } else {
        "NewMultisig"
    };
    json!([
        {"phase":"ApplyExtrinsic","extrinsic_idx":0,"module_id":"System","event_id":"ExtrinsicSuccess","attributes":{"dispatch_info":{"weight":{"ref_time":1000,"proof_size":0},"class":"Normal","pays_fee":"Yes"}},"topics":[]},
        {"phase":"ApplyExtrinsic","extrinsic_idx":0,"module_id":"Multisig","event_id":event,"attributes":attributes,"topics":[]}
    ])
}

#[derive(Clone, Copy)]
enum PendingAfterFirst {
    Same,
    Missing,
    Replaced,
}

struct ContinuationFactory {
    factory: Factory,
    chain: FakeChain,
    group: Multisig,
    call: rtao_runtime::CallBytes,
    pending: PendingAfterFirst,
}
impl RawSignerFactory for ContinuationFactory {
    fn source<'a>(
        &'a self,
        ctx: &'a AppContext,
        selection: &'a SigningSelection,
    ) -> BoxFuture<'a, Result<Arc<dyn DynKeySource>, CliError>> {
        self.factory.source(ctx, selection)
    }
    fn release<'a>(
        &'a self,
        selection: &'a SigningSelection,
        source: &'a dyn DynKeySource,
        success: bool,
    ) -> BoxFuture<'a, Result<(), CliError>> {
        Box::pin(async move {
            self.factory.release(selection, source, success).await?;
            if success
                && self.factory.acquired.load(Ordering::SeqCst) == 1
                && !matches!(self.pending, PendingAfterFirst::Missing)
            {
                self.chain.mutate_state(|state| {
                    let height = state.block + u64::from(matches!(self.pending, PendingAfterFirst::Replaced));
                    state.seed_python("Multisig", "Multisigs", &[
                        json!(self.group.address), json!(format!("0x{}",hex::encode(self.call.hash()))),
                    ], json!({"when":{"height":height,"index":0},"deposit":0,"depositor":source.coldkey_address().map_err(|error|CliError::failure(error.to_string()))?,"approvals":[source.coldkey_address().map_err(|error|CliError::failure(error.to_string()))?]}))
                        .map_err(|error| CliError::failure(error.to_string()))
                })?;
            }
            Ok(())
        })
    }
}

#[tokio::test]
async fn opening_timepoint_is_authenticated_from_included_matching_receipt_events() -> TestResult {
    use rtao_runtime::models::MultisigTimepoint;
    let keys = [Keys::new(7)?, Keys::new(8)?];
    let (client, chain) = fixture(&keys).await?;
    let runtime = View::runtime(&client).await?;
    let call = call::compose(&runtime, &request())?;
    let group = Multisig::new(
        &runtime,
        &keys
            .iter()
            .map(|keys| keys.key.account())
            .collect::<Vec<_>>(),
        2,
    )?;
    chain.mutate_state(|state| {
        state.queue_submit(SubmitScript::InBlockThenFinalized {
            events: approval_events(keys[0].key.account(), &group, call.hash(), None),
        })
    });
    let executor = rtao::Executor::new(client.clone());
    let receipt = group
        .approve(
            &executor,
            call.clone(),
            &keys[0],
            &ApproveOptions::default(),
        )
        .await?;
    let (height, index) = receipt
        .extrinsic_id
        .as_deref()
        .ok_or("included id")?
        .split_once('-')
        .ok_or("height-index")?;
    let expected = MultisigTimepoint {
        height: height.parse()?,
        index: index.parse()?,
    };
    assert_eq!(call::approval_timepoint(&receipt, &group, &call)?, expected);
    let mut changed = receipt.clone();
    changed.extrinsic_id = None;
    assert!(call::approval_timepoint(&changed, &group, &call).is_err());
    changed = receipt.clone();
    changed.inclusion = Inclusion::PoolAccepted;
    assert!(call::approval_timepoint(&changed, &group, &call).is_err());
    changed = receipt.clone();
    changed.events[1].extrinsic_idx = Some(expected.index + 1);
    assert!(call::approval_timepoint(&changed, &group, &call).is_err());
    changed = receipt.clone();
    changed.events.push(changed.events[1].clone());
    assert!(call::approval_timepoint(&changed, &group, &call).is_err());
    changed = receipt.clone();
    changed.events.clear();
    assert!(call::approval_timepoint(&changed, &group, &call).is_err());
    let other = rtao_runtime::calls::system::remark(vec![9]).compose(&runtime)?;
    assert!(call::approval_timepoint(&receipt, &group, &other).is_err());
    let other_group = Multisig::new(&runtime, &[keys[0].key.account(), AccountId([99; 32])], 2)?;
    assert!(call::approval_timepoint(&receipt, &other_group, &call).is_err());

    let original = MultisigTimepoint {
        height: 17,
        index: 3,
    };
    chain.mutate_state(|state|state.seed_python("Multisig", "Multisigs", &[json!(group.address),json!(format!("0x{}",hex::encode(call.hash())))], json!({"when":original,"deposit":0,"depositor":keys[0].key.account(),"approvals":[keys[0].key.account()]})))?;
    chain.mutate_state(|state| {
        state.queue_submit(SubmitScript::InBlockThenFinalized {
            events: approval_events(
                keys[1].key.account(),
                &group,
                call.hash(),
                Some(original.clone()),
            ),
        })
    });
    let later = group
        .approve(
            &executor,
            call.clone(),
            &keys[1],
            &ApproveOptions::default(),
        )
        .await?;
    assert_eq!(call::approval_timepoint(&later, &group, &call)?, original);
    client.close().await;
    Ok(())
}

fn chained_request(keys: &[Keys]) -> RawCallRequest {
    let mut raw = request();
    raw.multisig = Some(RawMultisig {
        threshold: 2,
        signatories: keys.iter().map(|key| key.key.account()).collect(),
        preset: Some("team".into()),
        refs: keys
            .iter()
            .map(|key| key.key.account().to_string())
            .collect(),
    });
    raw.rounds = keys
        .iter()
        .enumerate()
        .map(|(index, key)| SigningSelection {
            display_name: None,
            wallet: format!("member{index}"),
            backend: "wallet".into(),
            address: Some(key.key.account()),
        })
        .collect();
    raw
}

#[tokio::test]
async fn raw_multisig_funding_help_survives_cli_conversion_and_both_output_modes() -> TestResult {
    let keys = [Keys::new(7)?, Keys::new(8)?];
    let (client, chain) = fixture(&keys).await?;
    let payer = keys[0].key.account();
    chain.mutate_state(|state| {
        state.fee = 100_000;
        state.seed_python(
            "System",
            "Account",
            &[json!(payer)],
            json!({"data":{"free":0}}),
        )
    })?;
    let factory = Factory::new(
        keys.iter()
            .enumerate()
            .map(|(index, key)| (format!("member{index}"), key.clone())),
    );
    let (ctx, _) = context(&["--yes", "--json", "--no-mev-shield"])?;
    let error = call::execute_rounds_with(
        &ctx,
        &client,
        &chained_request(&keys),
        "rtao call",
        &factory,
        &Cache::default(),
    )
    .await
    .err()
    .ok_or("funding must refuse")?;
    let expected_message = format!(
        "the signing account cannot cover the multisig deposit plus the transaction fee: {payer} holds τ0 free, but opening this operation reserves a τ0.196 deposit on top of a ~τ0.0001 fee"
    );
    let expected_help =
        format!("fund {payer} with ≥ τ0.197 — the deposit is returned when the op executes");
    assert_eq!(error.code, 1);
    assert_eq!(error.message, expected_message);
    assert_eq!(error.help.as_deref(), Some(expected_help.as_str()));
    for json_mode in [false, true] {
        let sink = MemorySink::default();
        let output = Output::new(
            OutputOptions {
                json_mode,
                ..Default::default()
            },
            Arc::new(sink.clone()),
        );
        output.error(
            &error.message,
            rtao_cli::output::DiagnosticContext {
                help: error.help.as_deref(),
                ..Default::default()
            },
        )?;
        let stderr = sink.text(Stream::Stderr)?;
        if json_mode {
            assert_eq!(
                serde_json::from_str::<Value>(&stderr)?,
                json!({"error":expected_message,"help":expected_help})
            );
        } else {
            assert!(stderr.contains(&expected_message));
            assert!(stderr.contains(&expected_help));
        }
        assert!(sink.text(Stream::Stdout)?.is_empty());
    }
    assert_eq!(factory.acquired.load(Ordering::SeqCst), 1);
    assert_eq!(
        *factory.released.lock().map_err(|_| "release")?,
        vec![false]
    );
    assert!(
        keys.iter()
            .all(|key| key.unlocks.load(Ordering::SeqCst) == 0)
    );
    assert!(chain.mutate_state(|state| state.submissions.is_empty()));
    client.close().await;
    Ok(())
}

#[tokio::test]
async fn continued_funding_refusal_keeps_remediation_and_original_opening_recovery() -> TestResult {
    let keys = [Keys::new(7)?, Keys::new(8)?];
    let (client, chain) = fixture(&keys).await?;
    let runtime = View::runtime(&client).await?;
    let raw = chained_request(&keys);
    let inner = call::compose(&runtime, &raw)?;
    let hash = format!("0x{}", hex::encode(inner.hash()));
    let second = keys[1].key.account();
    let group = Multisig::new(
        &runtime,
        &raw.multisig.as_ref().ok_or("group")?.signatories,
        2,
    )?;
    chain.mutate_state(|state| -> TestResult {
        state.fee = 100_000;
        state.seed_python(
            "System",
            "Account",
            &[json!(second)],
            json!({"data":{"free":0}}),
        )?;
        state.queue_submit(SubmitScript::InBlockThenFinalized {
            events: approval_events(keys[0].key.account(), &group, inner.hash(), None),
        });
        Ok(())
    })?;
    let factory = ContinuationFactory {
        factory: Factory::new(
            keys.iter()
                .enumerate()
                .map(|(index, key)| (format!("member{index}"), key.clone())),
        ),
        chain: chain.clone(),
        group,
        call: inner,
        pending: PendingAfterFirst::Same,
    };
    let (ctx, _) = context(&["--yes", "--json", "--no-mev-shield"])?;
    let error = call::execute_rounds_with(
        &ctx,
        &client,
        &raw,
        "rtao call",
        &factory,
        &Cache::default(),
    )
    .await
    .err()
    .ok_or("continued funding must refuse")?;
    let expected_help = format!(
        "fund {second} (member1) with ≥ τ0.001; earlier approvals may remain pending for call hash {hash}; run `rtao multisig pending` to finish or cancel before retrying"
    );
    assert_eq!(error.code, 1);
    assert!(error.message.contains("this approval costs a ~τ0.0001 fee"));
    assert_eq!(error.help.as_deref(), Some(expected_help.as_str()));
    assert_eq!(factory.factory.acquired.load(Ordering::SeqCst), 1);
    assert_eq!(keys[0].unlocks.load(Ordering::SeqCst), 1);
    assert_eq!(keys[1].unlocks.load(Ordering::SeqCst), 0);
    assert_eq!(chain.mutate_state(|state| state.submissions.len()), 1);
    client.close().await;
    Ok(())
}

#[tokio::test]
async fn chained_approval_encodes_the_authenticated_opening_in_the_actual_second_extrinsic()
-> TestResult {
    let keys = [Keys::new(7)?, Keys::new(8)?];
    let (client, chain) = fixture(&keys).await?;
    let runtime = View::runtime(&client).await?;
    let raw = chained_request(&keys);
    let inner = call::compose(&runtime, &raw)?;
    let group = Multisig::new(
        &runtime,
        &raw.multisig.as_ref().ok_or("group")?.signatories,
        2,
    )?;
    let opening_height = chain.state().lock().map_err(|_| "state")?.block + 1;
    chain.mutate_state(|state| {
        state.queue_submit(SubmitScript::InBlockThenFinalized {
            events: approval_events(keys[0].key.account(), &group, inner.hash(), None),
        });
        state.queue_submit(SubmitScript::InBlockThenFinalized {
            events: success_and_executed(keys[1].key.account(), &group, inner.hash()),
        });
    });
    let factory = ContinuationFactory {
        factory: Factory::new(
            keys.iter()
                .enumerate()
                .map(|(index, key)| (format!("member{index}"), key.clone())),
        ),
        chain: chain.clone(),
        group,
        call: inner,
        pending: PendingAfterFirst::Same,
    };
    let (ctx, _) = context(&["--yes", "--json"])?;
    let receipt = call::execute_rounds_with(
        &ctx,
        &client,
        &raw,
        "rtao call",
        &factory,
        &Cache::default(),
    )
    .await?
    .ok_or("receipt")?;
    assert!(receipt.success);
    assert_eq!(factory.factory.acquired.load(Ordering::SeqCst), 2);
    let submissions = chain
        .state()
        .lock()
        .map_err(|_| "state")?
        .submissions
        .clone();
    assert_eq!(submissions.len(), 2);
    let decoded =
        rtao::core::DynValue::new(runtime.core().decode_extrinsic(&submissions[1], true)?)
            .to_json();
    let args = decoded["call"]["call_args"]
        .as_array()
        .ok_or("as_multi args")?;
    let timepoint = args
        .iter()
        .find(|arg| arg["name"] == json!("maybe_timepoint"))
        .ok_or("timepoint arg")?;
    assert_eq!(
        timepoint["value"],
        json!({"height":opening_height,"index":0})
    );
    assert_eq!(keys[1].unlocks.load(Ordering::SeqCst), 1);
    client.close().await;
    Ok(())
}

#[tokio::test]
async fn unknown_disappeared_or_replaced_opening_refuses_before_the_next_source_or_signature()
-> TestResult {
    for (pending, known_opening) in [
        (PendingAfterFirst::Missing, true),
        (PendingAfterFirst::Replaced, true),
        (PendingAfterFirst::Same, false),
    ] {
        let keys = [Keys::new(7)?, Keys::new(8)?];
        let (client, chain) = fixture(&keys).await?;
        let runtime = View::runtime(&client).await?;
        let raw = chained_request(&keys);
        let inner = call::compose(&runtime, &raw)?;
        let hash = format!("0x{}", hex::encode(inner.hash()));
        let group = Multisig::new(
            &runtime,
            &raw.multisig.as_ref().ok_or("group")?.signatories,
            2,
        )?;
        let mut events = approval_events(keys[0].key.account(), &group, inner.hash(), None);
        if !known_opening {
            events.as_array_mut().ok_or("event list")?.truncate(1);
        }
        chain.mutate_state(|state| {
            state.queue_submit(SubmitScript::InBlockThenFinalized { events })
        });
        let factory = ContinuationFactory {
            factory: Factory::new(
                keys.iter()
                    .enumerate()
                    .map(|(index, key)| (format!("member{index}"), key.clone())),
            ),
            chain: chain.clone(),
            group,
            call: inner,
            pending,
        };
        let (ctx, _) = context(&["--yes", "--json"])?;
        let error = call::execute_rounds_with(
            &ctx,
            &client,
            &raw,
            "rtao call",
            &factory,
            &Cache::default(),
        )
        .await
        .err()
        .ok_or("guard failure")?;
        assert!(
            error.message.contains(if !known_opening {
                "authenticate"
            } else if matches!(pending, PendingAfterFirst::Missing) {
                "absent"
            } else {
                "replaced"
            }),
            "{}",
            error.message
        );
        assert!(
            error
                .help
                .as_deref()
                .is_some_and(|help| help.contains(&hash) && help.contains("rtao multisig pending"))
        );
        assert_eq!(factory.factory.acquired.load(Ordering::SeqCst), 1);
        assert_eq!(keys[1].unlocks.load(Ordering::SeqCst), 0);
        assert_eq!(
            chain.state().lock().map_err(|_| "state")?.submissions.len(),
            1
        );
        client.close().await;
    }
    Ok(())
}

#[tokio::test]
async fn executed_first_approval_stops_later_members_and_caches_exact_inner_call() -> TestResult {
    let keys = vec![Keys::new(7)?, Keys::new(8)?, Keys::new(9)?];
    let (client, chain) = fixture(&keys).await?;
    let runtime = View::runtime(&client).await?;
    let mut raw = request();
    raw.sudo = true;
    raw.proxy_for = Some(AccountId([42; 32]));
    raw.multisig = Some(RawMultisig {
        threshold: 3,
        signatories: keys.iter().map(|keys| keys.key.account()).collect(),
        preset: Some("team".into()),
        refs: keys
            .iter()
            .map(|keys| keys.key.account().to_string())
            .collect(),
    });
    raw.rounds = keys
        .iter()
        .enumerate()
        .map(|(i, key)| SigningSelection {
            display_name: None,
            wallet: format!("member{i}"),
            backend: "wallet".into(),
            address: Some(key.key.account()),
        })
        .collect();
    let composed = call::compose(&runtime, &raw)?;
    let group = Multisig::new(
        &runtime,
        &raw.multisig.as_ref().ok_or("group")?.signatories,
        3,
    )?;
    chain.mutate_state(|state| {
        state.queue_submit(SubmitScript::InBlockThenFinalized {
            events: success_and_executed(keys[0].key.account(), &group, composed.hash()),
        })
    });
    let factory = Factory::new(
        keys.iter()
            .enumerate()
            .map(|(i, key)| (format!("member{i}"), key.clone())),
    );
    let cache = Cache::default();
    let (ctx, sink) = context(&["--yes", "--json"])?;
    let receipt = call::execute_rounds_with(&ctx, &client, &raw, "rtao call", &factory, &cache)
        .await?
        .ok_or("receipt")?;
    assert!(receipt.success);
    assert_eq!(factory.acquired.load(Ordering::SeqCst), 1);
    assert_eq!(
        chain
            .state()
            .lock()
            .map_err(|_| "state capture")?
            .submissions
            .len(),
        1
    );
    assert_eq!(
        receipt.data["multisig_followup"]["status"],
        json!("executed")
    );
    {
        let captured = cache.0.lock().map_err(|_| "cache capture")?;
        let stored = captured
            .get(&format!("0x{}", hex::encode(composed.hash())))
            .ok_or("cache entry")?;
        assert_eq!(
            stored["call_data"],
            json!(format!("0x{}", hex::encode(composed.as_bytes())))
        );
        assert_eq!(stored["sudo"], json!(true));
        assert_eq!(stored["proxy_for"], json!(AccountId([42; 32])));
    }
    assert!(call::multisig_executed(&receipt));
    let unknown_cache = Cache::default();
    let mut spec = MultisigCallSpec::from(&raw);
    spec.target = None;
    let unknown = call::multisig_followup_for_spec(
        &ctx,
        &client,
        MultisigFollowup {
            spec: &spec,
            group: raw.multisig.as_ref().ok_or("raw group")?,
            multisig: &group,
            call: &composed,
            result: &receipt,
        },
        &unknown_cache,
    )
    .await?;
    assert_eq!(unknown["status"], json!("executed"));
    assert_eq!(unknown["target"], Value::Null);
    assert_eq!(
        unknown["call_data"],
        json!(format!("0x{}", hex::encode(composed.as_bytes())))
    );
    assert!(
        unknown_cache
            .0
            .lock()
            .map_err(|_| "unknown cache")?
            .is_empty()
    );
    let pending_unknown = call::pending_multisig_followup(
        &ctx,
        &spec,
        raw.multisig.as_ref().ok_or("raw group")?,
        &group,
        &composed,
        None,
    )?;
    assert_eq!(pending_unknown["commands_available"], json!(false));
    assert_eq!(pending_unknown["co_signer_commands"], json!([]));
    assert_eq!(
        pending_unknown["remaining_signatories"]
            .as_array()
            .map(Vec::len),
        Some(3)
    );
    assert!(
        call::multisig_replay_command(
            &ctx,
            &spec,
            raw.multisig.as_ref().ok_or("raw group")?,
            keys[0].key.account(),
            &BTreeMap::new(),
        )
        .is_err()
    );
    assert_eq!(
        serde_json::from_str::<Value>(&sink.text(Stream::Stdout)?)?,
        receipt.to_json()
    );
    client.close().await;
    Ok(())
}

#[tokio::test]
async fn failed_receipt_is_rendered_once_and_returns_only_an_empty_exit_error() -> TestResult {
    let keys = Keys::new(7)?;
    let (client, chain) = fixture(std::slice::from_ref(&keys)).await?;
    chain.mutate_state(|state| state.queue_submit(SubmitScript::InBlockThenFinalized {
        events: json!([{"phase":"ApplyExtrinsic","extrinsic_idx":0,"module_id":"System","event_id":"ExtrinsicFailed","attributes":{"dispatch_error":{"BadOrigin":null},"dispatch_info":{"weight":{"ref_time":1000,"proof_size":0},"class":"Normal","pays_fee":"Yes"}},"topics":[]}]),
    }));
    let (ctx, sink) = context(&["--yes", "--json"])?;
    let factory = Factory::new([("alice".into(), keys)]);
    let error = call::execute_rounds_with(
        &ctx,
        &client,
        &request(),
        "rtao call",
        &factory,
        &Cache::default(),
    )
    .await
    .err()
    .ok_or("failure exit")?;
    assert_eq!(error.code, 1);
    assert!(error.message.is_empty());
    let rendered: Value = serde_json::from_str(&sink.text(Stream::Stdout)?)?;
    assert_eq!(rendered["success"], json!(false));
    assert!(sink.text(Stream::Stderr)?.is_empty());
    assert_eq!(
        *factory.released.lock().map_err(|_| "release")?,
        vec![false]
    );
    client.close().await;
    Ok(())
}

#[tokio::test]
async fn pending_followup_retains_timepoint_approvals_and_copyable_commands() -> TestResult {
    let keys = vec![Keys::new(7)?, Keys::new(8)?];
    let (client, chain) = fixture(&keys).await?;
    let runtime = View::runtime(&client).await?;
    let mut raw = request();
    raw.multisig = Some(RawMultisig {
        threshold: 2,
        signatories: keys.iter().map(|keys| keys.key.account()).collect(),
        preset: None,
        refs: keys
            .iter()
            .map(|keys| keys.key.account().to_string())
            .collect(),
    });
    raw.rounds = vec![SigningSelection {
        display_name: None,
        wallet: "alice".into(),
        backend: "wallet".into(),
        address: Some(keys[0].key.account()),
    }];
    let call = call::compose(&runtime, &raw)?;
    let group = Multisig::new(
        &runtime,
        &raw.multisig.as_ref().ok_or("group")?.signatories,
        2,
    )?;
    chain.mutate_state(|state|state.seed_python("Multisig","Multisigs",&[json!(group.address),json!(format!("0x{}",hex::encode(call.hash())))],json!({"when":{"height":10,"index":1},"deposit":0,"depositor":keys[0].key.account(),"approvals":[keys[0].key.account()]})))?;
    let (ctx, sink) = context(&["--yes"])?;
    let factory = Factory::new([("alice".into(), keys[0].clone())]);
    let receipt = call::execute_rounds_with(
        &ctx,
        &client,
        &raw,
        "rtao call",
        &factory,
        &Cache::default(),
    )
    .await?
    .ok_or("receipt")?;
    let followup = &receipt.data["multisig_followup"];
    assert_eq!(followup["status"], json!("pending"));
    assert_eq!(followup["timepoint"], json!({"height":10,"index":1}));
    assert_eq!(followup["approvals"], json!(1));
    assert_eq!(
        followup["remaining_signatories"],
        json!([keys[1].key.account()])
    );
    assert_eq!(followup["commands_available"], json!(true));
    assert!(
        followup["co_signer_commands"][0]["command"]
            .as_str()
            .ok_or("command")?
            .contains("--multisig-threshold 2")
    );
    let human = sink.text(Stream::Stdout)?;
    let command = followup["co_signer_commands"][0]["command"]
        .as_str()
        .ok_or("copyable command")?;
    assert!(human.contains(rtao_cli::output::terminal_text(command).as_ref()));
    assert_eq!(
        human
            .matches("--macos-password or --keychain-password")
            .count(),
        1
    );
    assert!(human.contains("--signer vault (or ledger/extension) before call"));
    let json_sink = MemorySink::default();
    let machine = Output::new(
        OutputOptions {
            json_mode: true,
            quiet: false,
            network: None,
        },
        Arc::new(json_sink.clone()),
    );
    assert!(machine.result(&receipt, "unused human summary")?);
    assert_eq!(
        serde_json::from_str::<Value>(&json_sink.text(Stream::Stdout)?)?,
        receipt.to_json()
    );
    // Arm the refusal only after the real executor returns its receipt and
    // releases the source. Retry help must also identify the reviewed call.
    let refused_keys = vec![Keys::new(7)?, Keys::new(8)?];
    let (refused_client, refused_chain) = fixture(&refused_keys).await?;
    let (refused_ctx, refused_sink) = context(&["--yes", "--json"])?;
    let inner = Factory::new([("alice".into(), refused_keys[0].clone())]);
    let policy = rtao_rpc::policy::classify_rpc_error(&json!({
        "code":-32005,"message":"slow down",
        "data":{"policy":"rate","retry_after":"12"},
    }))
    .ok_or("typed followup policy")?;
    let expected_message = policy.to_string();
    let factory = RefuseFollowupAfterRelease {
        inner: &inner,
        chain: &refused_chain,
        policy,
    };
    let error = call::execute_rounds_with(
        &refused_ctx,
        &refused_client,
        &raw,
        "rtao call",
        &factory,
        &Cache::default(),
    )
    .await
    .err()
    .ok_or("followup policy refusal was accepted")?;
    assert_eq!(error.code, 1);
    assert_eq!(error.message, expected_message);
    let help = error.help.as_deref().ok_or("followup recovery help")?;
    assert!(help.contains("wait 12 seconds, then retry"));
    assert!(help.contains("the transaction was already submitted"));
    assert!(help.contains("inspect that result before retrying"));
    let submissions = refused_chain.mutate_state(|state| state.submissions.clone());
    assert_eq!(submissions.len(), 1);
    assert!(help.contains(&format!("call hash 0x{}", hex::encode(call.hash()))));
    assert_eq!(inner.acquired.load(Ordering::SeqCst), 1);
    assert_eq!(*inner.released.lock().map_err(|_| "release")?, vec![true]);
    assert_eq!(refused_keys[0].unlocks.load(Ordering::SeqCst), 1);
    assert_eq!(
        refused_chain
            .requests()
            .iter()
            .filter(|request| request.method.starts_with("author_submit"))
            .count(),
        1
    );
    assert!(refused_sink.bytes(Stream::Stdout)?.is_empty());
    assert!(refused_sink.bytes(Stream::Stderr)?.is_empty());
    refused_client.close().await;
    client.close().await;
    Ok(())
}

#[tokio::test]
async fn source_mismatch_refuses_before_unlock_and_still_releases_external_session() -> TestResult {
    let keys = Keys::new(7)?;
    let (client, chain) = fixture(std::slice::from_ref(&keys)).await?;
    let (ctx, _) = context(&["--yes"])?;
    let factory = Factory::new([("alice".into(), keys.clone())]);
    let mut raw = request();
    raw.rounds[0].address = Some(AccountId([123; 32]));
    raw.rounds[0].backend = "ledger".into();
    assert!(
        call::execute_rounds_with(
            &ctx,
            &client,
            &raw,
            "rtao call",
            &factory,
            &Cache::default()
        )
        .await
        .is_err()
    );
    assert_eq!(keys.unlocks.load(Ordering::SeqCst), 0);
    assert_eq!(
        *factory.released.lock().map_err(|_| "release capture")?,
        vec![false]
    );
    assert!(
        chain
            .state()
            .lock()
            .map_err(|_| "state capture")?
            .submissions
            .is_empty()
    );
    client.close().await;
    Ok(())
}

static NEXT_TEMP: AtomicUsize = AtomicUsize::new(0);
struct Temp(PathBuf);
impl Temp {
    fn new() -> std::io::Result<Self> {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../../proof/raw-call-fixtures")
            .join(format!(
                "{}-{}",
                std::process::id(),
                NEXT_TEMP.fetch_add(1, Ordering::Relaxed)
            ));
        std::fs::create_dir_all(&root)?;
        Ok(Self(root))
    }
}
impl Drop for Temp {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[tokio::test]
async fn args_file_and_shared_cache_preserve_other_entries_and_cleanup_replacement_files()
-> TestResult {
    let temp = Temp::new()?;
    let args = temp.0.join("args.json");
    std::fs::write(&args, "{\"remark\":\"0x0102\"}")?;
    assert_eq!(
        call::load_params(None, Some(&args))?,
        serde_json::Map::from_iter([("remark".into(), json!("0x0102"))])
    );
    assert_eq!(
        call::load_params(Some(""), Some(&args))?,
        call::load_params(None, Some(&args))?
    );
    let paths = rtao::client::config::Paths::new(&temp.0, |name| {
        (name == "BTCLI_MULTISIG_CACHE").then(|| temp.0.join("cache.json").into_os_string())
    });
    let path = paths.path(rtao::client::config::ConfigFile::MultisigCache);
    std::fs::write(path, "{\"0xold\":{\"target\":\"System.remark\"}}")?;
    let cache = FsRawCallCache::new(&paths);
    let hash = hex::encode(rtao::core::codec::storage::hash_param(
        "Blake2_256",
        &[1, 2],
    )?);
    cache
        .save(
            hash.clone(),
            json!({"target":"System.remark","params":{"remark":"0x0102"},"call_data":"0x0102"}),
        )
        .await?;
    let saved: Value = serde_json::from_slice(&std::fs::read(path)?)?;
    assert_eq!(saved["0xold"], json!({"target":"System.remark"}));
    assert_eq!(
        saved[format!("0x{hash}")]["params"],
        json!({"remark":"0x0102"})
    );
    assert_eq!(std::fs::read_dir(&temp.0)?.count(), 2);
    assert!(call::load_params(Some("{}"), Some(&args)).is_err());
    Ok(())
}

#[test]
fn raw_nested_names_register_labels_kinds_and_public_resolution_messages() -> TestResult {
    let coldkey = AccountId([7; 32]);
    let hotkey = AccountId([8; 32]);
    let raw = AccountId([9; 32]);
    let args = json!({"calls":[
        {"Balances":{"transfer_keep_alive":{"dest":"contact","value":1}}},
        {"SubtensorModule":{"add_stake":{"hotkey":"operator","netuid":1,"amount_staked":1}}},
        {"Balances":{"transfer_keep_alive":{"dest":raw,"value":1}}}
    ]})
    .to_string();
    for mode in [None, Some("--json"), Some("--quiet")] {
        let mut flags = vec!["--yes"];
        flags.extend(mode);
        let (mut ctx, sink) = context(&flags)?;
        ctx.books = Books::from_values(
            json!([{"name":"contact","address":coldkey},{"name":"operator","address":hotkey}]),
            json!([]),
            json!([]),
        );
        let mut argv = vec![
            "rtao",
            "call",
            "Utility.batch_all",
            "--args",
            &args,
            "--yes",
        ];
        argv.extend(mode);
        let parsed = command_tree([call::command()]).try_get_matches_from(argv)?;
        let request = RawCallRequest::from_matches(&ctx, parsed.subcommand().ok_or("call")?.1)?;
        assert_eq!(
            request.params["calls"][0]["Balances"]["transfer_keep_alive"]["dest"],
            json!(coldkey)
        );
        assert_eq!(
            request.params["calls"][1]["SubtensorModule"]["add_stake"]["hotkey"],
            json!(hotkey)
        );
        assert_eq!(
            request.params["calls"][2]["Balances"]["transfer_keep_alive"]["dest"],
            json!(raw)
        );
        assert_eq!(
            ctx.output
                .address_names()?
                .get(&coldkey.to_string())
                .map(String::as_str),
            Some("contact")
        );
        assert_eq!(
            ctx.output
                .address_names()?
                .get(&hotkey.to_string())
                .map(String::as_str),
            Some("operator")
        );
        assert_eq!(
            ctx.output.address_kinds()?.get(&coldkey.to_string()),
            Some(&AccountKind::Coldkey)
        );
        assert_eq!(
            ctx.output.address_kinds()?.get(&hotkey.to_string()),
            Some(&AccountKind::Hotkey)
        );
        assert!(!ctx.output.address_kinds()?.contains_key(&raw.to_string()));
        assert!(sink.bytes(Stream::Stdout)?.is_empty());
        assert_eq!(
            sink.text(Stream::Stderr)?,
            if mode.is_none() {
                format!(
                    "dest: resolved address-book entry 'contact' to {coldkey}\nhotkey: resolved address-book entry 'operator' to {hotkey}\n"
                )
            } else {
                String::new()
            }
        );
    }
    Ok(())
}

#[test]
fn raw_dry_run_truncates_only_top_level_long_unicode_strings() {
    let string = "α".repeat(90);
    let params = serde_json::Map::from_iter([
        ("code".into(), json!(string)),
        ("nested".into(), json!({"code":string})),
    ]);
    let shown = call::for_display(&params);
    assert_eq!(
        shown["code"],
        json!(format!("{}… (+10 chars)", "α".repeat(80)))
    );
    assert_eq!(shown["nested"], params["nested"]);
}

struct ScriptedTerminal {
    answers: std::collections::VecDeque<String>,
    messages: Arc<Mutex<Vec<String>>>,
}
impl Terminal for ScriptedTerminal {
    fn stdin_is_tty(&self) -> bool {
        true
    }
    fn stderr_is_tty(&self) -> bool {
        true
    }
    fn read_line(&mut self, _: &str) -> Result<String, PromptError> {
        self.answers.pop_front().ok_or(PromptError::Aborted)
    }
    fn read_secret(&mut self, _: &str) -> Result<rtao::wallet::SecretString, PromptError> {
        Err(PromptError::Unavailable)
    }
    fn message(&mut self, text: &str) -> Result<(), PromptError> {
        self.messages
            .lock()
            .map_err(|_| PromptError::Io)?
            .push(text.into());
        Ok(())
    }
}

#[test]
fn unknown_external_backend_reasks_and_preserves_both_contact_tags_across_rounds() -> TestResult {
    let temp = Temp::new()?;
    let alice = AccountId([7; 32]);
    let bob = AccountId([8; 32]);
    let all = format!("{alice},{bob}");
    let flags = [
        "--multisig-threshold",
        "2",
        "--signatories",
        all.as_str(),
        "--signatory",
        "alice",
        "--signatory",
        "bob",
    ];
    let (mut ctx, _) = context(&flags)?;
    ctx.books = Books::from_values(
        json!([{"name":"alice","address":alice,"note":"retain first"},{"name":"bob","address":bob,"note":"retain second"},{"name":"other","address":AccountId([9;32]),"signer":"vault"}]),
        json!([]),
        json!([]),
    );
    let paths = rtao::client::config::Paths::new(&temp.0, |name| {
        (name == "BTCLI_ADDRESSES_PATH").then(|| temp.0.join("addresses.json").into_os_string())
    });
    let address_path = paths
        .path(rtao::client::config::ConfigFile::Addresses)
        .to_owned();
    std::fs::write(&address_path, serde_json::to_vec(&ctx.books.addresses)?)?;
    ctx.config_store = Some(rtao::client::config::Store::new(paths));
    let messages = Arc::new(Mutex::new(Vec::new()));
    ctx.terminal = Arc::new(Mutex::new(Box::new(ScriptedTerminal {
        answers: ["wrong".into(), "ledger".into(), "extension".into()].into(),
        messages: messages.clone(),
    })));
    let argv = vec!["rtao", "call", "System.remark"]
        .into_iter()
        .chain(flags);
    let parsed = command_tree([call::command()]).try_get_matches_from(argv)?;
    let request = RawCallRequest::from_matches(&ctx, parsed.subcommand().ok_or("call")?.1)?;
    assert_eq!(
        request
            .rounds
            .iter()
            .map(|round| round.backend.as_str())
            .collect::<Vec<_>>(),
        vec!["ledger", "extension"]
    );
    assert_eq!(
        request
            .rounds
            .iter()
            .filter_map(|round| round.address)
            .collect::<Vec<_>>(),
        vec![alice, bob]
    );
    let saved: Vec<Value> = serde_json::from_slice(&std::fs::read(address_path)?)?;
    assert_eq!(saved.len(), 3);
    assert_eq!(
        saved
            .iter()
            .find(|entry| entry["name"] == json!("alice"))
            .ok_or("alice")?["signer"],
        json!("ledger")
    );
    assert_eq!(
        saved
            .iter()
            .find(|entry| entry["name"] == json!("bob"))
            .ok_or("bob")?["signer"],
        json!("extension")
    );
    assert_eq!(
        saved
            .iter()
            .find(|entry| entry["name"] == json!("other"))
            .ok_or("other")?["signer"],
        json!("vault")
    );
    assert!(
        messages
            .lock()
            .map_err(|_| "messages")?
            .iter()
            .any(|text| text.contains("choose one of"))
    );
    assert_eq!(std::fs::read_dir(&temp.0)?.count(), 1);
    Ok(())
}

#[test]
fn signatory_without_multisig_and_unavailable_backend_refuse_without_prompting() -> TestResult {
    let (ctx, _) = context(&["--signatory", "unknown"])?;
    let parsed = command_tree([call::command()]).try_get_matches_from([
        "rtao",
        "call",
        "System.remark",
        "--signatory",
        "unknown",
    ])?;
    let error = RawCallRequest::from_matches(&ctx, parsed.subcommand().ok_or("call")?.1)
        .err()
        .ok_or("signatory misuse")?;
    assert_eq!(error.code, 2);
    assert!(error.message.contains("only applies"));
    let all = format!("{},{}", AccountId([7; 32]), AccountId([8; 32]));
    let flags = [
        "--json",
        "--multisig-threshold",
        "2",
        "--signatories",
        all.as_str(),
        "--signatory",
        "alice",
    ];
    let (mut ctx, _) = context(&flags)?;
    ctx.books = Books::from_values(
        json!([{"name":"alice","address":AccountId([7;32])}]),
        json!([]),
        json!([]),
    );
    let parsed = command_tree([call::command()]).try_get_matches_from(
        vec!["rtao", "call", "System.remark"]
            .into_iter()
            .chain(flags),
    )?;
    assert!(RawCallRequest::from_matches(&ctx, parsed.subcommand().ok_or("call")?.1).is_err());
    Ok(())
}

#[test]
fn multisig_alias_labels_and_rounds_use_public_natural_wallet_order() -> TestResult {
    let alice = AccountId([7; 32]);
    let bob = AccountId([8; 32]);
    for aliases in [["wallet10", "wallet2"], ["wallet2", "wallet10"]] {
        let temp = Temp::new()?;
        let wallet_root = temp.0.join("wallets");
        for (name, account) in [(aliases[0], alice), (aliases[1], alice), ("wallet3", bob)] {
            let directory = wallet_root.join(name);
            std::fs::create_dir_all(&directory)?;
            std::fs::write(
                directory.join("coldkeypub.txt"),
                serde_json::to_vec(&json!({"ss58Address":account,"cryptoType":1}))?,
            )?;
            // Default planning inspects only file existence. This sentinel is
            // public fixture text, never parsed or passed to a signing source.
            std::fs::write(
                directory.join("coldkey"),
                b"public fixture; not a private key",
            )?;
        }
        let (mut ctx, _) = context(&["--yes"])?;
        ctx.globals.wallet_path = wallet_root;
        ctx.globals.wallet_name = "team".into();
        let group = RawMultisig {
            threshold: 2,
            signatories: vec![bob, alice],
            preset: None,
            refs: vec![bob.to_string(), alice.to_string()],
        };
        let labels = call::signatory_labels(&ctx, &group)?;
        assert_eq!(labels.get(&alice).map(String::as_str), Some("wallet2"));
        assert_eq!(labels.get(&bob).map(String::as_str), Some("wallet3"));
        let rounds = call::plan_rounds(&ctx, Some(&group), SignerRole::Coldkey, None)?;
        assert_eq!(
            rounds
                .iter()
                .map(|round| round.wallet.as_str())
                .collect::<Vec<_>>(),
            vec!["wallet3", "wallet2"]
        );
        assert_eq!(
            rounds
                .iter()
                .filter_map(|round| round.address)
                .collect::<Vec<_>>(),
            vec![bob, alice]
        );
        assert!(rounds.iter().all(|round| round.backend == "wallet"));
        assert_eq!(
            rounds
                .iter()
                .map(SigningSelection::display_wallet)
                .collect::<Vec<_>>(),
            vec!["wallet3", "wallet10"]
        );
        ctx.globals.signatories = vec![alice.to_string()];
        assert_eq!(
            call::select_member(&ctx, &group, SignerRole::Coldkey, None)?.wallet,
            "wallet2"
        );
        ctx.globals.signatories = vec![alice.to_string(), bob.to_string()];
        assert_eq!(
            call::plan_rounds(&ctx, Some(&group), SignerRole::Coldkey, None)?
                .iter()
                .map(|round| round.wallet.as_str())
                .collect::<Vec<_>>(),
            vec!["wallet2", "wallet3"]
        );
        let spec = MultisigCallSpec::from(&request());
        let replay = call::multisig_replay_command(&ctx, &spec, &group, alice, &labels)?;
        assert!(replay.contains("--other-signatories wallet3 -w wallet2"));
    }
    Ok(())
}

#[test]
fn automatic_multisig_aliases_use_last_admission_and_display_but_first_wallet_source() -> TestResult
{
    let alice = AccountId([7; 32]);
    let bob = AccountId([8; 32]);
    for equal_names in [false, true] {
        for first_secret in [false, true] {
            for last_secret in [false, true] {
                let temp = Temp::new()?;
                let wallet_root = temp.0.join("wallets");
                let alias_names = if equal_names {
                    ["wallet02", "wallet2"]
                } else {
                    ["wallet2", "wallet10"]
                };
                for (name, account) in [
                    (alias_names[1], alice),
                    ("wallet3", bob),
                    (alias_names[0], alice),
                ] {
                    let directory = wallet_root.join(name);
                    std::fs::create_dir_all(&directory)?;
                    std::fs::write(
                        directory.join("coldkeypub.txt"),
                        serde_json::to_vec(&json!({"ss58Address":account,"cryptoType":1}))?,
                    )?;
                }
                // Equal natural names keep incoming public directory order;
                // no lexical tie-breaker or private key content is consulted.
                let lookup = rtao_cli::books::FsAccountLookup::new(&wallet_root)?;
                let candidates = rtao_cli::books::AccountLookup::candidates(&lookup);
                let natural_aliases = if equal_names {
                    candidates
                        .iter()
                        .filter(|entry| {
                            entry.source == "wallet" && entry.address == alice.to_string()
                        })
                        .map(|entry| entry.name.clone())
                        .collect::<Vec<_>>()
                } else {
                    alias_names
                        .iter()
                        .map(|name| (*name).to_owned())
                        .collect::<Vec<_>>()
                };
                assert_eq!(natural_aliases.len(), 2);
                let first = &natural_aliases[0];
                let last = &natural_aliases[1];
                for (name, exists) in [
                    (first.as_str(), first_secret),
                    (last.as_str(), last_secret),
                    ("wallet3", true),
                ] {
                    if exists {
                        std::fs::write(
                            wallet_root.join(name).join("coldkey"),
                            b"existence-only public fixture",
                        )?;
                    }
                }
                let (mut ctx, sink) = context(&["--yes"])?;
                ctx.globals.wallet_path = wallet_root;
                ctx.globals.wallet_name = "team".into();
                let group = RawMultisig {
                    threshold: 2,
                    signatories: vec![alice, bob],
                    preset: None,
                    refs: vec![alice.to_string(), bob.to_string()],
                };
                assert_eq!(
                    call::signatory_labels(&ctx, &group)?.get(&alice),
                    Some(first)
                );
                let automatic = call::plan_rounds(&ctx, Some(&group), SignerRole::Coldkey, None);
                if last_secret {
                    let rounds = automatic?;
                    assert_eq!(rounds.len(), 2);
                    assert_eq!(rounds[0].wallet, *first);
                    assert_eq!(rounds[0].display_wallet(), last.as_str());
                    assert_eq!(rounds[0].address, Some(alice));
                    assert_eq!(rounds[1].wallet, "wallet3");
                } else {
                    let error = automatic
                        .err()
                        .ok_or("last public-only alias must disable automatic local round")?;
                    assert!(error.message.contains("multiple local signatories"));
                    assert!(!sink.text(Stream::Stderr)?.contains("approvals planned"));
                }
                ctx.globals.signatories = vec![last.clone()];
                assert_eq!(
                    call::select_member(&ctx, &group, SignerRole::Coldkey, None)?.wallet,
                    *last
                );
                ctx.globals.signatories = vec![last.clone(), bob.to_string()];
                let chained = call::plan_rounds(&ctx, Some(&group), SignerRole::Coldkey, None)?;
                assert_eq!(chained[0].wallet, *first);
                assert_eq!(chained[0].display_wallet(), last.as_str());
            }
        }
    }
    Ok(())
}

#[test]
fn interactive_multisig_alias_selection_keeps_chosen_wallet_source() -> TestResult {
    let alice = AccountId([7; 32]);
    let bob = AccountId([8; 32]);
    for use_rounds_planner in [false, true] {
        for collision in [
            "none",
            "contact-member",
            "contact-nonmember",
            "proxy",
            "saved-multisig",
        ] {
            let temp = Temp::new()?;
            let wallet_root = temp.0.join("wallets");
            for name in ["wallet10", "wallet2"] {
                let directory = wallet_root.join(name);
                std::fs::create_dir_all(&directory)?;
                std::fs::write(
                    directory.join("coldkeypub.txt"),
                    serde_json::to_vec(&json!({"ss58Address":alice,"cryptoType":1}))?,
                )?;
            }
            let (mut ctx, _) = context(&[])?;
            ctx.globals.wallet_path = wallet_root.clone();
            ctx.globals.wallet_name = "team".into();
            ctx.books = Books::from_values(
                if collision.starts_with("contact") {
                    json!([{"name":"wallet10","address":if collision == "contact-member" { bob } else { AccountId([9;32]) }}])
                } else {
                    json!([])
                },
                if collision == "proxy" {
                    json!([{"name":"wallet10","address":bob}])
                } else {
                    json!([])
                },
                if collision == "saved-multisig" {
                    json!([{"name":"wallet10","threshold":1,"signatories":[alice,bob]}])
                } else {
                    json!([])
                },
            );
            ctx.terminal = Arc::new(Mutex::new(Box::new(ScriptedTerminal {
                answers: ["wallet10".into()].into(),
                messages: Arc::new(Mutex::new(Vec::new())),
            })));
            let group = RawMultisig {
                threshold: 1,
                signatories: vec![alice, bob],
                preset: None,
                refs: vec![alice.to_string(), bob.to_string()],
            };
            let selected = if use_rounds_planner {
                let rounds = call::plan_rounds(&ctx, Some(&group), SignerRole::Coldkey, None)?;
                assert_eq!(rounds.len(), 1);
                rounds.into_iter().next().ok_or("missing selected round")?
            } else {
                call::select_member(&ctx, &group, SignerRole::Coldkey, None)?
            };
            assert_eq!(selected.wallet, "wallet10");
            assert_eq!(selected.display_wallet(), "wallet10");
            assert_eq!(selected.address, Some(alice));
            assert!(!wallet_root.join("wallet2").join("coldkey").exists());
            assert!(!wallet_root.join("wallet10").join("coldkey").exists());
            ctx.globals.signatories = vec!["wallet10".into()];
            let explicit = call::select_member(&ctx, &group, SignerRole::Coldkey, None)?;
            assert_eq!(explicit.wallet, "wallet10");
            assert_eq!(explicit.address, Some(alice));
        }
    }
    Ok(())
}

#[test]
fn chained_multisig_references_use_only_member_contacts_and_local_names() -> TestResult {
    let alice = AccountId([7; 32]);
    let bob = AccountId([8; 32]);
    let (saved_account, _) = rtao::core::extrinsic::multisig_account_id(&[alice.0, bob.0], 1)?;
    let saved_account = AccountId(saved_account);
    for mode in [
        "contact-member",
        "contact-nonmember",
        "proxy-shadow",
        "saved-shadow",
        "proxy-only",
        "saved-only",
    ] {
        let temp = Temp::new()?;
        let wallet_root = temp.0.join("wallets");
        for (name, address) in [("wallet2", alice), ("wallet10", alice), ("wallet3", bob)] {
            let directory = wallet_root.join(name);
            std::fs::create_dir_all(&directory)?;
            std::fs::write(
                directory.join("coldkeypub.txt"),
                serde_json::to_vec(&json!({"ss58Address":address,"cryptoType":1}))?,
            )?;
        }
        let reference = if mode.ends_with("only") {
            "book-only"
        } else {
            "wallet10"
        };
        let (mut ctx, _) = context(&["--yes"])?;
        ctx.globals.wallet_path = wallet_root;
        ctx.globals.wallet_name = "team".into();
        ctx.books = Books::from_values(
            if mode.starts_with("contact") {
                json!([{"name":reference,"address":if mode=="contact-member" {bob} else {AccountId([9;32])}}])
            } else {
                json!([])
            },
            if mode.starts_with("proxy") {
                json!([{"name":reference,"address":bob}])
            } else {
                json!([])
            },
            if mode.starts_with("saved") {
                json!([{"name":reference,"threshold":1,"signatories":[alice,bob]}])
            } else {
                json!([])
            },
        );
        // The forbidden book-only aliases resolve to actual members under the
        // broader generic resolver, and forced signing removes backend refusal.
        let signatories = if mode == "saved-only" {
            vec![alice, bob, saved_account]
        } else {
            vec![alice, bob]
        };
        let group = RawMultisig {
            threshold: 2,
            refs: signatories.iter().map(ToString::to_string).collect(),
            signatories,
            preset: None,
        };
        let contact_first = mode == "contact-member";
        ctx.globals.signatories = vec![
            if mode.ends_with("only") {
                format!("{reference}=ledger")
            } else {
                reference.into()
            },
            if contact_first || mode.ends_with("only") {
                alice.to_string()
            } else {
                bob.to_string()
            },
        ];
        let rounds = call::plan_rounds(&ctx, Some(&group), SignerRole::Coldkey, None);
        if mode.ends_with("only") {
            let error = rounds
                .err()
                .ok_or("proxy/saved-only member name must refuse")?;
            assert!(error.message.contains("not a member"));
        } else {
            let rounds = rounds?;
            assert_eq!(rounds.len(), 2);
            assert_eq!(
                rounds[0].address,
                Some(if contact_first { bob } else { alice })
            );
            assert_eq!(
                rounds[0].wallet,
                if contact_first { "wallet3" } else { "wallet2" }
            );
            assert_eq!(rounds[0].display_wallet(), "wallet10");
        }
    }
    Ok(())
}
