//! Public file inputs pass through the real CLI parsers before connecting or signing.

use rtao::client::config::MAX_JSON_FILE_BYTES;
use rtao_cli::{
    application::command_tree,
    books::Books,
    call::{self, RawCallRequest},
    commands::{generated::TxRequest, weights},
    context::AppContext,
    error::CliError,
    globals::Globals,
    output::{MemorySink, Output, OutputOptions},
    prompt::{PromptError, Terminal},
};
use serde_json::json;
use std::{
    error::Error,
    fs::{self, File},
    io::{Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
    time::{Duration, Instant},
};

type TestResult = Result<(), Box<dyn Error>>;

struct Fixture(PathBuf);
impl Fixture {
    fn new() -> std::io::Result<Self> {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let base = std::env::var_os("RTAO_TEMP_ROOT")
            .map(PathBuf::from)
            .unwrap_or_else(|| {
                PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/cli-file-input-tests")
            });
        let parent = base.join("tmp");
        fs::create_dir_all(&parent)?;
        let root = parent.join(format!(
            "file-input-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        // Refuse a stale collision rather than adopting another task's files.
        fs::create_dir(&root)?;
        Ok(Self(root))
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        // Parser fixtures start no services or background tasks. The FIFO
        // subprocess is killed and reaped before this exact root is removed.
        let _ = fs::remove_dir_all(&self.0);
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
    fn read_secret(&mut self, _: &str) -> Result<rtao::wallet::SecretString, PromptError> {
        Err(PromptError::Unavailable)
    }
    fn message(&mut self, _: &str) -> Result<(), PromptError> {
        Ok(())
    }
}

// These production request boundaries receive neither a client nor a key
// source. They must finish input acquisition before either can be requested.
fn raw_request(home: &Path, flag: &str, value: &str) -> Result<RawCallRequest, CliError> {
    let parsed = command_tree([call::command()])
        .try_get_matches_from(["rtao", "call", "System.remark", flag, value])
        .map_err(|error| CliError::input(error.to_string()))?;
    let matches = parsed
        .subcommand()
        .ok_or_else(|| CliError::input("missing call fixture"))?
        .1;
    let config = serde_json::Map::new();
    let mut globals =
        Globals::resolve(&parsed, &config, home, |_| None).map_err(CliError::input)?;
    globals
        .apply_with_env(matches, &["signer_backend"], |_| None)
        .map_err(CliError::input)?;
    let mut ctx = AppContext::new(
        globals,
        config,
        Books::default(),
        Arc::new(Mutex::new(Box::new(NoTerminal))),
    );
    ctx.output = Output::new(OutputOptions::default(), Arc::new(MemorySink::default()));
    RawCallRequest::from_matches(&ctx, matches)
}

fn weights_request(path: &Path) -> Result<TxRequest, CliError> {
    let path = path
        .to_str()
        .ok_or_else(|| CliError::input("fixture path is not UTF-8"))?;
    let parsed = command_tree([weights::command()])
        .try_get_matches_from([
            "rtao",
            "weights",
            "set",
            "--netuid",
            "3",
            "--weights-file",
            path,
        ])
        .map_err(|error| CliError::input(error.to_string()))?;
    weights::request(
        parsed
            .subcommand()
            .ok_or_else(|| CliError::input("missing weights fixture"))?
            .1,
    )
}

fn raw_file_request(home: &Path, path: &Path) -> Result<RawCallRequest, CliError> {
    raw_request(
        home,
        "--args-file",
        path.to_str()
            .ok_or_else(|| CliError::input("fixture path is not UTF-8"))?,
    )
}

fn padded_json(payload: &[u8], length: usize) -> Vec<u8> {
    let mut bytes = payload.to_vec();
    bytes.resize(length, b' ');
    bytes
}

#[test]
fn raw_and_weights_accept_exact_sixteen_mib_files_without_rewriting() -> TestResult {
    assert_eq!(MAX_JSON_FILE_BYTES, 16 * 1024 * 1024);
    let fixture = Fixture::new()?;
    let raw_path = fixture.0.join("args.json");
    let raw_bytes = padded_json(br#"{"remark":"0x0102"}"#, MAX_JSON_FILE_BYTES);
    fs::write(&raw_path, &raw_bytes)?;
    let raw = raw_file_request(&fixture.0, &raw_path)?;
    assert_eq!(
        raw.params,
        serde_json::Map::from_iter([("remark".into(), json!("0x0102"))])
    );
    assert_eq!(raw.args_file.as_deref(), Some(raw_path.as_path()));
    assert_eq!(fs::read(&raw_path)?, raw_bytes);

    let weights_path = fixture.0.join("weights.json");
    let weights_bytes = padded_json(br#"{"2":3,"0":1}"#, MAX_JSON_FILE_BYTES);
    fs::write(&weights_path, &weights_bytes)?;
    let weights = weights_request(&weights_path)?;
    assert_eq!(weights.op, "set_weights");
    assert_eq!(weights.args["weights"], json!({"2":3,"0":1}));
    let canonical = weights.intent()?.as_dyn().to_json_dyn();
    assert_eq!(canonical["uids"], json!([0, 2]));
    assert_eq!(canonical["weights"], json!([1.0, 3.0]));
    assert_eq!(fs::read(&weights_path)?, weights_bytes);
    assert_eq!(fs::read_dir(&fixture.0)?.count(), 2);
    Ok(())
}

#[test]
fn raw_inline_json_accepts_the_boundary_and_rejects_one_extra_byte() -> TestResult {
    let fixture = Fixture::new()?;
    let mut raw = String::from_utf8(padded_json(br#"{"remark":"0x0102"}"#, MAX_JSON_FILE_BYTES))?;
    assert_eq!(
        raw_request(&fixture.0, "--args", &raw)?.params["remark"],
        json!("0x0102")
    );
    raw.push(' ');
    let error = raw_request(&fixture.0, "--args", &raw)
        .err()
        .ok_or("oversized inline JSON must refuse")?;
    assert_eq!(error.code, 2);
    assert_eq!(error.message, "JSON input exceeds 16 MiB limit");
    assert!(fs::read_dir(&fixture.0)?.next().is_none());
    Ok(())
}

#[test]
fn weights_inline_total_and_injected_json_reader_are_bounded_before_conversion() -> TestResult {
    let uids = format!("{}1", " ".repeat(MAX_JSON_FILE_BYTES / 2));
    let values = format!("{}1", " ".repeat(MAX_JSON_FILE_BYTES / 2));
    let parsed = command_tree([weights::command()]).try_get_matches_from([
        "rtao",
        "weights",
        "set",
        "--netuid",
        "3",
        "--uids",
        uids.as_str(),
        "--weights",
        values.as_str(),
    ])?;
    let error = weights::request(parsed.subcommand().ok_or("weights fixture")?.1)
        .err()
        .ok_or("combined inline bytes must refuse")?;
    assert_eq!(error.code, 2);
    assert_eq!(error.message, "Weights input exceeds 16 MiB limit");

    let parsed = command_tree([weights::command()]).try_get_matches_from([
        "rtao",
        "weights",
        "set",
        "--netuid",
        "3",
        "--weights-file",
        "/unused/injected.json",
    ])?;
    let mut data = String::from_utf8(padded_json(br#"{"1":1}"#, MAX_JSON_FILE_BYTES))?;
    data.push(' ');
    let error =
        weights::request_with_reader(parsed.subcommand().ok_or("weights fixture")?.1, |_| {
            Ok(data)
        })
        .err()
        .ok_or("injected oversized JSON must refuse")?;
    assert_eq!(error.code, 2);
    assert_eq!(error.message, "Weights input exceeds 16 MiB limit");
    Ok(())
}

#[test]
fn sparse_oversized_files_refuse_promptly_and_keep_their_contents() -> TestResult {
    let fixture = Fixture::new()?;
    let path = fixture.0.join("sparse.json");
    let prefix = br#"{"remark":"0x0102"}"#;
    let mut file = File::create(&path)?;
    file.write_all(prefix)?;
    let length = MAX_JSON_FILE_BYTES as u64 + 1;
    file.set_len(length)?;
    drop(file);

    let started = Instant::now();
    let raw_error = raw_file_request(&fixture.0, &path)
        .err()
        .ok_or("oversized raw file must refuse")?;
    let weights_error = weights_request(&path)
        .err()
        .ok_or("oversized weights file must refuse")?;
    assert!(started.elapsed() < Duration::from_secs(5));
    for error in [raw_error, weights_error] {
        assert_eq!(error.code, 2);
        assert!(error.message.contains("JSON input exceeds 16 MiB limit"));
    }
    assert_eq!(fs::metadata(&path)?.len(), length);
    let mut file = File::open(&path)?;
    let mut actual = vec![0; prefix.len()];
    file.read_exact(&mut actual)?;
    assert_eq!(actual, prefix);
    file.seek(SeekFrom::End(-1))?;
    let mut last = [1];
    file.read_exact(&mut last)?;
    assert_eq!(last, [0]);
    assert_eq!(fs::read_dir(&fixture.0)?.count(), 1);
    Ok(())
}

#[test]
fn directory_inputs_refuse_and_preserve_the_directory() -> TestResult {
    let fixture = Fixture::new()?;
    let path = fixture.0.join("directory.json");
    fs::create_dir(&path)?;
    let marker = path.join("keep.json");
    fs::write(&marker, b"{\"keep\":true}")?;
    let started = Instant::now();
    let raw_error = raw_file_request(&fixture.0, &path)
        .err()
        .ok_or("raw directory must refuse")?;
    let weights_error = weights_request(&path)
        .err()
        .ok_or("weights directory must refuse")?;
    assert!(started.elapsed() < Duration::from_secs(5));
    assert_eq!(raw_error.code, 2);
    assert!(raw_error.message.starts_with("cannot read "));
    assert_eq!(weights_error.code, 2);
    assert!(
        weights_error
            .message
            .starts_with("Cannot read weights file:")
    );
    assert!(path.is_dir());
    assert_eq!(fs::read(marker)?, b"{\"keep\":true}");
    assert_eq!(fs::read_dir(&path)?.count(), 1);
    assert_eq!(fs::read_dir(&fixture.0)?.count(), 1);
    Ok(())
}

#[cfg(unix)]
#[test]
fn regular_symlinks_remain_valid_and_preserve_the_link_and_target() -> TestResult {
    use std::os::unix::fs::symlink;

    let fixture = Fixture::new()?;
    for (name, bytes) in [
        ("args", br#"{"remark":"0x0102"}"#.as_slice()),
        ("weights", br#"{"2":3,"0":1}"#.as_slice()),
    ] {
        let target = fixture.0.join(format!("{name}.json"));
        let link = fixture.0.join(format!("{name}-link.json"));
        fs::write(&target, bytes)?;
        symlink(&target, &link)?;
        if name == "args" {
            assert_eq!(
                raw_file_request(&fixture.0, &link)?.params["remark"],
                json!("0x0102")
            );
        } else {
            assert_eq!(
                weights_request(&link)?.args["weights"],
                json!({"2":3,"0":1})
            );
        }
        assert!(fs::symlink_metadata(&link)?.file_type().is_symlink());
        assert_eq!(fs::read_link(&link)?, target);
        assert_eq!(fs::read(&link)?, bytes);
        assert_eq!(fs::read(&target)?, bytes);
    }
    assert_eq!(fs::read_dir(&fixture.0)?.count(), 4);
    Ok(())
}

#[cfg(unix)]
#[test]
fn fifo_inputs_refuse_without_a_writer_with_a_bounded_process() -> TestResult {
    use std::{os::unix::fs::FileTypeExt, process::Command};

    let fixture = Fixture::new()?;
    let path = fixture.0.join("input.fifo");
    assert!(Command::new("mkfifo").arg(&path).status()?.success());
    let mut command = Command::new(std::env::current_exe()?);
    command
        .args(["--exact", "fifo_entrypoints_probe", "--nocapture"])
        .env("RTAO_FILE_INPUT_FIFO_PROBE", &path);
    let output = bounded_output(&mut command)?;
    assert!(
        output.status.success(),
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(String::from_utf8(output.stdout)?.contains("1 passed"));
    assert!(fs::symlink_metadata(&path)?.file_type().is_fifo());
    assert_eq!(fs::read_dir(&fixture.0)?.count(), 1);
    Ok(())
}

#[cfg(unix)]
#[test]
fn fifo_entrypoints_probe() -> TestResult {
    let Some(path) = std::env::var_os("RTAO_FILE_INPUT_FIFO_PROBE").map(PathBuf::from) else {
        return Ok(());
    };
    let home = path.parent().ok_or("FIFO fixture parent")?;
    let raw_error = raw_file_request(home, &path)
        .err()
        .ok_or("raw FIFO must refuse")?;
    let weights_error = weights_request(&path)
        .err()
        .ok_or("weights FIFO must refuse")?;
    for error in [raw_error, weights_error] {
        assert_eq!(error.code, 2);
        assert!(error.message.contains("expected a regular JSON file"));
    }
    Ok(())
}

#[cfg(unix)]
#[test]
fn actual_cli_config_startup_preserves_an_oversized_source() -> TestResult {
    use std::process::Command;

    let fixture = Fixture::new()?;
    let path = fixture.0.join("btcli.json");
    let original = br#"{"network":"test"}"#;
    let mut file = File::create(&path)?;
    file.write_all(original)?;
    file.set_len(MAX_JSON_FILE_BYTES as u64 + 1)?;
    drop(file);
    let mut command = Command::new(env!("CARGO_BIN_EXE_rtao"));
    command
        .args(["call", "System.remark", "--args", r#"{"remark":"0x01"}"#])
        .env("BTCLI_CONFIG", &path);
    let output = bounded_output(&mut command)?;
    assert_eq!(output.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&output.stderr).contains("JSON input exceeds 16 MiB limit"));
    assert!(output.stdout.is_empty());
    assert_eq!(fs::metadata(&path)?.len(), MAX_JSON_FILE_BYTES as u64 + 1);
    let mut actual = vec![0; original.len()];
    File::open(&path)?.read_exact(&mut actual)?;
    assert_eq!(actual, original);
    assert_eq!(fs::read_dir(&fixture.0)?.count(), 1);
    Ok(())
}

#[cfg(unix)]
fn bounded_output(command: &mut std::process::Command) -> std::io::Result<std::process::Output> {
    use std::process::Stdio;

    let mut child = command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    let started = Instant::now();
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
        if started.elapsed() >= Duration::from_secs(5) {
            let _ = child.kill();
            let _ = child.wait();
            return Err(std::io::Error::new(
                std::io::ErrorKind::TimedOut,
                "CLI file parser blocked opening a FIFO without a writer",
            ));
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}
