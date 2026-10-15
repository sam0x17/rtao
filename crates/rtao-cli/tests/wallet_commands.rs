//! Wallet command fixtures use only task-owned files and injected terminals.

use rtao::{
    primitives::{AccountId, CryptoType},
    signer::{KeyView, SignerRole},
    wallet::{PasswordSources, SecretString, Wallet},
};
use rtao_cli::{
    application,
    books::Books,
    commands::wallet,
    context::AppContext,
    globals::Globals,
    output::{MemorySink, Output, OutputOptions, OutputSink, Stream},
    prompt::{PromptError, PromptHistory, Terminal},
};
use serde_json::{Value, json};
use std::{
    collections::VecDeque,
    error::Error,
    path::PathBuf,
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, AtomicUsize, Ordering},
    },
};

type TestResult = Result<(), Box<dyn Error>>;
type WalletContext = (clap::ArgMatches, AppContext, MemorySink, Arc<AtomicUsize>);
struct Root(PathBuf);
impl Root {
    fn new() -> Result<Self, std::io::Error> {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let base = std::env::var_os("RTAO_TEMP_ROOT")
            .map(PathBuf::from)
            .unwrap_or_else(|| {
                PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/wallet-command-tests")
            });
        let path = base.join("tmp").join(format!(
            "wallet-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&path)?;
        Ok(Self(path))
    }
}
impl Drop for Root {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

struct TerminalFixture {
    tty: bool,
    secrets: VecDeque<SecretString>,
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
        Err(PromptError::Unavailable)
    }
    fn read_secret(&mut self, _: &str) -> Result<SecretString, PromptError> {
        self.reads.fetch_add(1, Ordering::Relaxed);
        self.secrets.pop_front().ok_or(PromptError::Aborted)
    }
    fn message(&mut self, _: &str) -> Result<(), PromptError> {
        Ok(())
    }
}
fn fixture(
    root: &Root,
    args: &[&str],
    json_mode: bool,
    tty: bool,
    secrets: &[&str],
) -> Result<WalletContext, Box<dyn Error>> {
    let matches = application::command_tree([wallet::command()]).try_get_matches_from(args)?;
    let leaf = matches
        .subcommand()
        .ok_or("wallet group")?
        .1
        .subcommand()
        .ok_or("wallet leaf")?
        .1;
    let mut globals = Globals::resolve(&matches, &Default::default(), &root.0, |_| None)?;
    globals.apply_with_env(leaf, &[], |_| None)?;
    globals.wallet_path = root.0.clone();
    globals.json = json_mode;
    let reads = Arc::new(AtomicUsize::new(0));
    let terminal = TerminalFixture {
        tty,
        secrets: secrets
            .iter()
            .map(|value| SecretString::new((*value).into()))
            .collect(),
        reads: reads.clone(),
    };
    let mut ctx = AppContext::new(
        globals,
        Default::default(),
        Books::default(),
        Arc::new(Mutex::new(Box::new(terminal))),
    );
    let sink = MemorySink::default();
    ctx.output = Output::with_sink(
        OutputOptions {
            json_mode,
            quiet: false,
            network: None,
        },
        Arc::new(sink.clone()),
    );
    ctx.passwords = PasswordSources::default();
    Ok((matches, ctx, sink, reads))
}
async fn run(
    ctx: &mut AppContext,
    matches: &clap::ArgMatches,
    history: &mut PromptHistory,
) -> Result<(), rtao_cli::error::CliError> {
    wallet::run(
        ctx,
        matches
            .subcommand()
            .ok_or_else(|| rtao_cli::error::CliError::input("missing wallet"))?
            .1,
        history,
    )
    .await
}
fn payload(sink: &MemorySink) -> Result<Value, Box<dyn Error>> {
    Ok(serde_json::from_slice(&sink.bytes(Stream::Stdout)?)?)
}

#[test]
fn wallet_surface_exposes_wallet_commands_and_supports_compatibility_aliases() -> TestResult {
    let command = wallet::command();
    let names = command
        .get_subcommands()
        .map(|command| command.get_name())
        .collect::<Vec<_>>();
    assert_eq!(
        names,
        vec![
            "create",
            "new-coldkey",
            "new-hotkey",
            "regen-coldkey",
            "regen-hotkey",
            "regen-coldkeypub",
            "regen-hotkeypub",
            "show",
            "list",
            "balance",
            "inspect",
            "save-password-env",
            "keychain"
        ]
    );
    let root = Root::new()?;
    let _ = fixture(
        &root,
        &["rtao", "w", "new_hotkey", "--crypto-type", "ed25519"],
        true,
        false,
        &[],
    )?;
    assert!(
        application::command_tree([wallet::command()])
            .try_get_matches_from(["rtao", "wallet", "sign"])
            .is_err()
    );
    Ok(())
}

#[tokio::test]
async fn creates_both_real_key_schemes_and_keeps_the_single_json_mnemonic_payload() -> TestResult {
    let root = Root::new()?;
    let (matches, mut ctx, sink, reads) = fixture(
        &root,
        &[
            "rtao",
            "wallet",
            "create",
            "-w",
            "new",
            "-H",
            "worker",
            "--no-password",
            "--crypto-type",
            "ed25519",
            "--hotkey-crypto-type",
            "sr25519",
        ],
        true,
        false,
        &[],
    )?;
    run(&mut ctx, &matches, &mut PromptHistory::default()).await?;
    let value = payload(&sink)?;
    assert_eq!(value.as_object().ok_or("creation object")?.len(), 8);
    assert_eq!(value["coldkey_crypto_type"], "ed25519");
    assert_eq!(value["hotkey_crypto_type"], "sr25519");
    let local = Wallet::new("new", "worker", &root.0)?;
    assert_eq!(value["coldkey_ss58"], json!(local.coldkey_address()?));
    assert_eq!(
        local.public_info(SignerRole::Coldkey)?.crypto_type,
        CryptoType::Ed25519
    );
    assert_eq!(
        local.public_info(SignerRole::Hotkey)?.crypto_type,
        CryptoType::Sr25519
    );
    let phrase = value["coldkey_mnemonic"]
        .as_str()
        .ok_or("coldkey mnemonic")?;
    let derived = rtao::core::keys::Keypair::from_mnemonic(phrase, 0, None)?;
    assert_eq!(derived.public_key_bytes(), local.coldkey_address()?.0);
    let phrase = value["hotkey_mnemonic"].as_str().ok_or("hotkey mnemonic")?;
    let derived = rtao::core::keys::Keypair::from_mnemonic(phrase, 1, None)?;
    assert_eq!(derived.public_key_bytes(), local.hotkey_address()?.0);
    assert!(sink.bytes(Stream::Stderr)?.is_empty());
    assert_eq!(reads.load(Ordering::Relaxed), 0);
    let phrase = value["coldkey_mnemonic"]
        .as_str()
        .ok_or("coldkey mnemonic")?;
    let (matches, mut ctx, sink, _) = fixture(
        &root,
        &[
            "rtao",
            "wallet",
            "regen-coldkey",
            "-w",
            "restored",
            "--mnemonic",
            phrase,
            "--no-password",
            "--crypto-type",
            "ed25519",
        ],
        true,
        false,
        &[],
    )?;
    run(&mut ctx, &matches, &mut PromptHistory::default()).await?;
    assert_eq!(payload(&sink)?["ss58"], value["coldkey_ss58"]);
    assert!(!sink.text(Stream::Stdout)?.contains(phrase));
    Ok(())
}

#[tokio::test]
async fn default_coldkey_encryption_uses_confirmed_hidden_password_without_echo() -> TestResult {
    let root = Root::new()?;
    let (matches, mut ctx, sink, reads) = fixture(
        &root,
        &["rtao", "wallet", "new-coldkey", "-w", "encrypted"],
        true,
        true,
        &[
            "different-first-password",
            "different-second-password",
            "matched-private-password",
            "matched-private-password",
        ],
    )?;
    ctx.passwords.prompt = rtao::wallet::PromptPolicy::IfTty;
    run(&mut ctx, &matches, &mut PromptHistory::default()).await?;
    assert_eq!(reads.load(Ordering::Relaxed), 4);
    let local = Wallet::new("encrypted", "default", &root.0)?;
    let bytes = std::fs::read(local.reference().coldkey_path())?;
    assert!(rtao::core::keyfile_json::keyfile_data_is_encrypted_nacl(
        &bytes
    ));
    let identity = local.public_info(SignerRole::Coldkey)?.account;
    assert_eq!(payload(&sink)?["ss58"], json!(identity));
    let local = local.with_password_sources(PasswordSources {
        password: Some(SecretString::new("matched-private-password".into())),
        ..Default::default()
    });
    let signer = local.unlock_coldkey().await?;
    assert_eq!(signer.account(), identity);
    for secret in [
        "different-first-password",
        "different-second-password",
        "matched-private-password",
    ] {
        assert!(!sink.text(Stream::Stdout)?.contains(secret));
        assert!(!sink.text(Stream::Stderr)?.contains(secret));
    }
    Ok(())
}

#[tokio::test]
async fn new_key_commands_refuse_overwrite_and_create_rejects_bad_hot_scheme_before_writes()
-> TestResult {
    let root = Root::new()?;
    for (name, extra) in [
        ("new-coldkey", vec!["--no-password"]),
        ("new-hotkey", vec![]),
    ] {
        let mut args = vec!["rtao", "wallet", name, "-w", "existing"];
        args.extend(extra);
        let (matches, mut ctx, sink, _) = fixture(&root, &args, true, false, &[])?;
        run(&mut ctx, &matches, &mut PromptHistory::default()).await?;
        assert!(payload(&sink)?["mnemonic"].is_string());
        let wallet = Wallet::new("existing", "default", &root.0)?;
        let path = if name == "new-coldkey" {
            wallet.reference().coldkey_path()
        } else {
            wallet.reference().hotkey_path()
        };
        let before = std::fs::read(&path)?;
        let (matches, mut ctx, sink, _) = fixture(&root, &args, true, false, &[])?;
        assert!(
            run(&mut ctx, &matches, &mut PromptHistory::default())
                .await
                .is_err()
        );
        assert_eq!(std::fs::read(path)?, before);
        assert!(sink.bytes(Stream::Stdout)?.is_empty());
    }
    let (matches, mut ctx, sink, _) = fixture(
        &root,
        &[
            "rtao",
            "wallet",
            "create",
            "-w",
            "bad-scheme",
            "--no-password",
            "--hotkey-crypto-type",
            "invalid",
        ],
        true,
        false,
        &[],
    )?;
    assert!(
        run(&mut ctx, &matches, &mut PromptHistory::default())
            .await
            .is_err()
    );
    assert!(!root.0.join("bad-scheme").exists());
    assert!(sink.bytes(Stream::Stdout)?.is_empty());
    Ok(())
}

#[tokio::test]
async fn seed_and_private_key_recovery_do_not_echo_material_or_put_it_in_replay() -> TestResult {
    let root = Root::new()?;
    let seed = "17".repeat(32);
    let args = [
        "rtao",
        "wallet",
        "regen-coldkey",
        "-w",
        "recovered",
        "--seed",
        &seed,
        "--no-password",
        "--crypto-type",
        "ed25519",
    ];
    let (matches, mut ctx, sink, reads) = fixture(&root, &args, true, false, &[])?;
    let mut history = PromptHistory::default();
    run(&mut ctx, &matches, &mut history).await?;
    let value = payload(&sink)?;
    assert_eq!(value.as_object().ok_or("recovery object")?.len(), 4);
    let bytes = hex::decode(&seed)?;
    let keypair = rtao::core::keys::Keypair::from_seed(&bytes, 0)?;
    assert_eq!(value["ss58"], json!(AccountId(keypair.public_key_bytes())));
    let private_keypair = rtao::core::keys::Keypair::from_seed(&bytes, 1)?;
    let private = zeroize::Zeroizing::new(format!(
        "0x{}",
        hex::encode(private_keypair.private_key_bytes().ok_or("private key")?)
    ));
    let args = [
        "rtao",
        "wallet",
        "regen-hotkey",
        "-w",
        "recovered",
        "-H",
        "imported",
        "--private-key",
        &private,
        "--crypto-type",
        "sr25519",
    ];
    let (matches, mut ctx, sink, _) = fixture(&root, &args, true, false, &[])?;
    run(&mut ctx, &matches, &mut history).await?;
    assert_eq!(
        payload(&sink)?["ss58"],
        json!(AccountId(private_keypair.public_key_bytes()))
    );
    for secret in [&seed, private.as_str()] {
        assert!(!sink.text(Stream::Stdout)?.contains(secret));
        assert!(!sink.text(Stream::Stderr)?.contains(secret));
        assert!(
            !history
                .replay_command(&args.iter().map(|arg| (*arg).to_owned()).collect::<Vec<_>>())
                .contains(secret)
        );
    }
    assert_eq!(reads.load(Ordering::Relaxed), 0);
    Ok(())
}

#[tokio::test]
async fn recovery_rejects_ambiguous_and_invalid_secrets_before_creating_directories() -> TestResult
{
    let root = Root::new()?;
    for options in [
        vec!["--seed", "secret-sentinel"],
        vec!["--private-key", "secret-sentinel"],
        vec![
            "--seed",
            "secret-sentinel",
            "--mnemonic",
            "secret-phrase-sentinel",
        ],
        vec![],
    ] {
        let mut args = vec!["rtao", "wallet", "regen-hotkey", "-w", "absent"];
        args.extend(options);
        let (matches, mut ctx, sink, reads) = fixture(&root, &args, true, false, &[])?;
        let error = run(&mut ctx, &matches, &mut PromptHistory::default())
            .await
            .err()
            .ok_or("invalid secret accepted")?;
        assert_eq!(error.code, 2);
        assert!(!error.to_string().contains("sentinel"));
        assert!(!root.0.join("absent").exists());
        assert!(sink.bytes(Stream::Stdout)?.is_empty());
        assert_eq!(reads.load(Ordering::Relaxed), 0);
    }
    Ok(())
}

#[tokio::test]
async fn hidden_recovery_auto_detects_seed_without_recording_secret_answers() -> TestResult {
    let root = Root::new()?;
    let seed = "23".repeat(32);
    let (matches, mut ctx, sink, reads) = fixture(
        &root,
        &[
            "rtao",
            "wallet",
            "regen-hotkey",
            "-w",
            "prompted",
            "-H",
            "worker",
        ],
        false,
        true,
        &[&seed],
    )?;
    let mut history = PromptHistory::default();
    run(&mut ctx, &matches, &mut history).await?;
    assert_eq!(reads.load(Ordering::Relaxed), 1);
    assert!(!sink.text(Stream::Stdout)?.contains(&seed));
    assert!(!sink.text(Stream::Stderr)?.contains(&seed));
    assert!(
        !history
            .replay_command(&["rtao".into(), "wallet".into(), "regen-hotkey".into()])
            .contains(&seed)
    );
    Ok(())
}

#[tokio::test]
async fn public_companions_require_matching_public_bytes_and_never_write_private_files()
-> TestResult {
    let root = Root::new()?;
    let account = AccountId([9; 32]);
    let ss58 = account.to_string();
    let key = hex::encode(account.0);
    for name in ["regen-coldkeypub", "regen-hotkeypub"] {
        let args = [
            "rtao",
            "wallet",
            name,
            "-w",
            "watch",
            "-H",
            "worker",
            "--ss58",
            &ss58,
            "--public-key",
            &key,
        ];
        let (matches, mut ctx, sink, _) = fixture(&root, &args, true, false, &[])?;
        run(&mut ctx, &matches, &mut PromptHistory::default()).await?;
        assert_eq!(
            payload(&sink)?,
            json!({"ss58":ss58,"crypto_type":"sr25519"})
        );
    }
    let wallet = Wallet::new("watch", "worker", &root.0)?;
    assert!(!wallet.reference().coldkey_path().exists());
    assert!(!wallet.reference().hotkey_path().exists());
    assert_eq!(wallet.coldkey_address()?, account);
    assert_eq!(wallet.hotkey_address()?, account);
    let args = [
        "rtao",
        "wallet",
        "regen-coldkeypub",
        "-w",
        "wrong",
        "--ss58",
        &ss58,
        "--public-key",
        &"00".repeat(32),
    ];
    let (matches, mut ctx, _, _) = fixture(&root, &args, true, false, &[])?;
    assert!(
        run(&mut ctx, &matches, &mut PromptHistory::default())
            .await
            .is_err()
    );
    assert!(!root.0.join("wrong").exists());
    Ok(())
}

#[tokio::test]
async fn show_reads_watch_only_companions_and_saved_multisigs_without_secret_prompts() -> TestResult
{
    let root = Root::new()?;
    let account = AccountId([13; 32]);
    let wallet = Wallet::new("watch", "default", &root.0)?;
    wallet
        .regenerate_coldkeypub(account, CryptoType::Ed25519, rtao::wallet::Overwrite::No)
        .await?;
    let (matches, mut ctx, sink, reads) = fixture(
        &root,
        &["rtao", "wallet", "show", "-w", "watch"],
        true,
        false,
        &[],
    )?;
    run(&mut ctx, &matches, &mut PromptHistory::default()).await?;
    let value = payload(&sink)?;
    assert_eq!(value["coldkey_ss58"], json!(account));
    assert_eq!(value["coldkey_crypto_type"], "ed25519");
    assert!(
        value["hotkey"]
            .as_str()
            .is_some_and(|value| value.contains("unavailable"))
    );
    assert_eq!(reads.load(Ordering::Relaxed), 0);
    let (matches, mut ctx, sink, reads) = fixture(
        &root,
        &["rtao", "wallet", "show", "-w", "group"],
        true,
        false,
        &[],
    )?;
    ctx.books = Books::from_values(
        json!([]),
        json!([]),
        json!([{"name":"group","threshold":1,"signatories":[account.to_string()],"note":"watch account"}]),
    );
    run(&mut ctx, &matches, &mut PromptHistory::default()).await?;
    let value = payload(&sink)?;
    assert_eq!(value["threshold"], 1);
    assert_eq!(value["signatories"], json!([account.to_string()]));
    assert!(value["multisig_address"].is_string());
    assert_eq!(reads.load(Ordering::Relaxed), 0);
    assert!(!root.0.join("group").exists());
    Ok(())
}

#[tokio::test]
async fn json_import_failures_keep_the_keystore_and_password_out_of_diagnostics() -> TestResult {
    let root = Root::new()?;
    let path = root.0.join("export.json");
    std::fs::write(&path, b"keystore-private-sentinel")?;
    let path = path.to_string_lossy();
    let args = [
        "rtao",
        "wallet",
        "regen-coldkey",
        "-w",
        "imported",
        "--json-path",
        &path,
        "--json-password",
        "passphrase-private-sentinel",
        "--no-password",
    ];
    let (matches, mut ctx, sink, reads) = fixture(&root, &args, true, false, &[])?;
    let error = run(&mut ctx, &matches, &mut PromptHistory::default())
        .await
        .err()
        .ok_or("invalid JSON import accepted")?;
    assert_eq!(error.code, 1);
    assert!(!error.to_string().contains("sentinel"));
    assert!(sink.bytes(Stream::Stdout)?.is_empty());
    assert!(!root.0.join("imported").exists());
    assert_eq!(reads.load(Ordering::Relaxed), 0);
    assert_eq!(std::fs::read(&*path)?, b"keystore-private-sentinel");
    Ok(())
}

// Copyright 2017-2026 @polkadot/keyring authors & contributors (Apache-2.0).
// Exact public encrypted fixture from polkadot-js/common
// packages/keyring/src/index.spec.ts @ 9281daa14c2c2a41708c3489de8770a064525577,
// "version 3 JSON (base64)". The upstream passphrase is "version3" and the
// expected public identity is independent of the rtao import implementation.
// Fixture SHA-256: eeab2b83e4cf25ec651a7645ce705e6066adaa12787717b3934b68ac090fbf74.
const POLKADOTJS_V3_SR25519: &str = r#"{"address":"FLiSDPCcJ6auZUGXALLj6jpahcP6adVFDBUQznPXUQ7yoqH","encoded":"ILjSgYaGvq1zaCz/kx+aqfLaHBjLXz0Qsmr6RnkOVU4AgAAAAQAAAAgAAAB5R2hm5kgXyc0NQYFxvMU4zCdjB+ugs/ibEooqCvuudbaeKn3Ee47NkCqU1ecOJV+eeaVn4W4dRvIpj5kGmQOGsewR+MiQ/B0G9NFh7JXV0qcPlk2QMNW1/mbJrTO4miqL448BSkP7ZOhUV6HFUpMt3B9HwjiRLN8RORcFp0ID/Azs4Jl/xOpXNzbgQGIffWgCIKTxN9N1ku6tdlG4","encoding":{"content":["pkcs8","sr25519"],"type":["scrypt","xsalsa20-poly1305"],"version":"3"},"meta":{"genesisHash":"0xb0a8d493285c2df73290dfb7e61f870f17b41801197a149ca93654499ea3dafe","name":"version3","tags":[],"whenCreated":1595277797639,"whenEdited":1595278378596}}"#;

#[tokio::test]
async fn encrypted_json_import_uses_hidden_backup_password_and_reloads_the_real_encrypted_identity()
-> TestResult {
    let root = Root::new()?;
    let export_path = root.0.join("backup.json");
    std::fs::write(&export_path, POLKADOTJS_V3_SR25519)?;
    let export_path = export_path.to_string_lossy().into_owned();
    let expected: AccountId = "FLiSDPCcJ6auZUGXALLj6jpahcP6adVFDBUQznPXUQ7yoqH".parse()?;
    let args = [
        "rtao",
        "wallet",
        "regen-coldkey",
        "-w",
        "json-restored",
        "--json-path",
        &export_path,
        "--crypto-type",
        "ed25519",
    ];
    let storage_password = "rtao-storage-private-passphrase";
    let (matches, mut ctx, sink, reads) = fixture(&root, &args, false, true, &["version3"])?;
    // Import's hidden passphrase and the new on-disk password are separate.
    // An explicit storage source also isolates this fixture from process env.
    ctx.passwords.password = Some(SecretString::new(storage_password.into()));
    let mut history = PromptHistory::default();
    run(&mut ctx, &matches, &mut history).await?;
    assert_eq!(reads.load(Ordering::Relaxed), 1);
    let restored = Wallet::new("json-restored", "default", &root.0)?;
    let public = restored.public_info(SignerRole::Coldkey)?;
    assert_eq!(public.account, expected);
    // The export's scheme wins over the deliberately different CLI hint.
    assert_eq!(public.crypto_type, CryptoType::Sr25519);
    let secret_path = restored.reference().coldkey_path();
    let public_path = restored.reference().coldkeypub_path();
    let encrypted_before = std::fs::read(&secret_path)?;
    let public_before = std::fs::read(&public_path)?;
    assert!(rtao::core::keyfile_json::keyfile_data_is_encrypted_nacl(
        &encrypted_before
    ));
    // Construct a fresh handle and actually decrypt the written coldkey. A
    // public companion or a populated in-memory cache cannot satisfy this.
    let reloaded =
        Wallet::new("json-restored", "default", &root.0)?.with_password_sources(PasswordSources {
            password: Some(SecretString::new(storage_password.into())),
            ..Default::default()
        });
    let signer = reloaded.unlock_coldkey().await?;
    assert_eq!(signer.account(), expected);
    assert_eq!(signer.crypto_type(), CryptoType::Sr25519);
    let export: Value = serde_json::from_str(POLKADOTJS_V3_SR25519)?;
    let encoded = export["encoded"].as_str().ok_or("encrypted export bytes")?;
    let stdout = sink.text(Stream::Stdout)?;
    let stderr = sink.text(Stream::Stderr)?;
    assert!(stdout.contains(&expected.to_string()));
    assert!(stdout.contains("sr25519"));
    let replay =
        history.replay_command(&args.iter().map(|arg| (*arg).to_owned()).collect::<Vec<_>>());
    for secret in ["version3", storage_password, encoded, POLKADOTJS_V3_SR25519] {
        assert!(!stdout.contains(secret));
        assert!(!stderr.contains(secret));
        assert!(!replay.contains(secret));
    }
    assert!(!replay.contains("--json-password"));
    assert_eq!(
        std::fs::read(&export_path)?,
        POLKADOTJS_V3_SR25519.as_bytes()
    );

    let (matches, mut ctx, refused_sink, reads) =
        fixture(&root, &args, false, true, &["version3"])?;
    ctx.passwords.password = Some(SecretString::new(storage_password.into()));
    let error = run(&mut ctx, &matches, &mut PromptHistory::default())
        .await
        .err()
        .ok_or("encrypted import overwrote an existing wallet")?;
    assert_eq!(error.code, 1);
    assert_eq!(reads.load(Ordering::Relaxed), 1);
    assert!(error.message.contains("refusing to overwrite"));
    assert!(refused_sink.bytes(Stream::Stdout)?.is_empty());
    for secret in ["version3", storage_password, encoded] {
        assert!(!error.to_string().contains(secret));
        assert!(!refused_sink.text(Stream::Stderr)?.contains(secret));
    }
    assert_eq!(std::fs::read(secret_path)?, encrypted_before);
    assert_eq!(std::fs::read(public_path)?, public_before);
    assert_eq!(
        std::fs::read(export_path)?,
        POLKADOTJS_V3_SR25519.as_bytes()
    );
    Ok(())
}

#[tokio::test]
async fn list_retains_partial_wallets_without_deriving_or_unlocking_secret_files() -> TestResult {
    let root = Root::new()?;
    let account = AccountId([5; 32]);
    for name in ["coldkey10", "coldkey2"] {
        std::fs::create_dir_all(root.0.join(name).join("hotkeys"))?;
        std::fs::write(
            root.0.join(name).join("coldkeypub.txt"),
            serde_json::to_vec(
                &json!({"ss58Address":account.to_string(),"cryptoType":0,"secretSeed":"never-copy-sentinel"}),
            )?,
        )?;
    }
    let hot = root.0.join("coldkey2/hotkeys");
    std::fs::write(
        hot.join("worker2"),
        serde_json::to_vec(
            &json!({"ss58Address":account.to_string(),"crypto_type":1,"privateKey":"never-copy-sentinel"}),
        )?,
    )?;
    std::fs::write(hot.join("worker10"), b"$NACL-ciphertext")?;
    std::fs::write(hot.join("worker2pub.txt"), b"ignored")?;
    std::fs::write(hot.join("worker2.lock"), b"ignored")?;
    let (matches, mut ctx, sink, reads) =
        fixture(&root, &["rtao", "wallet", "list"], true, false, &[])?;
    ctx.books = Books::from_values(
        json!([{"name":"contact","address":account.to_string()}]),
        json!([{"name":"proxy","address":account.to_string()}]),
        json!([]),
    );
    run(&mut ctx, &matches, &mut PromptHistory::default()).await?;
    let value = payload(&sink)?;
    assert_eq!(value["coldkeys"][0]["coldkey"], "coldkey2");
    assert_eq!(value["coldkeys"][1]["coldkey"], "coldkey10");
    assert_eq!(
        value["coldkeys"][0]["hotkeys"]
            .as_array()
            .ok_or("hotkey list")?
            .len(),
        2
    );
    assert_eq!(value["coldkeys"][0]["hotkeys"][0]["name"], "worker2");
    assert_eq!(value["coldkeys"][0]["hotkeys"][1]["ss58"], Value::Null);
    assert_eq!(value["addresses"], json!(ctx.books.addresses));
    assert!(!sink.text(Stream::Stdout)?.contains("sentinel"));
    assert_eq!(reads.load(Ordering::Relaxed), 0);
    Ok(())
}

#[test]
fn natural_sort_matches_python_digit_runs_casefold_and_unicode_data_version() {
    use std::cmp::Ordering::*;
    assert_eq!(wallet::natural_cmp("coldkey2", "coldkey10"), Less);
    assert_eq!(wallet::natural_cmp("coldkey٢", "coldkey10"), Less);
    assert_eq!(wallet::natural_cmp("coldkey0002", "coldkey2"), Equal);
    assert_eq!(wallet::natural_cmp("Straße", "STRASSE"), Equal);
    assert_eq!(
        wallet::natural_cmp(
            "x99999999999999999999999999999",
            "x100000000000000000000000000000"
        ),
        Less
    );
    assert_ne!(
        wallet::natural_cmp("\u{1c89}", "\u{1c8a}"),
        Equal,
        "Unicode 16 case pair is unassigned in Python 3.12"
    );
}

#[tokio::test]
async fn password_env_export_is_pure_and_uses_the_existing_keyfile_encoder() -> TestResult {
    let root = Root::new()?;
    for json_mode in [false, true] {
        let (matches, mut ctx, sink, reads) = fixture(
            &root,
            &["rtao", "wallet", "save-password-env"],
            json_mode,
            false,
            &[],
        )?;
        // The command only computes this path; it must not create a wallet.
        ctx.globals.wallet_path = PathBuf::from("/rtao_wallet_export_fixture");
        ctx.passwords.password = Some(SecretString::new("env-password-sentinel".into()));
        let wallet = ctx.wallet()?;
        let name = rtao::wallet::env_var_name(wallet.coldkey_file().path());
        let before = std::env::var_os(&name);
        run(&mut ctx, &matches, &mut PromptHistory::default()).await?;
        let output = sink.text(Stream::Stdout)?;
        let command = if json_mode {
            let value: Value = serde_json::from_str(&output)?;
            assert_eq!(value["name"], name);
            assert_eq!(
                value["export"],
                format!(
                    "export {name}={}",
                    value["value"].as_str().ok_or("encoded password")?
                )
            );
            value["export"].as_str().ok_or("export text")?.to_owned()
        } else {
            output.trim_end().to_owned()
        };
        let encoded = command
            .strip_prefix(&format!("export {name}="))
            .ok_or("shell assignment")?;
        assert_eq!(
            rtao::core::keyfile_env::decode_password_from_environment(&name, encoded)?,
            "env-password-sentinel"
        );
        assert_eq!(std::env::var_os(&name), before);
        assert!(!output.contains("env-password-sentinel"));
        assert!(sink.text(Stream::Stderr)?.is_empty());
        assert_eq!(reads.load(Ordering::Relaxed), 0);
    }
    let (matches, mut ctx, sink, reads) = fixture(
        &root,
        &["rtao", "wallet", "save-password-env"],
        true,
        false,
        &[],
    )?;
    ctx.globals.wallet_path = PathBuf::from("/invalid-wallet-path");
    ctx.passwords.password = Some(SecretString::new("unprinted-sentinel".into()));
    let error = run(&mut ctx, &matches, &mut PromptHistory::default())
        .await
        .err()
        .ok_or("invalid shell name must refuse")?;
    assert_eq!(error.code, 2);
    assert!(sink.text(Stream::Stdout)?.is_empty());
    assert_eq!(reads.load(Ordering::Relaxed), 0);
    Ok(())
}

#[tokio::test]
async fn password_export_sink_failure_emits_one_guarded_document_and_redacts_the_error()
-> TestResult {
    struct ClosedSink(Mutex<Vec<Vec<u8>>>);
    impl OutputSink for ClosedSink {
        fn write(&self, stream: Stream, bytes: &[u8]) -> std::io::Result<()> {
            assert_eq!(stream, Stream::Stdout);
            self.0
                .lock()
                .map_err(|_| std::io::Error::other("fixture sink poisoned"))?
                .push(bytes.to_vec());
            Err(std::io::Error::new(
                std::io::ErrorKind::BrokenPipe,
                "fixture output closed",
            ))
        }
    }
    let root = Root::new()?;
    for json_mode in [false, true] {
        let (matches, mut ctx, _, reads) = fixture(
            &root,
            &["rtao", "wallet", "save-password-env"],
            json_mode,
            false,
            &[],
        )?;
        ctx.globals.wallet_path = PathBuf::from("/rtao_wallet_export_fixture");
        ctx.passwords.password = Some(SecretString::new("disposable export sentinel π".into()));
        let sink = Arc::new(ClosedSink(Mutex::new(Vec::new())));
        ctx.output = Output::with_sink(
            OutputOptions {
                json_mode,
                quiet: true,
                network: None,
            },
            sink.clone(),
        );
        let wallet = ctx.wallet()?;
        let name = rtao::wallet::env_var_name(wallet.coldkey_file().path());
        let error = run(&mut ctx, &matches, &mut PromptHistory::default())
            .await
            .err()
            .ok_or("closed output must fail")?;
        assert_eq!(error.message, "fixture output closed");
        let documents = sink.0.lock().map_err(|_| "fixture sink")?;
        assert_eq!(documents.len(), 1);
        let text = std::str::from_utf8(&documents[0])?;
        let command = if json_mode {
            let value: Value = serde_json::from_str(text)?;
            assert_eq!(value["name"], name);
            assert_eq!(value.as_object().ok_or("export object")?.len(), 3);
            assert_eq!(
                value["export"],
                format!(
                    "export {name}={}",
                    value["value"].as_str().ok_or("encoded password")?
                )
            );
            value["export"].as_str().ok_or("export command")?.to_owned()
        } else {
            text.trim_end().to_owned()
        };
        let encoded = command
            .strip_prefix(&format!("export {name}="))
            .ok_or("shell assignment")?;
        assert_eq!(
            rtao::core::keyfile_env::decode_password_from_environment(&name, encoded)?,
            "disposable export sentinel π"
        );
        assert!(!error.message.contains(encoded));
        assert!(!error.message.contains("sentinel"));
        assert_eq!(reads.load(Ordering::Relaxed), 0);
    }
    assert!(std::fs::read_dir(&root.0)?.next().is_none());
    Ok(())
}

#[cfg(unix)]
#[test]
fn fifo_json_import_and_public_listing_refuse_without_a_writer() -> TestResult {
    use std::os::unix::fs::FileTypeExt;
    use std::process::{Command, Output as ProcessOutput, Stdio};
    use std::time::{Duration, Instant};

    fn bounded_output(command: &mut Command) -> std::io::Result<ProcessOutput> {
        let mut child = command
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()?;
        let start = Instant::now();
        loop {
            match child.try_wait() {
                Ok(Some(_)) => return child.wait_with_output(),
                Ok(None) => {}
                Err(error) => {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err(error);
                }
            }
            if start.elapsed() >= Duration::from_secs(5) {
                let _ = child.kill();
                let _ = child.wait();
                return Err(std::io::Error::new(
                    std::io::ErrorKind::TimedOut,
                    "wallet command blocked opening a FIFO without a writer",
                ));
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    }
    let root = Root::new()?;
    let import = root.0.join("import.fifo");
    let public = root.0.join("default/coldkeypub.txt");
    std::fs::create_dir_all(public.parent().ok_or("public parent")?)?;
    for path in [&import, &public] {
        assert!(Command::new("mkfifo").arg(path).status()?.success());
    }
    let make_command = || {
        let mut command = Command::new(env!("CARGO_BIN_EXE_rtao"));
        command
            .env("XDG_CONFIG_HOME", root.0.join("config"))
            .env("XDG_CACHE_HOME", root.0.join("cache"));
        command
    };
    let mut command = make_command();
    let result = bounded_output(command.args([
        "wallet",
        "regen-coldkey",
        "--wallet-path",
        root.0.to_str().ok_or("root UTF-8")?,
        "--json-path",
        import.to_str().ok_or("FIFO UTF-8")?,
        "--json-password",
        "unread-password-sentinel",
        "--no-password",
        "--json",
        "--quiet",
    ]))?;
    assert_eq!(result.status.code(), Some(2));
    assert!(
        result.stdout.is_empty(),
        "refusal must not emit wallet data"
    );
    let diagnostic: Value = serde_json::from_slice(&result.stderr)?;
    assert_eq!(
        diagnostic["error"],
        "JSON keystore must be a readable regular file no larger than 16 MiB"
    );
    assert!(!String::from_utf8(result.stderr)?.contains("unread-password-sentinel"));
    let mut command = make_command();
    let result = bounded_output(command.args([
        "wallet",
        "list",
        "--wallet-path",
        root.0.to_str().ok_or("root UTF-8")?,
        "--json",
        "--quiet",
    ]))?;
    assert!(result.status.success());
    let value: Value = serde_json::from_slice(&result.stdout)?;
    assert_eq!(value["coldkeys"][0]["ss58"], Value::Null);
    assert!(std::fs::symlink_metadata(import)?.file_type().is_fifo());
    Ok(())
}
