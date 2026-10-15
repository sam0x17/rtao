//! Source-derived fixtures for the shared M8 contact/proxy books and CLI routes.
//! These use public fixture JSON and fake RPC only; no account unlocks or devices.

use clap::ArgMatches;
use rtao::{
    client::{
        Client, ClientConfig, Network,
        config::{ConfigFile, Paths},
    },
    primitives::AccountId,
    signer::BoxFuture,
};
use rtao_cli::{
    application::command_tree,
    books::{AddressBook, Books, ProxyBook},
    commands::{
        addresses::{self, ContactImport, ContactSource, ImportedContact},
        proxy,
    },
    context::AppContext,
    error::CliError,
    globals::Globals,
    output::{MemorySink, Output, OutputOptions, Stream},
    prompt::{PromptError, Terminal},
};
use rtao_testkit::{FakeChain, FakeState, Reply, rpc::RpcClient};
use serde_json::{Map, Value, json};
use std::{
    collections::VecDeque,
    fs,
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, AtomicUsize, Ordering},
    },
};

type TestResult = Result<(), Box<dyn std::error::Error>>;
type FixtureContext = (AppContext, ArgMatches, MemorySink, Arc<AtomicUsize>);
const ALICE: AccountId = AccountId([3; 32]);
const BOB: AccountId = AccountId([4; 32]);

struct Files {
    root: PathBuf,
    paths: Paths,
}
impl Files {
    fn new() -> Result<Self, Box<dyn std::error::Error>> {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let base = std::env::var_os("RTAO_TEMP_ROOT")
            .map(PathBuf::from)
            .unwrap_or_else(|| {
                PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/m8-books-tests")
            });
        let parent = base.join("tmp");
        fs::create_dir_all(&parent)?;
        let root = loop {
            let path = parent.join(format!(
                "m8-books-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            match fs::create_dir(&path) {
                Ok(()) => break path,
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(error) => return Err(error.into()),
            }
        };
        let paths = Paths::new(&root, |key| match key {
            "BTCLI_ADDRESSES_PATH" => Some(root.join("shared/contacts.json").into_os_string()),
            "BTCLI_PROXIES_PATH" => Some(root.join("shared/proxies.json").into_os_string()),
            _ => None,
        });
        Ok(Self { root, paths })
    }
    fn write(&self, file: ConfigFile, bytes: &[u8]) -> TestResult {
        let path = self.paths.path(file);
        fs::create_dir_all(path.parent().ok_or("parent")?)?;
        fs::write(path, bytes)?;
        Ok(())
    }
    fn value(&self, file: ConfigFile) -> Result<Value, Box<dyn std::error::Error>> {
        Ok(rtao::primitives::json::from_slice(&fs::read(
            self.paths.path(file),
        )?)?)
    }
}
impl Drop for Files {
    fn drop(&mut self) {
        // This exact, exclusively created fixture holds public JSON only.
        // All offloaded writers/client operations have been awaited, and no
        // process, launchd unit, config, symlink or virtualenv depends on it.
        let _ = fs::remove_dir_all(&self.root);
    }
}

struct Prompt {
    tty: bool,
    replies: VecDeque<String>,
    reads: Arc<AtomicUsize>,
}
impl Terminal for Prompt {
    fn stdin_is_tty(&self) -> bool {
        self.tty
    }
    fn stderr_is_tty(&self) -> bool {
        self.tty
    }
    fn read_line(&mut self, _: &str) -> Result<String, PromptError> {
        self.reads.fetch_add(1, Ordering::SeqCst);
        self.replies.pop_front().ok_or(PromptError::Aborted)
    }
    fn read_secret(&mut self, _: &str) -> Result<rtao::wallet::SecretString, PromptError> {
        Err(PromptError::Unavailable)
    }
    fn message(&mut self, _: &str) -> Result<(), PromptError> {
        Ok(())
    }
}

fn context(
    argv: &[&str],
    tty: bool,
    replies: &[&str],
) -> Result<FixtureContext, Box<dyn std::error::Error>> {
    let root = command_tree([addresses::command(), proxy::command()?]);
    let parsed = root.try_get_matches_from(argv)?;
    let (name, group) = parsed.subcommand().ok_or("family")?;
    let mut leaf = group;
    while let Some((_, next)) = leaf.subcommand() {
        leaf = next;
    }
    let config = Map::new();
    let mut globals = Globals::resolve(
        &parsed,
        &config,
        Path::new("/fixture/nonexistent-home"),
        |_| None,
    )?;
    let reserved = if name == "addr" {
        addresses::reserved(group)
    } else {
        proxy::reserved(group)?
    };
    globals.apply_with_env(leaf, &reserved, |_| None)?;
    let reads = Arc::new(AtomicUsize::new(0));
    let mut ctx = AppContext::new(
        globals,
        config,
        Books::default(),
        Arc::new(Mutex::new(Box::new(Prompt {
            tty,
            replies: replies.iter().map(|value| (*value).to_owned()).collect(),
            reads: reads.clone(),
        }))),
    );
    let sink = MemorySink::default();
    ctx.output = Output::new(
        OutputOptions {
            json_mode: ctx.globals.json,
            quiet: ctx.globals.quiet,
            network: Some("local".into()),
        },
        Arc::new(sink.clone()),
    );
    Ok((ctx, group.clone(), sink, reads))
}
fn stdout(sink: &MemorySink) -> Result<Value, Box<dyn std::error::Error>> {
    Ok(rtao::primitives::json::from_str(
        &sink.text(Stream::Stdout)?,
    )?)
}
fn book_group(group: &ArgMatches) -> Result<&ArgMatches, Box<dyn std::error::Error>> {
    Ok(group.subcommand().ok_or("book")?.1)
}

struct Source {
    calls: Arc<Mutex<Vec<ContactImport>>>,
    value: Result<ImportedContact, &'static str>,
}
impl Source {
    fn contact(address: String, genesis: Option<String>) -> Self {
        Self {
            calls: Arc::new(Mutex::new(Vec::new())),
            value: Ok(ImportedContact { address, genesis }),
        }
    }
    fn unused() -> Self {
        Self {
            calls: Arc::new(Mutex::new(Vec::new())),
            value: Err("unexpected external import"),
        }
    }
}
impl ContactSource for Source {
    fn import<'a>(
        &'a self,
        _: &'a AppContext,
        kind: ContactImport,
    ) -> BoxFuture<'a, Result<ImportedContact, CliError>> {
        Box::pin(async move {
            self.calls
                .lock()
                .map_err(|_| CliError::failure("lock"))?
                .push(kind);
            self.value.clone().map_err(CliError::failure)
        })
    }
}

#[tokio::test]
async fn address_writers_share_paths_preserve_rows_reorder_replacement_and_exact_signer_tag()
-> TestResult {
    let files = Files::new()?;
    let book = AddressBook::new(&files.paths);
    assert_eq!(book.path(), files.paths.path(ConfigFile::Addresses));
    book.save(vec![
        json!({"name":"Treasury","address":BOB,"unknown":[7,9]}),
        json!({"name":"alice","address":ALICE,"signer":"vault","note":"old"}),
        json!({"name":"alice","address":BOB,"duplicate":true}),
    ])
    .await?;
    let saved = book
        .save_contact("alice".into(), BOB.to_string(), "new".into(), None)
        .await?;
    assert_eq!(
        saved,
        json!({"name":"alice","address":BOB,"note":"new","signer":"vault"})
    );
    assert_eq!(
        book.load().await?,
        vec![
            json!({"name":"Treasury","address":BOB,"unknown":[7,9]}),
            saved
        ]
    );
    assert!(!book.remove("ALICE").await?);
    assert!(book.remove("alice").await?);
    let loaded = Books::load(&files.paths).await?;
    assert_eq!(loaded.addresses, book.load().await?);
    let bytes = fs::read(book.path())?;
    assert!(bytes.ends_with(b"\n"));
    assert!(book.add(json!({"name":"","address":ALICE})).await.is_err());
    assert!(book.add(json!({"name":"bad","address":""})).await.is_err());
    assert_eq!(fs::read(book.path())?, bytes);
    let open_rows = vec![
        json!({"address":ALICE,"custom":1}),
        json!({"name":7,"address":BOB}),
    ];
    book.save(open_rows.clone()).await?;
    assert_eq!(book.load().await?, open_rows);
    let native_name = json!({"name":7,"address":ALICE});
    book.add(native_name.clone()).await?;
    assert_eq!(book.load().await?, vec![open_rows[0].clone(), native_name]);
    for (old, new, equal) in [
        (json!(1), json!(true), true),
        (json!(7), json!(7.0), true),
        (json!([{"nested":1}]), json!([{"nested":true}]), true),
        (
            rtao::primitives::json::from_str("9007199254740993")?,
            json!(9007199254740992.0),
            false,
        ),
        (
            rtao::primitives::json::from_str("1208925819614629174706176")?,
            json!(2_f64.powi(80)),
            true,
        ),
    ] {
        book.save(vec![json!({"name":old,"address":ALICE})]).await?;
        book.add(json!({"name":new,"address":BOB})).await?;
        assert_eq!(book.load().await?.len(), if equal { 1 } else { 2 });
    }
    Ok(())
}

#[tokio::test]
async fn proxy_updates_only_first_match_preserve_coordinates_and_do_not_rewrite_missing_names()
-> TestResult {
    let files = Files::new()?;
    let book = ProxyBook::new(&files.paths);
    assert_eq!(book.update("absent", Map::new()).await?, None);
    assert!(!book.path().exists());
    let first =
        json!({"name":"pure","address":ALICE,"height":123,"ext_index":7,"unknown":{"nested":true}});
    let duplicate = json!({"name":"pure","address":BOB,"height":999});
    book.save(vec![first.clone(), duplicate.clone()]).await?;
    let before = fs::read(book.path())?;
    assert_eq!(book.update("PURE", Map::new()).await?, None);
    assert_eq!(fs::read(book.path())?, before);
    let updated = book
        .update(
            "pure",
            Map::from_iter([
                ("name".into(), json!("ignored")),
                ("note".into(), json!("changed")),
                ("address".into(), json!("")),
            ]),
        )
        .await?
        .ok_or("updated")?;
    assert_eq!(updated["name"], "pure");
    assert_eq!(updated["height"], 123);
    assert_eq!(updated["ext_index"], 7);
    assert_eq!(updated["unknown"], first["unknown"]);
    assert_eq!(book.load().await?[1], duplicate);
    assert!(book.remove("pure").await?);
    assert_eq!(book.clear().await?, 0);
    assert_eq!(files.value(ConfigFile::Proxies)?, json!([]));
    let open_rows = vec![
        json!({"address":ALICE,"custom":1}),
        json!({"name":17,"address":BOB}),
        json!("legacy row"),
    ];
    book.save(open_rows.clone()).await?;
    assert_eq!(book.load().await?, open_rows);
    let native_name = json!({"name":17,"address":ALICE,"custom":2});
    book.add(native_name.clone()).await?;
    assert_eq!(
        book.load().await?,
        vec![open_rows[0].clone(), open_rows[2].clone(), native_name]
    );
    Ok(())
}

#[tokio::test]
async fn corrupt_books_are_read_only_until_a_valid_bounded_replacement_preserves_recovery_bytes()
-> TestResult {
    let files = Files::new()?;
    let original = b"{bad public JSON\n";
    files.write(ConfigFile::Addresses, original)?;
    let book = AddressBook::new(&files.paths);
    assert!(book.load().await?.is_empty());
    assert_eq!(fs::read(book.path())?, original);
    assert!(book.add(json!({"name":""})).await.is_err());
    assert_eq!(fs::read(book.path())?, original);
    book.add(json!({"name":"valid","address":ALICE})).await?;
    let backups =
        fs::read_dir(book.path().parent().ok_or("parent")?)?.collect::<Result<Vec<_>, _>>()?;
    let backup = backups
        .iter()
        .find(|entry| {
            entry
                .file_name()
                .to_string_lossy()
                .starts_with("contacts.json.corrupt.")
        })
        .ok_or("corrupt backup")?;
    assert_eq!(fs::read(backup.path())?, original);
    assert_eq!(
        book.load().await?,
        vec![json!({"name":"valid","address":ALICE})]
    );
    assert!(
        !backups
            .iter()
            .any(|entry| entry.file_name().to_string_lossy().contains(".tmp."))
    );
    Ok(())
}

#[tokio::test]
async fn oversized_input_and_output_leave_original_book_unchanged() -> TestResult {
    let files = Files::new()?;
    files.write(ConfigFile::Proxies, b"[]\n")?;
    let book = ProxyBook::new(&files.paths);
    assert!(
        book.add(json!({"name":"large","note":"x".repeat(16 * 1024 * 1024)}))
            .await
            .is_err()
    );
    assert_eq!(fs::read(book.path())?, b"[]\n");
    let mut deep = json!("leaf");
    for _ in 0..129 {
        deep = json!([deep]);
    }
    assert!(
        book.add(json!({"name":"deep","nested":deep}))
            .await
            .is_err()
    );
    assert_eq!(fs::read(book.path())?, b"[]\n");
    book.add(json!({"name":"existing","address":ALICE})).await?;
    let before = fs::read(book.path())?;
    let mut deep = json!("leaf");
    for _ in 0..129 {
        deep = json!([deep]);
    }
    assert!(
        book.update("existing", Map::from_iter([("nested".into(), deep)]))
            .await
            .is_err()
    );
    assert_eq!(fs::read(book.path())?, before);
    assert!(
        book.update(
            "existing",
            Map::from_iter([("note".into(), json!("x".repeat(16 * 1024 * 1024)))])
        )
        .await
        .is_err()
    );
    assert_eq!(fs::read(book.path())?, before);
    fs::OpenOptions::new()
        .write(true)
        .open(book.path())?
        .set_len(16 * 1024 * 1024 + 1)?;
    assert!(book.load().await.is_err());
    assert!(book.clear().await.is_err());
    assert_eq!(fs::metadata(book.path())?.len(), 16 * 1024 * 1024 + 1);
    Ok(())
}

#[cfg(unix)]
#[tokio::test]
async fn file_modes_survive_replacement_readers_follow_regular_links_writers_refuse_them()
-> TestResult {
    use std::os::unix::fs::{PermissionsExt, symlink};
    let files = Files::new()?;
    let book = AddressBook::new(&files.paths);
    book.add(json!({"name":"new","address":ALICE})).await?;
    assert_eq!(
        fs::metadata(book.path())?.permissions().mode() & 0o777,
        0o600
    );
    fs::set_permissions(book.path(), fs::Permissions::from_mode(0o640))?;
    book.add(json!({"name":"other","address":BOB})).await?;
    assert_eq!(
        fs::metadata(book.path())?.permissions().mode() & 0o777,
        0o640
    );
    let actual = files.root.join("public-target.json");
    fs::rename(book.path(), &actual)?;
    symlink(&actual, book.path())?;
    let bytes = fs::read(&actual)?;
    assert_eq!(book.load().await?.len(), 2);
    assert!(book.remove("new").await.is_err());
    assert_eq!(fs::read(&actual)?, bytes);
    fs::remove_file(book.path())?;
    fs::rename(actual, book.path())?;
    fs::set_permissions(book.path(), fs::Permissions::from_mode(0o0))?;
    // Hosted Ubuntu's ordinary non-root user has no access. Do not turn an
    // acquisition refusal into corrupt recovery or publish over the file.
    let result = book.remove("new").await;
    fs::set_permissions(book.path(), fs::Permissions::from_mode(0o640))?;
    assert!(result.is_err());
    assert_eq!(fs::read(book.path())?, bytes);
    Ok(())
}

#[cfg(unix)]
#[tokio::test]
async fn special_book_destinations_fail_without_blocking_or_publication() -> TestResult {
    use std::os::unix::net::UnixListener;
    let files = Files::new()?;
    fs::create_dir_all(
        files
            .paths
            .path(ConfigFile::Proxies)
            .parent()
            .ok_or("parent")?,
    )?;
    let book = ProxyBook::new(&files.paths);
    let listener = UnixListener::bind(book.path())?;
    assert!(book.load().await?.is_empty());
    assert!(
        book.add(json!({"name":"no","address":ALICE}))
            .await
            .is_err()
    );
    drop(listener);
    fs::remove_file(book.path())?;
    Ok(())
}

#[tokio::test]
async fn contact_commands_preserve_json_shapes_retag_notes_and_command_owned_signer() -> TestResult
{
    let files = Files::new()?;
    let alice = ALICE.to_string();
    let source = Source::unused();
    let (ctx, group, sink, _) = context(
        &[
            "rtao",
            "--signer",
            "wallet",
            "addresses",
            "add",
            "alice",
            &alice,
            "--signer",
            " VaUlT ",
            "--note",
            "before",
            "--json",
        ],
        false,
        &[],
    )?;
    assert_eq!(ctx.globals.signer_backend.as_deref(), Some("wallet"));
    addresses::run_with_source(&ctx, &files.paths, &group, &source).await?;
    let saved = json!({"name":"alice","address":ALICE,"note":"before","signer":"vault"});
    assert_eq!(
        stdout(&sink)?,
        json!({"entry":saved,"path":files.paths.path(ConfigFile::Addresses)})
    );
    let (ctx, group, sink, reads) = context(
        &[
            "rtao", "addr", "add", "alice", "--signer", "ledger", "--json",
        ],
        false,
        &[],
    )?;
    addresses::run_with_source(&ctx, &files.paths, &group, &source).await?;
    assert_eq!(
        stdout(&sink)?["entry"],
        json!({"name":"alice","address":ALICE,"note":"before","signer":"ledger"})
    );
    assert_eq!(reads.load(Ordering::SeqCst), 0);
    let (ctx, group, sink, _) = context(
        &[
            "rtao", "addr", "save", "alice", &alice, "--note", "", "--json",
        ],
        false,
        &[],
    )?;
    addresses::run_with_source(&ctx, &files.paths, &group, &source).await?;
    assert_eq!(stdout(&sink)?["entry"]["signer"], "ledger");
    let (ctx, group, sink, _) = context(&["rtao", "addr", "list", "--json"], false, &[])?;
    addresses::run_with_source(&ctx, &files.paths, &group, &source).await?;
    assert_eq!(
        stdout(&sink)?,
        json!({"path":files.paths.path(ConfigFile::Addresses),"addresses":[{"name":"alice","address":ALICE,"note":"","signer":"ledger"}]})
    );
    for (argv, expected) in [
        (
            vec!["rtao", "addr", "show", "alice", "--json"],
            json!({"name":"alice","address":ALICE,"note":"","signer":"ledger"}),
        ),
        (
            vec!["rtao", "addr", "remove", "ALICE", "--json"],
            json!({"name":"ALICE","existed":false}),
        ),
        (
            vec!["rtao", "addr", "remove", "alice", "--json"],
            json!({"name":"alice","existed":true}),
        ),
    ] {
        let (ctx, group, sink, _) = context(&argv, false, &[])?;
        addresses::run_with_source(&ctx, &files.paths, &group, &source).await?;
        assert_eq!(stdout(&sink)?, expected);
    }
    assert!(source.calls.lock().map_err(|_| "lock")?.is_empty());
    let malformed = br#"[{"name":"malformed","address":17,"note":false}]"#;
    files.write(ConfigFile::Addresses, malformed)?;
    let (ctx, group, _, reads) = context(
        &["rtao", "addr", "add", "malformed", "--signer", "ledger"],
        true,
        &[],
    )?;
    assert!(
        addresses::run_with_source(&ctx, &files.paths, &group, &source)
            .await
            .is_err()
    );
    assert_eq!(reads.load(Ordering::SeqCst), 0);
    assert_eq!(
        fs::read(files.paths.path(ConfigFile::Addresses))?,
        malformed
    );
    Ok(())
}

#[tokio::test]
async fn contact_conflicts_and_invalid_inputs_fail_before_import_or_writes() -> TestResult {
    let files = Files::new()?;
    let alice = ALICE.to_string();
    let source = Source::contact(alice.clone(), None);
    for args in [
        vec!["rtao", "addr", "add", "bad", "--vault", "--extension"],
        vec!["rtao", "addr", "add", "bad", &alice, "--vault"],
        vec![
            "rtao", "addr", "add", "bad", "--vault", "--signer", "ledger",
        ],
        vec![
            "rtao",
            "addr",
            "add",
            "bad",
            "--extension",
            "--signer",
            "unknown",
        ],
        vec!["rtao", "addr", "add", "bad", "--signer", "wallet"],
        vec!["rtao", "addr", "add", "bad", "not-ss58"],
        vec!["rtao", "addr", "add", "bad", "--json"],
    ] {
        let (ctx, group, _, reads) = context(&args, false, &[])?;
        assert!(
            addresses::run_with_source(&ctx, &files.paths, &group, &source)
                .await
                .is_err()
        );
        assert_eq!(reads.load(Ordering::SeqCst), 0);
        assert!(!files.paths.path(ConfigFile::Addresses).exists());
    }
    assert!(source.calls.lock().map_err(|_| "lock")?.is_empty());
    Ok(())
}

#[tokio::test]
async fn contact_imports_are_explicit_public_only_and_other_genesis_is_advisory() -> TestResult {
    let files = Files::new()?;
    let source = Source::contact(ALICE.to_string(), Some("0x1234567890abcdef".into()));
    let (ctx, group, sink, _) = context(&["rtao", "addr", "add", "phone", "--vault"], false, &[])?;
    addresses::run_with_source(&ctx, &files.paths, &group, &source).await?;
    assert!(
        sink.text(Stream::Stderr)?
            .contains("saving the address anyway")
    );
    assert_eq!(files.value(ConfigFile::Addresses)?[0]["signer"], "vault");
    let source = Source::contact(BOB.to_string(), None);
    let (ctx, group, sink, _) = context(
        &["rtao", "addr", "add", "browser", "--extension", "--json"],
        false,
        &[],
    )?;
    addresses::run_with_source(&ctx, &files.paths, &group, &source).await?;
    assert_eq!(
        *source.calls.lock().map_err(|_| "lock")?,
        vec![ContactImport::Extension]
    );
    assert_eq!(stdout(&sink)?["entry"]["signer"], "extension");
    let source = Source::contact("bad-public-address".into(), None);
    let (ctx, group, _, _) = context(
        &["rtao", "addr", "add", "invalid", "--vault", "--json"],
        false,
        &[],
    )?;
    let bytes = fs::read(files.paths.path(ConfigFile::Addresses))?;
    assert!(
        addresses::run_with_source(&ctx, &files.paths, &group, &source)
            .await
            .is_err()
    );
    assert_eq!(fs::read(files.paths.path(ConfigFile::Addresses))?, bytes);
    Ok(())
}

#[tokio::test]
async fn contact_missing_address_prompts_only_in_an_interactive_session() -> TestResult {
    let files = Files::new()?;
    let alice = ALICE.to_string();
    let (ctx, group, _, reads) = context(
        &["rtao", "addr", "add", "prompted"],
        true,
        &["invalid", &alice],
    )?;
    addresses::run_with_source(&ctx, &files.paths, &group, &Source::unused()).await?;
    assert_eq!(reads.load(Ordering::SeqCst), 2);
    assert_eq!(files.value(ConfigFile::Addresses)?[0]["address"], alice);
    Ok(())
}

#[tokio::test]
async fn public_contact_controls_remain_exact_in_json_and_are_escaped_for_humans() -> TestResult {
    let files = Files::new()?;
    let name = "contact\u{1b}[31m";
    let note = "private label\nfor accounting\u{202e}";
    AddressBook::new(&files.paths)
        .add(json!({"name":name,"address":ALICE,"note":note,"signer":"vault"}))
        .await?;
    for json_mode in [false, true] {
        let mut args = vec!["rtao", "addr", "list"];
        if json_mode {
            args.push("--json");
        }
        let (ctx, group, sink, _) = context(&args, false, &[])?;
        addresses::run_with_source(&ctx, &files.paths, &group, &Source::unused()).await?;
        if json_mode {
            assert_eq!(stdout(&sink)?["addresses"][0]["name"], name);
            assert_eq!(stdout(&sink)?["addresses"][0]["note"], note);
        } else {
            let text = sink.text(Stream::Stdout)?;
            assert!(!text.contains('\u{1b}'));
            assert!(!text.contains('\u{202e}'));
            assert!(text.contains("contact\\x1b[31m"));
            assert!(text.contains("note · private label\\nfor accounting\\u{202e}"));
            assert!(text.contains("signer · vault"));
            assert!(text.contains("1 entry"));
        }
    }
    Ok(())
}

#[tokio::test]
async fn proxy_book_commands_preserve_exact_coordinate_numbers_and_update_omission_semantics()
-> TestResult {
    let files = Files::new()?;
    let alice = ALICE.to_string();
    let source_number = "1208925819614629174706176";
    let (ctx, group, sink, _) = context(
        &[
            "rtao",
            "proxy",
            "book",
            "add",
            "--name",
            "pure",
            "--address",
            &alice,
            "--proxy-type",
            "Any",
            "--delay",
            "-1",
            "--index",
            "0",
            "--height",
            source_number,
            "--ext-index",
            "9",
            "--note",
            "keep",
            "--json",
        ],
        false,
        &[],
    )?;
    proxy::book_execute(&ctx, &files.paths, book_group(&group)?, None).await?;
    let row = stdout(&sink)?["entry"].clone();
    assert_eq!(row["height"].to_string(), source_number);
    assert_eq!(row["delay"], -1);
    assert_eq!(row["ext_index"], 9);
    assert_eq!(row["spawner"], "");
    let (ctx, group, sink, _) = context(
        &[
            "rtao", "proxy", "book", "update", "pure", "--note", "", "--json",
        ],
        false,
        &[],
    )?;
    proxy::book_execute(&ctx, &files.paths, book_group(&group)?, None).await?;
    let updated = stdout(&sink)?;
    assert_eq!(updated["note"], "");
    assert_eq!(updated["height"], row["height"]);
    assert_eq!(updated["address"], row["address"]);
    let (ctx, group, sink, _) = context(&["rtao", "proxy", "book", "list", "--json"], false, &[])?;
    proxy::book_execute(&ctx, &files.paths, book_group(&group)?, None).await?;
    assert_eq!(
        stdout(&sink)?,
        json!({"path":files.paths.path(ConfigFile::Proxies),"proxies":[updated]})
    );
    let (ctx, group, sink, _) = context(&["rtao", "proxy", "book", "clear", "--json"], false, &[])?;
    proxy::book_execute(&ctx, &files.paths, book_group(&group)?, None).await?;
    assert_eq!(
        stdout(&sink)?,
        json!({"removed":1,"path":files.paths.path(ConfigFile::Proxies)})
    );
    Ok(())
}

#[tokio::test]
async fn proxy_book_verification_uses_fake_chain_statuses_without_persisting_them_or_submitting()
-> TestResult {
    let files = Files::new()?;
    let mut state = FakeState::try_default()?;
    let pure = AccountId([7; 32]);
    state.seed_python("Proxy", "Proxies", &[json!(pure)], json!([[{"delegate":ALICE,"proxy_type":"Any","delay":0},{"delegate":BOB,"proxy_type":"Staking","delay":7}],0]))?;
    let rows = vec![
        json!({"name":"missing"}),
        json!({"name":"empty","address":AccountId([8;32])}),
        json!({"name":"count","address":pure}),
        json!({"name":"ok","address":pure,"spawner":ALICE,"proxy_type":"Any"}),
        json!({"name":"mismatch","address":pure,"spawner":BOB,"proxy_type":"Any"}),
        json!({"name":"stale","address":pure,"spawner":AccountId([9;32])}),
    ];
    ProxyBook::new(&files.paths).save(rows.clone()).await?;
    let before = fs::read(files.paths.path(ConfigFile::Proxies))?;
    let chain = FakeChain::from_state(state);
    let mut config = ClientConfig::new(Network::resolve_with("local", |_| None)?)?;
    config.track_heads = false;
    config.runtime_cache_dir = None;
    config.config_store = None;
    let client = Client::from_rpc(RpcClient::new(chain.clone()), config).await?;
    let (ctx, group, sink, _) = context(
        &["rtao", "proxy", "book", "list", "--verify", "--json"],
        false,
        &[],
    )?;
    proxy::book_execute(&ctx, &files.paths, book_group(&group)?, Some(&client)).await?;
    let result = stdout(&sink)?;
    let statuses: Vec<_> = result["proxies"]
        .as_array()
        .ok_or("proxies")?
        .iter()
        .map(|row| row["status"].as_str().unwrap_or_default())
        .collect();
    assert_eq!(
        statuses,
        [
            "no address recorded",
            "stale: account has no on-chain delegations",
            "ok: 2 on-chain delegations",
            "ok",
            "mismatch: spawner is a delegate but with a different proxy type",
            "stale: spawner is not a delegate of this account"
        ]
    );
    assert_eq!(fs::read(files.paths.path(ConfigFile::Proxies))?, before);
    assert!(chain.mutate_state(|state| state.submissions.is_empty()));
    client.close().await;
    // A failed verification is not a cached status and never rewrites a book.
    let mut config = ClientConfig::new(Network::resolve_with("local", |_| None)?)?;
    config.track_heads = false;
    config.runtime_cache_dir = None;
    config.config_store = None;
    let client = Client::from_rpc(RpcClient::new(chain.clone()), config).await?;
    chain.mutate_state(|state| {
        for method in [
            "state_getStorage",
            "state_getStorageAt",
            "state_queryStorageAt",
        ] {
            state.set_handler(method, |_, _| {
                Reply::error(-32602, "fixture storage refusal", None)
            });
        }
    });
    let (ctx, group, sink, _) = context(
        &["rtao", "proxy", "book", "list", "--verify", "--json"],
        false,
        &[],
    )?;
    assert!(
        proxy::book_execute(&ctx, &files.paths, book_group(&group)?, Some(&client))
            .await
            .is_err()
    );
    assert!(sink.text(Stream::Stdout)?.is_empty());
    assert_eq!(fs::read(files.paths.path(ConfigFile::Proxies))?, before);
    assert!(chain.mutate_state(|state| state.submissions.is_empty()));
    client.close().await;
    Ok(())
}

#[tokio::test]
async fn empty_verified_proxy_list_stays_offline_and_invalid_numbers_do_not_publish() -> TestResult
{
    let files = Files::new()?;
    let (ctx, group, sink, _) = context(
        &["rtao", "proxy", "book", "list", "--verify", "--json"],
        false,
        &[],
    )?;
    proxy::book_run(&ctx, &files.paths, book_group(&group)?).await?;
    assert_eq!(
        stdout(&sink)?,
        json!({"path":files.paths.path(ConfigFile::Proxies),"proxies":[]})
    );
    let (ctx, group, _, _) = context(
        &[
            "rtao",
            "proxy",
            "book",
            "add",
            "--name",
            "bad",
            "--address",
            "opaque-public-value",
            "--delay",
            "1.5",
        ],
        false,
        &[],
    )?;
    assert!(
        proxy::book_execute(&ctx, &files.paths, book_group(&group)?, None)
            .await
            .is_err()
    );
    assert!(!files.paths.path(ConfigFile::Proxies).exists());
    Ok(())
}
