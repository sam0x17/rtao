//! Wallet parity exercises actual command dispatch, keyfile policy and fake RPC.

use rtao::{
    client::{Client, ClientConfig, Network},
    core::keys::Keypair,
    primitives::{AccountId, CryptoType},
    wallet::{ExposeSecret, PasswordSources, RegenerateFrom, SecretString, Wallet, WriteOptions},
};
use rtao_cli::{
    application,
    books::Books,
    commands::wallet,
    context::AppContext,
    globals::Globals,
    output::{MemorySink, Output, OutputOptions, Stream},
    prompt::{PromptError, PromptHistory, Terminal},
};
use rtao_testkit::{FakeChain, FakeState, rpc::RpcClient};
use serde_json::{Value, json};
use std::{
    error::Error,
    fs,
    path::PathBuf,
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
};

type TestResult = Result<(), Box<dyn Error>>;

#[tokio::test]
async fn nonpositive_history_limit_returns_exact_empty_json_without_chain_or_http() -> TestResult {
    let root = Root::new()?;
    root.public("alice", COLD)?;
    for limit in ["0", "-1"] {
        let (parsed, mut ctx, sink) = context(
            &root,
            &["wallet", "history", "--network", "finney", "--limit", limit],
        )?;
        local(&mut ctx, &parsed).await?;
        assert_eq!(value(&sink)?, json!([]));
    }
    let (parsed, mut ctx, sink) = context(
        &root,
        &["wallet", "history", "--network", "test", "--limit", "0"],
    )?;
    let error = wallet::run(&mut ctx, group(&parsed)?, &mut PromptHistory::default())
        .await
        .err()
        .ok_or("history must refuse non-mainnet")?;
    assert_eq!(error.code, 1);
    assert!(error.message.contains("not indexed for network"));
    assert!(sink.text(Stream::Stdout)?.is_empty());
    Ok(())
}

#[tokio::test]
async fn encrypted_hotkey_decrypt_prompts_without_inheriting_coldkey_password_sources() -> TestResult
{
    use rtao::wallet::{PasswordPrompt, PromptPolicy, WalletError};
    use std::sync::atomic::AtomicUsize;
    struct Password {
        calls: AtomicUsize,
    }
    impl PasswordPrompt for Password {
        fn is_terminal(&self) -> bool {
            true
        }
        fn prompt(&self, _: &str) -> Result<SecretString, WalletError> {
            self.calls.fetch_add(1, Ordering::Relaxed);
            Ok(SecretString::new("hotkey-fixture-password".into()))
        }
    }
    let root = Root::new()?;
    let wallet = Wallet::new("alice", "default", &root.0)?;
    let account = wallet
        .regenerate_hotkey(
            RegenerateFrom::Seed(zeroize::Zeroizing::new(vec![44; 32])),
            CryptoType::Ed25519,
            WriteOptions {
                encrypt: true,
                password: Some(SecretString::new("hotkey-fixture-password".into())),
                overwrite: rtao::wallet::Overwrite::No,
            },
        )
        .await?;
    let encrypted =
        rtao::wallet::encrypt_message("hotkey private message", &account.to_string()).await?;
    let (parsed, mut ctx, sink) = context(
        &root,
        &[
            "wallet",
            "decrypt",
            "--ciphertext",
            &encrypted.ciphertext,
            "--use-hotkey",
        ],
    )?;
    let password = Arc::new(Password {
        calls: AtomicUsize::new(0),
    });
    ctx.passwords = PasswordSources {
        password: Some(SecretString::new(
            "coldkey-password-must-not-be-used".into(),
        )),
        password_file: Some(root.0.join("missing-coldkey-password-file")),
        keychain: true,
        macos_prompt: true,
        prompt: PromptPolicy::Always,
        prompt_callback: Some(password.clone()),
        ..Default::default()
    };
    local(&mut ctx, &parsed).await?;
    assert_eq!(password.calls.load(Ordering::Relaxed), 1);
    assert_eq!(value(&sink)?, json!({"message":"hotkey private message"}));
    Ok(())
}

#[tokio::test]
async fn identity_edit_preserves_omitted_current_fields_and_validates_before_secret_acquisition()
-> TestResult {
    let root = Root::new()?;
    root.public("alice", COLD)?;
    let mut state = FakeState::default();
    state.seed_python(
        "System",
        "Account",
        &[json!(COLD)],
        json!({"data":{"free":10_000_000_000u64}}),
    )?;
    state.seed_python(
        "SubtensorModule",
        "IdentitiesV2",
        &[json!(COLD)],
        json!({"name":"Before","url":"https://existing.example","description":"keep me"}),
    )?;
    let (client, chain) = client(state).await?;
    let (parsed, mut ctx, sink) = context(
        &root,
        &[
            "wallet",
            "set-identity",
            "--name",
            "After",
            "--dry-run",
            "--yes",
        ],
    )?;
    wallet::run_with_client(
        &mut ctx,
        &client,
        group(&parsed)?,
        &mut PromptHistory::default(),
    )
    .await?;
    let plan = value(&sink)?;
    assert_eq!(plan["args"]["name"], "After");
    assert_eq!(plan["args"]["url"], "https://existing.example");
    assert_eq!(plan["args"]["description"], "keep me");
    assert_eq!(plan["args"]["additional"], "");
    assert!(
        chain
            .state()
            .lock()
            .map_err(|_| "state poisoned")?
            .submissions
            .is_empty()
    );
    assert_eq!(
        fs::read(root.0.join("alice/coldkey"))?,
        b"private-key sentinel must not be opened by reads or plans"
    );
    client.close().await;
    Ok(())
}
const COLD: AccountId = AccountId([7; 32]);
const HOT: AccountId = AccountId([8; 32]);
const OWNED: AccountId = AccountId([9; 32]);

struct Root(PathBuf);
impl Root {
    fn new() -> Result<Self, Box<dyn Error>> {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let root = PathBuf::from(
            std::env::var_os("RTAO_TEMP_ROOT").ok_or("hosted tests require RTAO_TEMP_ROOT")?,
        );
        let parent = root.join("tmp/m8-wallet");
        fs::create_dir_all(&parent)?;
        let path = parent.join(format!(
            "run-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&path)?;
        Ok(Self(path))
    }
    fn public(&self, name: &str, account: AccountId) -> TestResult {
        let dir = self.0.join(name);
        fs::create_dir_all(&dir)?;
        fs::write(
            dir.join("coldkeypub.txt"),
            serde_json::to_vec(&json!({"ss58Address":account,"cryptoType":1}))?,
        )?;
        fs::write(
            dir.join("coldkey"),
            b"private-key sentinel must not be opened by reads or plans",
        )?;
        Ok(())
    }
}
impl Drop for Root {
    fn drop(&mut self) {
        // These fixtures spawn no children and install no persistent state.
        // Only this exact task-owned child is removed after all async work.
        if self.0.file_name().is_some_and(|n| {
            n.to_string_lossy()
                .starts_with(&format!("run-{}-", std::process::id()))
        }) && !self.0.is_symlink()
        {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
}
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
    fn read_secret(&mut self, _: &str) -> Result<SecretString, PromptError> {
        Err(PromptError::Unavailable)
    }
    fn message(&mut self, _: &str) -> Result<(), PromptError> {
        Ok(())
    }
}
fn context(
    root: &Root,
    args: &[&str],
) -> Result<(clap::ArgMatches, AppContext, MemorySink), Box<dyn Error>> {
    let mut argv = vec![
        "rtao".to_owned(),
        "--json".into(),
        "--wallet-path".into(),
        root.0.to_string_lossy().into_owned(),
        "--wallet".into(),
        "alice".into(),
    ];
    argv.extend(args.iter().map(|s| s.to_string()));
    let parsed = application::command_tree([wallet::command()]).try_get_matches_from(argv)?;
    let leaf = parsed
        .subcommand()
        .ok_or("wallet group")?
        .1
        .subcommand()
        .ok_or("wallet leaf")?
        .1;
    let mut globals = Globals::resolve(&parsed, &Default::default(), &root.0, |_| None)?;
    globals.apply_with_env(leaf, &[], |_| None)?;
    let mut ctx = AppContext::new(
        globals,
        Default::default(),
        Books::default(),
        Arc::new(Mutex::new(Box::new(NoTerminal))),
    );
    let sink = MemorySink::default();
    ctx.output = Output::with_sink(
        OutputOptions {
            json_mode: true,
            quiet: false,
            network: None,
        },
        Arc::new(sink.clone()),
    );
    ctx.passwords = PasswordSources::default();
    Ok((parsed, ctx, sink))
}
fn group(parsed: &clap::ArgMatches) -> Result<&clap::ArgMatches, Box<dyn Error>> {
    Ok(parsed.subcommand().ok_or("wallet group")?.1)
}
fn value(sink: &MemorySink) -> Result<Value, Box<dyn Error>> {
    Ok(serde_json::from_str(&sink.text(Stream::Stdout)?)?)
}
async fn client(state: FakeState) -> Result<(Client, FakeChain), Box<dyn Error>> {
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
async fn local(ctx: &mut AppContext, parsed: &clap::ArgMatches) -> TestResult {
    wallet::run(ctx, group(parsed)?, &mut PromptHistory::default()).await?;
    Ok(())
}

#[test]
fn all_sixteen_routes_and_hidden_group_command_aliases_parse() -> TestResult {
    let address = COLD.to_string();
    let tree = application::command_tree([wallet::command()]);
    for argv in [
        vec!["sign", "--message", "hello"],
        vec![
            "verify",
            "--message",
            "hello",
            "--signature",
            "00",
            "--ss58",
            &address,
        ],
        vec!["encrypt", "--message", "secret", "--recipient", &address],
        vec!["decrypt", "--ciphertext", "00", "--use-hotkey"],
        vec!["unlock"],
        vec!["overview", "--all", "--netuid", "7", "--dust"],
        vec!["registrations", "--netuid", "0"],
        vec!["history", "--limit", "-1"],
        vec![
            "transfer",
            "--dest-ss58",
            &address,
            "--amount",
            "0.000000001",
        ],
        vec!["set_identity", "--name", "Alice"],
        vec!["get-identity", "--coldkey-ss58", &address],
        vec!["associate-hotkey", "--hotkey-ss58", &address],
        vec![
            "swap_hotkey",
            "--new-hotkey-ss58",
            &address,
            "--netuid",
            "3",
        ],
        vec!["swap-coldkey", "--new-coldkey", &address],
        vec!["swap-check"],
        vec!["announce-coldkey-swap", "--new-coldkey-ss58", &address],
    ] {
        tree.clone()
            .try_get_matches_from(["rtao", "w"].into_iter().chain(argv))?;
    }
    assert!(
        tree.clone()
            .try_get_matches_from(["rtao", "wallet", "overview", "--netuid", "65536"])
            .is_err()
    );
    let wallet = tree.find_subcommand("wallet").ok_or("wallet command")?;
    for alias in ["w", "wallets"] {
        assert!(wallet.get_all_aliases().any(|name| name == alias));
        assert!(!wallet.get_visible_aliases().any(|name| name == alias));
        tree.clone()
            .try_get_matches_from(["rtao", alias, "sign", "--message", "hello"])?;
    }
    let mut tree = tree;
    let help = tree.render_long_help().to_string();
    assert!(
        !help
            .lines()
            .any(|line| matches!(line.split_whitespace().next(), Some("w" | "wallets")))
    );
    Ok(())
}

#[tokio::test]
async fn sign_verify_uses_exact_utf8_bare_hex_and_false_exit_contract() -> TestResult {
    let root = Root::new()?;
    let wallet = Wallet::new("alice", "default", &root.0)?;
    let account = wallet
        .regenerate_coldkey(
            RegenerateFrom::Uri(SecretString::new("//Alice".into())),
            CryptoType::Sr25519,
            WriteOptions::hotkey(),
        )
        .await?;
    let (parsed, mut ctx, sink) = context(&root, &["wallet", "sign", "--message", "π\nhello"])?;
    local(&mut ctx, &parsed).await?;
    let signed = value(&sink)?;
    let signature = signed["signed_message"].as_str().ok_or("signature")?;
    assert_eq!(signature.len(), 128);
    assert!(!signature.starts_with("0x"));
    assert_eq!(signed["signer_address"], account.to_string());
    let spaced = signature
        .as_bytes()
        .chunks(2)
        .map(std::str::from_utf8)
        .collect::<Result<Vec<_>, _>>()?
        .join(" \t");
    assert!(rtao::wallet::verify_message(
        "π\nhello",
        &spaced,
        &account.to_string()
    ));
    for (signature, message, expected) in [
        (signature, "π\nhello", true),
        (signature, "π\nhello!", false),
        ("not-hex", "π\nhello", false),
    ] {
        let (parsed, mut ctx, sink) = context(
            &root,
            &[
                "wallet",
                "verify",
                "--message",
                message,
                "--signature",
                signature,
                "--ss58",
                &account.to_string(),
            ],
        )?;
        let result = wallet::run(&mut ctx, group(&parsed)?, &mut PromptHistory::default()).await;
        assert_eq!(result.is_ok(), expected);
        assert_eq!(value(&sink)?, json!({"valid":expected,"ss58":account}));
        if let Err(error) = result {
            assert_eq!(error.code, 1);
            assert!(error.message.is_empty());
        }
    }
    Ok(())
}

#[tokio::test]
async fn encrypt_decrypt_preserves_utf8_and_copy_json_refuses_before_wallet_access() -> TestResult {
    let root = Root::new()?;
    let wallet = Wallet::new("alice", "default", &root.0)?;
    let account = wallet
        .regenerate_coldkey(
            RegenerateFrom::Seed(zeroize::Zeroizing::new(vec![42; 32])),
            CryptoType::Ed25519,
            WriteOptions::hotkey(),
        )
        .await?;
    let plaintext = "private message π\n\u{0}done";
    let (parsed, mut ctx, sink) = context(
        &root,
        &[
            "wallet",
            "encrypt",
            "--message",
            plaintext,
            "--recipient",
            &account.to_string(),
        ],
    )?;
    local(&mut ctx, &parsed).await?;
    let encrypted = value(&sink)?;
    assert_eq!(encrypted["recipient"], account.to_string());
    let ciphertext = encrypted["ciphertext"].as_str().ok_or("ciphertext")?;
    let spaced = ciphertext
        .trim_start_matches("0x")
        .as_bytes()
        .chunks(2)
        .map(std::str::from_utf8)
        .collect::<Result<Vec<_>, _>>()?
        .join(" \n");
    let (parsed, mut ctx, sink) = context(&root, &["wallet", "decrypt", "--ciphertext", &spaced])?;
    local(&mut ctx, &parsed).await?;
    assert_eq!(value(&sink)?, json!({"message":plaintext}));
    for quiet in [false, true] {
        let (parsed, mut ctx, sink) =
            context(&root, &["wallet", "decrypt", "--ciphertext", ciphertext])?;
        ctx.globals.json = false;
        ctx.globals.quiet = quiet;
        ctx.output = Output::with_sink(
            OutputOptions {
                json_mode: false,
                quiet,
                network: None,
            },
            Arc::new(sink.clone()),
        );
        local(&mut ctx, &parsed).await?;
        assert_eq!(
            sink.text(Stream::Stdout)?,
            "decrypted\n  message: private message π\\n\\x00done\n"
        );
        assert!(sink.bytes(Stream::Stderr)?.is_empty());
    }
    let (parsed, mut ctx, sink) = context(
        &root,
        &["wallet", "decrypt", "--ciphertext", "bad", "--copy"],
    )?;
    fs::remove_file(wallet.reference().coldkey_path())?;
    let error = wallet::run(&mut ctx, group(&parsed)?, &mut PromptHistory::default())
        .await
        .err()
        .ok_or("copy/json must refuse")?;
    assert_eq!(error.code, 2);
    assert_eq!(error.message, "`--copy` does not apply in --json mode");
    assert!(sink.text(Stream::Stdout)?.is_empty());
    Ok(())
}

#[tokio::test]
async fn sdk_decrypt_reuses_authenticated_password_policy_and_refuses_non_utf8() -> TestResult {
    let root = Root::new()?;
    let wallet = Wallet::new("alice", "default", &root.0)?.with_password_sources(PasswordSources {
        password: Some(SecretString::new("fixture-password".into())),
        ..Default::default()
    });
    let account = wallet
        .regenerate_coldkey(
            RegenerateFrom::Seed(zeroize::Zeroizing::new(vec![43; 32])),
            CryptoType::Ed25519,
            WriteOptions::coldkey(),
        )
        .await?;
    let cipher = rtao::wallet::encrypt_message("hello", &account.to_string()).await?;
    assert_eq!(
        wallet
            .decrypt_message(&cipher.ciphertext, false)
            .await?
            .expose_secret(),
        "hello"
    );
    let wrong = Wallet::new("alice", "default", &root.0)?.with_password_sources(PasswordSources {
        password: Some(SecretString::new("wrong-fixture-password".into())),
        ..Default::default()
    });
    let error = wrong
        .decrypt_message(&cipher.ciphertext, false)
        .await
        .err()
        .ok_or("wrong password must fail")?;
    assert!(matches!(
        error,
        rtao::wallet::WalletError::WrongPassword { .. }
    ));
    assert!(!error.to_string().contains("wrong-fixture-password"));
    let invalid = Keypair::encrypt_for(&account.to_string(), &[0xff], 0)?;
    let error = wallet
        .decrypt_message(&hex::encode(invalid), false)
        .await
        .err()
        .ok_or("invalid UTF8 must fail")?;
    assert_eq!(
        error.to_string(),
        "crypto error: decrypted message is not valid UTF-8"
    );
    Ok(())
}

#[tokio::test]
async fn registrations_include_local_empty_and_chain_only_hotkeys_at_one_hash() -> TestResult {
    let root = Root::new()?;
    root.public("alice", COLD)?;
    let hotkeys = root.0.join("alice/hotkeys");
    fs::create_dir(&hotkeys)?;
    fs::write(
        hotkeys.join("worker10"),
        serde_json::to_vec(&json!({"ss58Address":HOT,"cryptoType":1}))?,
    )?;
    let mut state = FakeState::default();
    state.seed_python(
        "SubtensorModule",
        "OwnedHotkeys",
        &[json!(COLD)],
        json!([OWNED, HOT]),
    )?;
    state.seed_python(
        "SubtensorModule",
        "IsNetworkMember",
        &[json!(OWNED), json!(2)],
        json!(true),
    )?;
    state.seed_python(
        "SubtensorModule",
        "Uids",
        &[json!(2), json!(OWNED)],
        json!(0),
    )?;
    let (client, chain) = client(state).await?;
    let (parsed, mut ctx, sink) = context(&root, &["wallet", "registrations"])?;
    let before = chain.requests().len();
    wallet::run_with_client(
        &mut ctx,
        &client,
        group(&parsed)?,
        &mut PromptHistory::default(),
    )
    .await?;
    assert_eq!(
        value(&sink)?,
        json!([{"wallet":"alice","coldkey":COLD,"hotkeys":[
            {"name":"worker10","hotkey":HOT,"local":true,"registrations":[]},
            {"name":null,"hotkey":OWNED,"local":false,"registrations":[{"netuid":2,"uid":0}]}
        ]}])
    );
    let mut hashes = std::collections::BTreeSet::new();
    for request in &chain.requests()[before..] {
        assert!(!request.method.starts_with("author_"));
        if matches!(
            request.method.as_str(),
            "state_getStorage"
                | "state_getStorageAt"
                | "state_queryStorageAt"
                | "state_getKeysPaged"
        ) {
            let index = if request.method == "state_getKeysPaged" {
                3
            } else {
                1
            };
            hashes.insert(
                request
                    .params
                    .get(index)
                    .and_then(Value::as_str)
                    .ok_or("read lacks pinned hash")?
                    .to_owned(),
            );
        }
    }
    assert_eq!(hashes.len(), 1);
    assert_eq!(
        fs::read(root.0.join("alice/coldkey"))?,
        b"private-key sentinel must not be opened by reads or plans"
    );
    client.close().await;
    Ok(())
}

#[tokio::test]
async fn identity_and_empty_swap_check_use_public_reads_only() -> TestResult {
    let root = Root::new()?;
    root.public("alice", COLD)?;
    let mut state = FakeState::default();
    state.seed_python(
        "SubtensorModule",
        "IdentitiesV2",
        &[json!(COLD)],
        json!({"name":"Alice","url":"https://example.com"}),
    )?;
    let (client, chain) = client(state).await?;
    for (command, expected) in [
        (
            "get-identity",
            json!({"name":"Alice","url":"https://example.com","github_repo":"","image":"","discord":"","description":"","additional":""}),
        ),
        ("swap-check", json!({"status":"none"})),
    ] {
        let (parsed, mut ctx, sink) = context(&root, &["wallet", command])?;
        wallet::run_with_client(
            &mut ctx,
            &client,
            group(&parsed)?,
            &mut PromptHistory::default(),
        )
        .await?;
        assert_eq!(value(&sink)?, expected);
    }
    assert!(
        !chain
            .requests()
            .iter()
            .any(|r| r.method.starts_with("author_"))
    );
    client.close().await;
    Ok(())
}

#[tokio::test]
async fn transfer_preview_retains_native_exact_amount_and_never_reads_private_material()
-> TestResult {
    let root = Root::new()?;
    root.public("alice", COLD)?;
    let mut state = FakeState::default();
    state.seed_python(
        "System",
        "Account",
        &[json!(COLD)],
        json!({"data":{"free":10_000_000_000u64}}),
    )?;
    let (client, chain) = client(state).await?;
    let (parsed, mut ctx, sink) = context(
        &root,
        &[
            "wallet",
            "transfer",
            "--dest",
            &OWNED.to_string(),
            "--amount",
            "1.000000001",
            "--period",
            "64",
            "--dry-run",
            "--yes",
        ],
    )?;
    wallet::run_with_client(
        &mut ctx,
        &client,
        group(&parsed)?,
        &mut PromptHistory::default(),
    )
    .await?;
    let plan = value(&sink)?;
    assert_eq!(plan["args"]["amount_tao"], "1.000000001");
    assert_eq!(plan["args"]["dest_ss58"], OWNED.to_string());
    assert_eq!(
        fs::read(root.0.join("alice/coldkey"))?,
        b"private-key sentinel must not be opened by reads or plans"
    );
    assert!(
        chain
            .state()
            .lock()
            .map_err(|_| "state poisoned")?
            .submissions
            .is_empty()
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

fn seed_overview_availability(
    state: &mut FakeState,
    coldkey: AccountId,
    rows: &[(u16, u64, u64, u64)],
) -> TestResult {
    use rtao::core::{codec::Value as CoreValue, ops::runtime_api, runtime::TypeSpec};
    use rtao_testkit::fakes::json_to_core;
    let api = "StakeInfoRuntimeApi";
    let method = "get_stake_availability_for_coldkeys";
    let signature = runtime_api::runtime_api_method(&state.runtime, api, method)?;
    let input = runtime_api::encode_params(
        &state.runtime,
        api,
        signature,
        &[
            CoreValue::List(vec![CoreValue::Bytes(coldkey.0.to_vec())]),
            CoreValue::List(
                rows.iter()
                    .map(|(netuid, ..)| CoreValue::Int(i128::from(*netuid)))
                    .collect(),
            ),
        ],
    )?;
    let availability = rows
        .iter()
        .map(|(netuid, total, locked, available)| {
            Ok((
                CoreValue::Int(i128::from(*netuid)),
                json_to_core(&json!({"total":total,"locked":locked,"available":available}))?,
            ))
        })
        .collect::<Result<Vec<_>, Box<dyn Error>>>()?;
    let output = state.runtime.encode_spec(
        &TypeSpec::Id(signature.output),
        &CoreValue::Dict(vec![(
            CoreValue::Bytes(coldkey.0.to_vec()),
            CoreValue::Dict(availability),
        )]),
    )?;
    state.seed_runtime_encoded(format!("{api}_{method}"), input, output);
    Ok(())
}

#[tokio::test]
async fn overview_json_preserves_dust_positions_locks_root_yield_and_registered_uid() -> TestResult
{
    let root = Root::new()?;
    root.public("alice", COLD)?;
    let mut state = FakeState::default();
    state.seed_python(
        "System",
        "Account",
        &[json!(COLD)],
        json!({"data":{"free":1_250_000_000u64}}),
    )?;
    state.seed_python("SubtensorModule", "Owner", &[json!(HOT)], json!(COLD))?;
    state.seed_python("SubtensorModule", "Owner", &[json!(OWNED)], json!(COLD))?;
    state.seed_python(
        "SubtensorModule",
        "Uids",
        &[json!(1), json!(HOT)],
        json!(17),
    )?;
    state.seed_python("SubtensorModule", "TokenSymbol", &[json!(1)], json!("Ω"))?;
    state.seed_runtime("StakeInfoRuntimeApi","get_stake_info_for_coldkeys",json!([[COLD,[
        {"hotkey":HOT,"coldkey":COLD,"netuid":1,"stake":3_000_000_000u64,"is_registered":true},
        {"hotkey":OWNED,"coldkey":COLD,"netuid":0,"stake":2_000_000_000u64,"is_registered":false},
        {"hotkey":OWNED,"coldkey":COLD,"netuid":1,"stake":2u64,"is_registered":false},
    ]]]))?;
    state.seed_runtime(
        "SwapRuntimeApi",
        "current_alpha_price_all",
        json!([{"netuid":1,"price":500_000_000}]),
    )?;
    state.seed_runtime(
        "BetaBasketRuntimeApi",
        "get_root_basket_owed",
        json!(250_000_000),
    )?;
    seed_overview_availability(
        &mut state,
        COLD,
        &[
            (0, 2_000_000_000, 0, 2_000_000_000),
            (1, 3_000_000_000, 2_000_000_000, 1_000_000_000),
        ],
    )?;
    let (client, chain) = client(state).await?;
    let (parsed, mut ctx, sink) = context(&root, &["wallet", "overview", "--netuid", "1"])?;
    wallet::run_with_client(
        &mut ctx,
        &client,
        group(&parsed)?,
        &mut PromptHistory::default(),
    )
    .await?;
    let rows = value(&sink)?;
    // The machine formatter uses Python's two-digit exponents. With
    // arbitrary_precision, serde_json Number equality retains that spelling.
    let dust_amount: Value = serde_json::from_str("2e-09")?;
    let dust_value: Value = serde_json::from_str("1e-09")?;
    let yields = json!({
        "accrued_basket_yield":"τ0.250000000", "accrued_basket_yield_tao":0.25,
        "root_staked":"τ2.000000000", "root_staked_tao":2.0,
        "personal_yield":0.125, "auto_claim":false,
    });
    let mut expected = json!({
        "wallet":"alice", "coldkey":COLD,
        "free":"τ1.250000000", "free_tao":1.25,
        "stake_value":"τ3.500000001", "stake_value_tao":3.500000001,
        "stake_value_basis":"spot price; excludes slippage/fees of an actual unstake",
        "locked_value":"τ1.000000000", "locked_value_tao":1.0,
        "locked_subnets":1, "positions":2,
        "stakes":[
            {
                "netuid":1, "hotkey":HOT, "stake":"3.000000000Ω", "stake_amount":3.0,
                "stake_unit":"alpha (netuid 1)", "value":"τ1.500000000", "value_tao":1.5,
                "registered":true, "uid":17,
                "locked":"2.000000000Ω", "locked_amount":2.0,
                "available":"1.000000000Ω", "available_amount":1.0,
            },
            {
                "netuid":1, "hotkey":OWNED, "stake":"0.000000002Ω", "stake_amount":dust_amount,
                "stake_unit":"alpha (netuid 1)", "value":"τ0.000000001", "value_tao":dust_value,
                "registered":false, "uid":null,
                "locked":"2.000000000Ω", "locked_amount":2.0,
                "available":"1.000000000Ω", "available_amount":1.0,
            },
        ],
    });
    let yield_fields = yields.as_object().ok_or("yield fields")?;
    for record in expected["stakes"].as_array_mut().ok_or("expected stakes")? {
        record
            .as_object_mut()
            .ok_or("expected position")?
            .extend(yield_fields.clone());
    }
    expected
        .as_object_mut()
        .ok_or("expected overview")?
        .extend(yield_fields.clone());
    assert_eq!(rows, json!([expected]));
    assert!(sink.text(Stream::Stderr)?.is_empty());
    assert!(
        !chain
            .requests()
            .iter()
            .any(|r| r.method.starts_with("author_"))
    );
    client.close().await;
    Ok(())
}

#[tokio::test]
async fn overview_human_groups_wallet_subtotals_and_lock_targets_without_repeating_availability()
-> TestResult {
    const BOB: AccountId = AccountId([10; 32]);
    const LOCK_TARGET: AccountId = AccountId([11; 32]);
    const DUST: AccountId = AccountId([12; 32]);
    let root = Root::new()?;
    root.public("alice", COLD)?;
    root.public("bob", BOB)?;
    let hotkeys = root.0.join("alice/hotkeys");
    fs::create_dir(&hotkeys)?;
    fs::write(
        hotkeys.join("local-worker"),
        serde_json::to_vec(&json!({"ss58Address":HOT,"cryptoType":1}))?,
    )?;
    let mut state = FakeState::default();
    for hotkey in [HOT, OWNED, DUST, LOCK_TARGET] {
        state.seed_python("SubtensorModule", "Owner", &[json!(hotkey)], json!(COLD))?;
    }
    state.seed_python(
        "SubtensorModule",
        "IdentitiesV2",
        &[json!(COLD)],
        json!({"name":"chain-validator"}),
    )?;
    state.seed_python("SubtensorModule", "TokenSymbol", &[json!(1)], json!("Ω"))?;
    state.seed_python("SubtensorModule", "TokenSymbol", &[json!(2)], json!("Ψ"))?;
    state.seed_python(
        "SubtensorModule",
        "Uids",
        &[json!(1), json!(HOT)],
        json!(17),
    )?;
    state.seed_python(
        "SubtensorModule",
        "Lock",
        &[json!(COLD), json!(1), json!(LOCK_TARGET)],
        json!({"locked_mass":2_000_000_000u64,"conviction":{"bits":0},"last_update":0}),
    )?;
    state.seed_python(
        "SubtensorModule",
        "DecayingLock",
        &[json!(COLD), json!(1)],
        json!(false),
    )?;
    state.seed_runtime(
        "StakeInfoRuntimeApi",
        "get_coldkey_lock",
        json!({"locked_mass":2_000_000_000u64,"conviction":{"bits":0},"last_update":0}),
    )?;
    // Runtime order is deliberately different from subnet/stake display order.
    state.seed_runtime("StakeInfoRuntimeApi", "get_stake_info_for_coldkeys", json!([
        [COLD,[
            {"hotkey":OWNED,"coldkey":COLD,"netuid":2,"stake":7_000_000_000u64,"is_registered":false},
            {"hotkey":OWNED,"coldkey":COLD,"netuid":1,"stake":2_000_000_000u64,"is_registered":false},
            {"hotkey":HOT,"coldkey":COLD,"netuid":1,"stake":3_000_000_000u64,"is_registered":true},
            {"hotkey":DUST,"coldkey":COLD,"netuid":1,"stake":1u64,"is_registered":false},
        ]],
        [BOB,[{"hotkey":HOT,"coldkey":BOB,"netuid":1,"stake":4_000_000_000u64,"is_registered":false}]],
    ]))?;
    state.seed_runtime(
        "SwapRuntimeApi",
        "current_alpha_price_all",
        json!([
            {"netuid":1,"price":500_000_000}, {"netuid":2,"price":250_000_000},
        ]),
    )?;
    state.seed_runtime("BetaBasketRuntimeApi", "get_root_basket_owed", json!(0))?;
    seed_overview_availability(
        &mut state,
        COLD,
        &[
            (1, 5_000_000_001, 2_000_000_000, 3_000_000_001),
            (2, 7_000_000_000, 0, 7_000_000_000),
        ],
    )?;
    seed_overview_availability(&mut state, BOB, &[(1, 4_000_000_000, 0, 4_000_000_000)])?;
    let (client, chain) = client(state).await?;
    for (quiet, dust) in [(false, false), (true, false), (false, true)] {
        let mut args = vec!["wallet", "overview", "--all"];
        if dust {
            args.push("--dust");
        }
        let (parsed, mut ctx, _) = context(&root, &args)?;
        ctx.globals.json = false;
        ctx.globals.quiet = quiet;
        ctx.books.addresses.extend([
            json!({"name":"contact-must-not-replace-local","address":HOT}),
            json!({"name":"lock\u{1b}[31mtarget","address":LOCK_TARGET}),
        ]);
        let sink = MemorySink::default();
        ctx.output = Output::with_sink(
            OutputOptions {
                json_mode: false,
                quiet,
                network: None,
            },
            Arc::new(sink.clone()),
        );
        // run_with_client bypasses AppContext::run_with's output setup.
        // Install the same client-loaded symbols on this replacement renderer.
        ctx.output.update_token_symbols(
            client
                .token_symbols()
                .await
                .into_iter()
                .map(|(netuid, symbol)| (netuid.get(), rtao::primitives::Symbol::new(symbol))),
        )?;
        wallet::run_with_client(
            &mut ctx,
            &client,
            group(&parsed)?,
            &mut PromptHistory::default(),
        )
        .await?;
        let text = sink.text(Stream::Stdout)?;
        let alice_one = text.find("alice · netuid 1 Ω").ok_or("alice subnet 1")?;
        let alice_two = text.find("alice · netuid 2 Ψ").ok_or("alice subnet 2")?;
        let bob_one = text.find("bob · netuid 1 Ω").ok_or("bob subnet 1")?;
        assert!(alice_one < alice_two && alice_two < bob_one);
        let first = &text[alice_one..alice_two];
        let second = &text[alice_two..bob_one];
        let bob = &text[bob_one..];
        assert!(first.contains("5.000000001Ω"));
        assert!(first.contains("τ2.500000000"));
        assert!(second.contains("7.000000000Ψ"));
        assert!(second.contains("τ1.750000000"));
        assert!(bob.contains("4.000000000Ω"));
        assert!(bob.contains("τ2.000000000"));
        assert_eq!(
            text.matches("2.000000000Ω locked · 3.000000001Ω free")
                .count(),
            1
        );
        assert!(first.contains("lock → lock\\x1b[31mtarget"));
        assert_eq!(
            first.matches("lock on lock\\x1b[31mtarget").count(),
            if dust { 3 } else { 2 }
        );
        assert!(!text.contains('\u{1b}'));
        assert!(
            first.find("local-worker").ok_or("local name")?
                < first.find("chain-validator").ok_or("chain name")?
        );
        assert!(!text.contains("contact-must-not-replace-local"));
        assert_eq!(first.contains("0.000000001Ω"), dust);
        assert_eq!(text.contains("1 dust positions hidden"), !dust);
        assert!(text.contains("total τ6.250000000  (spot, excl. slippage/fees)"));
        assert!(sink.text(Stream::Stderr)?.is_empty());
    }
    assert!(
        !chain
            .requests()
            .iter()
            .any(|request| request.method.starts_with("author_"))
    );
    assert_eq!(
        fs::read(root.0.join("alice/coldkey"))?,
        b"private-key sentinel must not be opened by reads or plans"
    );
    client.close().await;
    Ok(())
}

#[tokio::test]
async fn overview_human_subnet_stake_overflow_refuses_before_output_while_json_keeps_positions()
-> TestResult {
    let root = Root::new()?;
    root.public("alice", COLD)?;
    let mut state = FakeState::default();
    for hotkey in [HOT, OWNED] {
        state.seed_python("SubtensorModule", "Owner", &[json!(hotkey)], json!(COLD))?;
    }
    let amount = (u64::MAX / 2) + 1;
    state.seed_runtime(
        "StakeInfoRuntimeApi",
        "get_stake_info_for_coldkeys",
        json!([[COLD,[
            {"hotkey":HOT,"coldkey":COLD,"netuid":1,"stake":amount,"is_registered":false},
            {"hotkey":OWNED,"coldkey":COLD,"netuid":1,"stake":amount,"is_registered":false},
        ]]]),
    )?;
    state.seed_runtime(
        "SwapRuntimeApi",
        "current_alpha_price_all",
        json!([{ "netuid":1,"price":250_000_000 }]),
    )?;
    state.seed_runtime("BetaBasketRuntimeApi", "get_root_basket_owed", json!(0))?;
    seed_overview_availability(&mut state, COLD, &[(1, u64::MAX, 0, u64::MAX)])?;
    let (client, chain) = client(state).await?;
    let (parsed, mut ctx, sink) = context(&root, &["wallet", "overview"])?;
    wallet::run_with_client(
        &mut ctx,
        &client,
        group(&parsed)?,
        &mut PromptHistory::default(),
    )
    .await?;
    assert_eq!(
        value(&sink)?[0]["stakes"]
            .as_array()
            .ok_or("positions")?
            .len(),
        2
    );
    let (parsed, mut ctx, _) = context(&root, &["wallet", "overview"])?;
    ctx.globals.json = false;
    let sink = MemorySink::default();
    ctx.output = Output::with_sink(OutputOptions::default(), Arc::new(sink.clone()));
    let error = wallet::run_with_client(
        &mut ctx,
        &client,
        group(&parsed)?,
        &mut PromptHistory::default(),
    )
    .await
    .err()
    .ok_or("subnet sum must overflow")?;
    assert_eq!(error.code, 2);
    assert_eq!(error.message, "subnet stake exceeds native u64 rao");
    assert!(sink.text(Stream::Stdout)?.is_empty());
    assert!(sink.text(Stream::Stderr)?.is_empty());
    assert!(
        !chain
            .requests()
            .iter()
            .any(|request| request.method.starts_with("author_"))
    );
    client.close().await;
    Ok(())
}
