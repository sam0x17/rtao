// Derived from RaoFoundation/subtensor sdk/python/bittensor/cli/commands/timelock.py @ 850f2755bb05a3851f948489f0ab27b0645cebce (Apache-2.0); modified by rtao.
//! Standalone timelock encryption, decryption and offline envelope inspection.

use crate::{
    context::AppContext,
    error::CliError,
    globals::{Tier, with_globals},
    prompt::{PromptError, PromptHistory, PromptSpec, PromptValidation, Terminal, ask_with},
};
use clap::{Arg, ArgAction, ArgMatches, Command};
use rtao::{
    client::config::{MAX_JSON_FILE_BYTES, read_bounded_json_file},
    tx::timelock::{self, DrandClient, OpenOptions, Reveal, TimelockError, Timelocked},
};
use serde_json::{Value, json};
use std::{
    future::Future,
    path::{Path, PathBuf},
    pin::Pin,
    time::Duration,
};
use zeroize::Zeroizing;

// Reserve a KiB for core framing, and allow an optional hex prefix. Check the
// actual envelope before emitting it too: every successful encryption must be
// consumable through both bounded raw-file and inline-hex acquisition paths.
/// Maximum encrypted envelope size; its prefixed hex fits the 16 MiB acquisition limit.
pub const MAX_CIPHERTEXT_BYTES: usize = (MAX_JSON_FILE_BYTES - 2) / 2;
/// Maximum CLI plaintext size (just under 8 MiB).
pub const MAX_PLAINTEXT_BYTES: usize = MAX_CIPHERTEXT_BYTES - 1024;
const INLINE_DECODE_BYTES: usize = 64 * 1024;
const PAYLOAD_HELP: &str = "Text to seal, or a path to a file whose contents to seal.";
const CIPHERTEXT_HELP: &str = "Ciphertext hex (from encrypt), or a path to a ciphertext file.";
const DURATION_HELP: &str =
    "Duration until the reveal: '30s', '15m', '1h30m', '2d', or plain seconds.";

/// An injected crypto operation; no beacon or cryptography is needed by CLI fixtures.
pub type TimelockFuture<'a, T> =
    Pin<Box<dyn Future<Output = Result<T, TimelockError>> + Send + 'a>>;

/// Explicit crypto and beacon boundary for standalone timelock commands.
pub trait TimelockBackend: Send + Sync {
    /// Encrypt the borrowed payload for the selected reveal moment.
    fn seal<'a>(&'a self, payload: &'a [u8], reveal: Reveal) -> TimelockFuture<'a, Timelocked>;
    /// Open an envelope using the CLI's explicit wait and timeout choices.
    fn open<'a>(
        &'a self,
        sealed: &'a Timelocked,
        options: OpenOptions,
    ) -> TimelockFuture<'a, Vec<u8>>;
}

struct NativeTimelock;
impl TimelockBackend for NativeTimelock {
    fn seal<'a>(&'a self, payload: &'a [u8], reveal: Reveal) -> TimelockFuture<'a, Timelocked> {
        Box::pin(timelock::seal(payload, reveal))
    }
    fn open<'a>(
        &'a self,
        sealed: &'a Timelocked,
        options: OpenOptions,
    ) -> TimelockFuture<'a, Vec<u8>> {
        Box::pin(async move { timelock::open(sealed, &DrandClient::new()?, options).await })
    }
}

fn source(command: Command, encrypt: bool) -> Command {
    let (data_help, file_help, limit_help) = if encrypt {
        (
            PAYLOAD_HELP,
            "Read the payload from this file instead of the argument.",
            "Regular files only; plaintext must be just under 8 MiB.",
        )
    } else {
        (
            CIPHERTEXT_HELP,
            "Read the ciphertext from this file (raw bytes or hex).",
            "Regular files only; ciphertext input is limited to 16 MiB.",
        )
    };
    command.arg(Arg::new("data").help(data_help)).arg(
        Arg::new("file")
            .long("file")
            .help(file_help)
            .long_help(format!("{file_help}\n\n{limit_help}")),
    )
}
fn destination(command: Command, encrypt: bool) -> Command {
    command.arg(Arg::new("out").long("out").help(if encrypt {
        "Write raw ciphertext bytes here instead of printing hex."
    } else {
        "Write the decrypted bytes here instead of printing."
    }))
}

/// Build the three `misc timelock` commands; old top-level aliases are added by the application.
pub fn command() -> Command {
    Command::new("timelock")
        .about("Timelock encryption: seal data anyone can open at a set future time.")
        .long_about(format!(
            "Timelock encryption: seal data anyone can open at a set future time.\n\nGuide: {}/guides/timelock",
            rtao_catalog::py::DOCS_URL
        ))
        .subcommand_required(true)
        .arg_required_else_help(true)
        .subcommand(with_globals(
            destination(
                source(
                    Command::new("encrypt")
                        .about("Seal text or a file until a future moment; prints the ciphertext hex.")
                        .long_about(
                            "Seal text or a file until a future moment; prints the ciphertext hex.\n\nThe reveal round is embedded in the ciphertext, so the hex is all anyone needs to decrypt later -- no key, no metadata, no account.",
                        ),
                    true,
                ),
                true,
            )
            .arg(
                Arg::new("reveal_in")
                    .long("in")
                    .conflicts_with_all(["reveal_at", "reveal_round", "reveal_blocks"])
                    .help(DURATION_HELP),
            )
            .arg(
                Arg::new("reveal_at")
                    .long("at")
                    .conflicts_with_all(["reveal_in", "reveal_round", "reveal_blocks"])
                    .help("Absolute reveal moment, ISO-8601: '2026-07-08T12:00Z' (no offset = UTC)."),
            )
            .arg(
                Arg::new("reveal_round")
                    .long("round")
                    .value_parser(clap::value_parser!(u64))
                    .conflicts_with_all(["reveal_in", "reveal_at", "reveal_blocks"])
                    .help("Explicit drand quicknet round number to encrypt to."),
            )
            .arg(
                Arg::new("reveal_blocks")
                    .long("blocks")
                    .value_parser(clap::value_parser!(u64))
                    .conflicts_with_all(["reveal_in", "reveal_at", "reveal_round"])
                    .help("Nominal 12-second blocks until reveal (rtao addition D-033)."),
            ),
            Tier::Read,
        ))
        .subcommand(with_globals(
            destination(
                source(
                    Command::new("decrypt")
                        .about("Open a timelocked ciphertext once its reveal moment has passed."),
                    false,
                ),
                false,
            )
            .arg(
                Arg::new("wait")
                    .long("wait")
                    .action(ArgAction::SetTrue)
                    .help("Sleep until the reveal moment instead of failing early."),
            )
            .arg(
                Arg::new("timeout")
                    .long("timeout")
                    .allow_hyphen_values(true)
                    .help("Give up after this many seconds (with --wait).")
                    .long_help(
                        "Give up after this many seconds (with --wait).\n\nTotal decryption deadline, including beacon access and crypto. Must be finite and non-negative.",
                    ),
            ),
            Tier::Read,
        ))
        .subcommand(with_globals(
            source(
                Command::new("show").about("Show when a ciphertext unlocks, without decrypting it."),
                false,
            ),
            Tier::Read,
        ))
}

fn text<'a>(matches: &'a ArgMatches, name: &str) -> Option<&'a str> {
    matches
        .try_get_one::<String>(name)
        .ok()
        .flatten()
        .map(String::as_str)
}
fn failure(error: impl std::fmt::Display) -> CliError {
    CliError::failure(error.to_string())
}
fn prompt_error(error: PromptError) -> CliError {
    let help = matches!(&error, PromptError::MissingRequired(_))
        .then(|| "pass the options explicitly, or run on a terminal to be prompted".into());
    CliError {
        code: error.exit_code(),
        message: error.to_string(),
        help,
    }
}
fn checked_input(value: &str) -> Result<(), CliError> {
    if value.len() > MAX_JSON_FILE_BYTES {
        return Err(CliError::input("timelock input exceeds 16 MiB limit"));
    }
    Ok(())
}

async fn prompt_missing(
    ctx: &AppContext,
    leaf: &ArgMatches,
    encrypt: bool,
    history: &mut PromptHistory,
) -> Result<(Option<Zeroizing<String>>, Option<String>), CliError> {
    let missing_data = text(leaf, "data").is_none() && text(leaf, "file").is_none();
    let missing_reveal = encrypt
        && ["reveal_in", "reveal_at"]
            .iter()
            .all(|name| text(leaf, name).is_none())
        && leaf.get_one::<u64>("reveal_round").is_none()
        && leaf.get_one::<u64>("reveal_blocks").is_none();
    if !missing_data && !missing_reveal {
        return Ok((None, None));
    }
    let context = ctx.prompt_context();
    let mut terminal = ctx.terminal.clone();
    if !context.interactive(&terminal) {
        let mut fields = Vec::new();
        if missing_data {
            fields.push(if encrypt { "data" } else { "ciphertext" }.into());
        }
        if missing_reveal {
            fields.push("--in".into());
        }
        return Err(prompt_error(PromptError::MissingRequired(fields)));
    }
    let data = if missing_data {
        let mut spec = PromptSpec::new("data", if encrypt { "data" } else { "ciphertext" });
        spec.positional = true;
        spec.help = Some(
            if encrypt {
                PAYLOAD_HELP
            } else {
                CIPHERTEXT_HELP
            }
            .into(),
        );
        let raw = loop {
            let (_, raw) = ask_with(&context, &mut terminal, &spec, |raw| {
                checked_input(raw).map_err(|error| PromptValidation::Message(error.message))?;
                if !encrypt && raw.len() <= INLINE_DECODE_BYTES && !Path::new(raw).is_file() {
                    Timelocked::parse(raw)
                        .map_err(|error| PromptValidation::Message(error.to_string()))?;
                }
                Ok(())
            })
            .map_err(prompt_error)?;
            if !encrypt && raw.len() > INLINE_DECODE_BYTES && !Path::new(&raw).is_file() {
                let (raw, parsed) = rtao::core::offload(move || {
                    let parsed = Timelocked::parse(raw.as_str());
                    (raw, parsed)
                })
                .await
                .map_err(failure)?;
                if let Err(error) = parsed {
                    terminal.message(&error.to_string()).map_err(prompt_error)?;
                    continue;
                }
                break raw;
            }
            break raw;
        };
        if !encrypt {
            history.record_answers(vec![raw.clone()]);
        }
        // Plaintext is deliberately omitted from replay history (D-042).
        Some(Zeroizing::new(raw))
    } else {
        None
    };
    let duration = if missing_reveal {
        let mut spec = PromptSpec::new("reveal_in", "--in");
        spec.help = Some(DURATION_HELP.into());
        let (_, raw) = ask_with(&context, &mut terminal, &spec, |raw| {
            timelock::parse_duration(raw)
                .map(|_| ())
                .map_err(|error| PromptValidation::Message(error.to_string()))
        })
        .map_err(prompt_error)?;
        history.record_answers(vec!["--in".into(), raw.clone()]);
        Some(raw)
    } else {
        None
    };
    Ok((data, duration))
}

async fn payload(data: Option<&str>, file: Option<&str>) -> Result<Zeroizing<Vec<u8>>, CliError> {
    if let Some(data) = data {
        checked_input(data)?;
    }
    let file = file.or_else(|| data.filter(|data| Path::new(data).is_file()));
    match file {
        Some(path) => {
            let path = PathBuf::from(path);
            let bytes = rtao::core::offload(move || rtao::wallet::read_keyfile_data(&path))
                .await
                .map_err(failure)?
                .map_err(failure)?;
            if bytes.len() > MAX_PLAINTEXT_BYTES {
                return Err(CliError::input(
                    "timelock plaintext exceeds the encryption limit (just under 8 MiB)",
                ));
            }
            Ok(bytes)
        }
        None => {
            let data = data.ok_or_else(|| CliError::input("missing data"))?;
            if data.len() > MAX_PLAINTEXT_BYTES {
                return Err(CliError::input(
                    "timelock plaintext exceeds the encryption limit (just under 8 MiB)",
                ));
            }
            Ok(Zeroizing::new(data.as_bytes().to_vec()))
        }
    }
}

async fn ciphertext(data: Option<&str>, file: Option<&str>) -> Result<Timelocked, CliError> {
    if let Some(data) = data {
        checked_input(data)?;
    }
    let file = file.or_else(|| data.filter(|data| Path::new(data).is_file()));
    if let Some(path) = file {
        let path = PathBuf::from(path);
        rtao::core::offload(move || {
            let raw = read_bounded_json_file(&path).map_err(failure)?;
            if raw.is_ascii() {
                let compact: String = raw
                    .iter()
                    .copied()
                    // Python str.split includes vertical tab and information separators.
                    .filter(|&byte| !matches!(byte, b'\t'..=b'\r' | b' ' | 0x1c..=0x1f))
                    .map(char::from)
                    .collect();
                let hex = compact.strip_prefix("0x").unwrap_or(&compact);
                if !compact.is_empty() && hex.bytes().all(|byte| byte.is_ascii_hexdigit()) {
                    return Timelocked::parse(compact.as_str()).map_err(failure);
                }
            }
            Timelocked::parse(raw).map_err(failure)
        })
        .await
        .map_err(failure)?
    } else {
        let data = data.ok_or_else(|| CliError::input("missing ciphertext"))?;
        if data.len() <= INLINE_DECODE_BYTES {
            return Timelocked::parse(data).map_err(failure);
        }
        let data = data.to_owned();
        rtao::core::offload(move || Timelocked::parse(data.as_str()).map_err(failure))
            .await
            .map_err(failure)?
    }
}

fn reveal(leaf: &ArgMatches, prompted: Option<&str>) -> Result<Reveal, CliError> {
    if let Some(value) = text(leaf, "reveal_in").or(prompted) {
        return Reveal::after(value).map_err(failure);
    }
    if let Some(value) = text(leaf, "reveal_at") {
        return Reveal::at(value).map_err(failure);
    }
    if let Some(value) = leaf.get_one::<u64>("reveal_round") {
        return Ok(Reveal::Round(*value));
    }
    if let Some(value) = leaf.get_one::<u64>("reveal_blocks") {
        if *value == 0 {
            return Err(CliError::input("reveal blocks must be positive"));
        }
        return Ok(Reveal::InBlocks(*value));
    }
    Err(CliError::input(
        "give exactly one reveal moment: --in, --at, --round, or --blocks",
    ))
}

fn note(sealed: &Timelocked) -> Result<String, CliError> {
    let when = sealed.reveal_at().map_err(failure)?.to_rfc3339();
    Ok(if sealed.revealed().map_err(failure)? {
        format!("revealed since {when} (round {})", sealed.reveal_round())
    } else {
        format!(
            "reveals at {when} (round {}, in {})",
            sealed.reveal_round(),
            timelock::format_duration(sealed.remaining().map_err(failure)?).map_err(failure)?
        )
    })
}

async fn envelope_hex(sealed: &Timelocked) -> Result<String, CliError> {
    if sealed.ciphertext().len() <= INLINE_DECODE_BYTES {
        return Ok(sealed.hex());
    }
    let bytes = sealed.ciphertext().to_vec();
    rtao::core::offload(move || hex::encode(bytes))
        .await
        .map_err(failure)
}

fn plaintext_display(
    mut plaintext: Zeroizing<Vec<u8>>,
) -> (Option<Zeroizing<String>>, Zeroizing<String>) {
    let encoded = Zeroizing::new(hex::encode(plaintext.as_slice()));
    let text = match String::from_utf8(std::mem::take(&mut *plaintext)) {
        Ok(text) => Some(Zeroizing::new(text)),
        Err(error) => {
            drop(Zeroizing::new(error.into_bytes()));
            None
        }
    };
    (text, encoded)
}

/// Run a standalone command; only `decrypt` consults the beacon network.
pub async fn run(
    ctx: &AppContext,
    group: &ArgMatches,
    history: &mut PromptHistory,
) -> Result<(), CliError> {
    run_with_backend(ctx, group, history, &NativeTimelock).await
}

/// Execute with injected cryptography and beacon access, retaining real argument and output handling.
pub async fn run_with_backend(
    ctx: &AppContext,
    group: &ArgMatches,
    history: &mut PromptHistory,
    backend: &dyn TimelockBackend,
) -> Result<(), CliError> {
    let (name, leaf) = group
        .subcommand()
        .ok_or_else(|| CliError::input("missing timelock command"))?;
    if text(leaf, "data").is_some() && text(leaf, "file").is_some() {
        return Err(if name == "encrypt" {
            failure("give the payload exactly one way")
                .with_help("pass text or a file path as an argument, or use --file")
        } else {
            failure("give the ciphertext exactly one way")
                .with_help("pass the hex or a file path as an argument, or use --file")
        });
    }
    let (prompted, duration) = prompt_missing(ctx, leaf, name == "encrypt", history).await?;
    let data = text(leaf, "data").or_else(|| prompted.as_deref().map(String::as_str));
    let file = text(leaf, "file");
    match name {
        "encrypt" => {
            if file.is_none() && data.is_some_and(|data| Path::new(data).is_file()) {
                ctx.output
                    .message(&format!("sealing file {}", data.unwrap_or_default()))
                    .map_err(failure)?;
            }
            let bytes = payload(data, file).await?;
            let reveal = reveal(leaf, duration.as_deref())?;
            let sealed = backend.seal(&bytes, reveal).await.map_err(failure)?;
            if sealed.ciphertext().len() > MAX_CIPHERTEXT_BYTES {
                return Err(CliError::input(
                    "timelock ciphertext exceeds the roundtrip-safe encryption limit",
                ));
            }
            let mut record = json!({
                "reveal_round":sealed.reveal_round(), "reveal_at":sealed.reveal_at().map_err(failure)?.to_rfc3339(),
                "reveal_in_seconds":sealed.remaining().map_err(failure)?.as_secs(), "bytes":sealed.ciphertext().len(),
            });
            if let Some(path) = text(leaf, "out").filter(|path| !path.is_empty()) {
                let bytes = sealed.ciphertext().to_vec();
                let path_owned = PathBuf::from(path);
                rtao::core::offload(move || std::fs::write(path_owned, bytes))
                    .await
                    .map_err(failure)?
                    .map_err(failure)?;
                record["path"] = json!(path);
                if ctx.output.options().json_mode {
                    ctx.output.value(&record).map_err(failure)?;
                } else {
                    ctx.output
                        .message(&format!(
                            "wrote {} bytes to {path}",
                            sealed.ciphertext().len()
                        ))
                        .map_err(failure)?;
                }
            } else if ctx.output.options().json_mode {
                let encoded = envelope_hex(&sealed).await?;
                record["ciphertext"] = json!(encoded);
                ctx.output.value(&record).map_err(failure)?;
            } else {
                ctx.output
                    .value(&envelope_hex(&sealed).await?)
                    .map_err(failure)?;
            }
            if !ctx.output.options().json_mode {
                ctx.output.message(&note(&sealed)?).map_err(failure)?;
            }
            Ok(())
        }
        "show" => {
            let sealed = ciphertext(data, file).await?;
            let fields = vec![
                ("reveal_round".into(), json!(sealed.reveal_round())),
                (
                    "reveal_at".into(),
                    json!(sealed.reveal_at().map_err(failure)?.to_rfc3339()),
                ),
                (
                    "revealed".into(),
                    json!(sealed.revealed().map_err(failure)?),
                ),
                (
                    "remaining".into(),
                    json!(
                        timelock::format_duration(sealed.remaining().map_err(failure)?)
                            .map_err(failure)?
                    ),
                ),
                ("bytes".into(), json!(sealed.ciphertext().len())),
            ];
            let mut record: Value = fields
                .iter()
                .cloned()
                .collect::<serde_json::Map<_, _>>()
                .into();
            record
                .as_object_mut()
                .ok_or_else(|| failure("invalid timelock record"))?
                .remove("remaining");
            record["remaining_seconds"] = json!(sealed.remaining().map_err(failure)?.as_secs());
            ctx.output
                .detail(Some("timelock"), &fields, Some(&record))
                .map_err(failure)
        }
        "decrypt" => {
            let sealed = ciphertext(data, file).await?;
            let options = OpenOptions {
                wait: leaf.get_flag("wait"),
                timeout: text(leaf, "timeout")
                    .map(|raw| {
                        let seconds =
                            rtao::primitives::pyfmt::parse_float(raw).ok_or_else(|| {
                                CliError::input("timeout must be finite and non-negative")
                            })?;
                        Duration::try_from_secs_f64(seconds)
                            .map_err(|_| CliError::input("timeout must be finite and non-negative"))
                    })
                    .transpose()?,
            };
            if options.wait
                && !sealed.revealed().map_err(failure)?
                && !ctx.output.options().json_mode
            {
                ctx.output
                    .message(&format!("waiting: {}", note(&sealed)?))
                    .map_err(failure)?;
            }
            let plaintext =
                Zeroizing::new(backend.open(&sealed, options).await.map_err(|error| {
                    if let TimelockError::NotReady { reveal_at, .. } = &error {
                        failure(&error).with_help(format!(
                            "re-run with --wait, or come back after {}",
                            reveal_at.to_rfc3339()
                        ))
                    } else {
                        failure(error)
                    }
                })?);
            let plaintext_len = plaintext.len();
            if let Some(path) = text(leaf, "out").filter(|path| !path.is_empty()) {
                let path_owned = PathBuf::from(path);
                rtao::core::offload(move || std::fs::write(path_owned, plaintext.as_slice()))
                    .await
                    .map_err(failure)?
                    .map_err(failure)?;
                if ctx.output.options().json_mode {
                    ctx.output.value(&json!({"path":path,"bytes":plaintext_len,"reveal_round":sealed.reveal_round()})).map_err(failure)
                } else {
                    ctx.output
                        .message(&format!("wrote {plaintext_len} bytes to {path}"))
                        .map_err(failure)
                }
            } else {
                let (decoded_text, encoded) = if plaintext.len() > INLINE_DECODE_BYTES {
                    rtao::core::offload(move || plaintext_display(plaintext))
                        .await
                        .map_err(failure)?
                } else {
                    plaintext_display(plaintext)
                };
                let text = decoded_text.as_deref().map(String::as_str);
                if ctx.output.options().json_mode {
                    let mut fields = vec![("reveal_round".into(), json!(sealed.reveal_round()))];
                    let mut secrets = vec![("plaintext_hex", encoded.as_str())];
                    if let Some(text) = text {
                        secrets.push(("plaintext", text));
                    } else {
                        fields.push(("plaintext".into(), Value::Null));
                    }
                    ctx.output
                        .secret_detail(None, &fields, &secrets)
                        .map_err(failure)
                } else {
                    ctx.output
                        .secret_line(&[text.unwrap_or(&encoded)])
                        .map_err(failure)?;
                    if text.is_none() {
                        ctx.output
                            .message("payload is binary; shown as hex (use --out to write bytes)")
                            .map_err(failure)?;
                    }
                    Ok(())
                }
            }
        }
        _ => Err(CliError::input("unknown timelock command")),
    }
}
