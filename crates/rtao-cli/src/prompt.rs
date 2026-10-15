// Derived from RaoFoundation/subtensor sdk/python/bittensor/cli/prompt.py @ 850f2755bb05a3851f948489f0ab27b0645cebce (Apache-2.0); modified by rtao.
// Additional sources at the same commit/license: sdk/python/bittensor/keyfiles.py:42-62 and sdk/python/bittensor/cli/context.py:1900-1935.
//! Terminal-gated, injectable prompting and secret password retry policy.

use rtao::wallet::{PasswordPrompt, PromptPolicy, WalletError};
use secrecy::{ExposeSecret, SecretString};
use serde_json::{Map, Value};
use std::{
    fmt,
    future::Future,
    io::{self, IsTerminal, Write},
    sync::{Arc, Mutex},
};

/// A prompt failure with an explicit command-line exit status.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PromptError {
    /// Every required option missing from a non-interactive invocation.
    MissingRequired(Vec<String>),
    /// Input is unavailable under the current terminal/output policy.
    Unavailable,
    /// End of input or an interrupted prompt.
    Aborted,
    /// A terminal operation failed, with its raw error deliberately discarded.
    Io,
    /// A context-specific custom flow has no installed resolver.
    UnsupportedFlow,
    /// A wallet selection rejected before prompting, with public remediation.
    InvalidValue(String),
}
impl PromptError {
    /// Python-compatible missing-option and abort exit codes.
    pub fn exit_code(&self) -> u8 {
        match self {
            Self::MissingRequired(_) | Self::InvalidValue(_) => 2,
            Self::Aborted => 130,
            _ => 1,
        }
    }
}
impl fmt::Display for PromptError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MissingRequired(flags) => write!(
                f,
                "missing required option{}: {}",
                if flags.len() > 1 { "s" } else { "" },
                flags
                    .iter()
                    .map(|flag| format!("`{flag}`"))
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
            Self::Unavailable => f.write_str("prompt requires an interactive terminal"),
            Self::Aborted => f.write_str("aborted."),
            Self::Io => f.write_str("terminal input/output failed"),
            Self::UnsupportedFlow => f.write_str("prompt flow has no context resolver"),
            Self::InvalidValue(message) => f.write_str(message),
        }
    }
}
impl std::error::Error for PromptError {}

/// Terminal interface used by prompts without process-global test overrides.
pub trait Terminal: Send {
    /// Whether standard input is a terminal.
    fn stdin_is_tty(&self) -> bool;
    /// Whether the diagnostics stream is a terminal.
    fn stderr_is_tty(&self) -> bool;
    /// Read an ordinary, echoing line; EOF and interrupts are aborts.
    fn read_line(&mut self, label: &str) -> Result<String, PromptError>;
    /// Read without echo; returned secret material must never be logged.
    fn read_secret(&mut self, label: &str) -> Result<SecretString, PromptError>;
    /// Write a public prompt hint or diagnostic to stderr.
    fn message(&mut self, message: &str) -> Result<(), PromptError>;
}

/// One invocation's shared terminal, used by ordinary and wallet secret prompts.
pub type SharedTerminal = Arc<Mutex<Box<dyn Terminal>>>;

impl<T: Terminal + ?Sized> Terminal for Box<T> {
    fn stdin_is_tty(&self) -> bool {
        (**self).stdin_is_tty()
    }
    fn stderr_is_tty(&self) -> bool {
        (**self).stderr_is_tty()
    }
    fn read_line(&mut self, label: &str) -> Result<String, PromptError> {
        (**self).read_line(label)
    }
    fn read_secret(&mut self, label: &str) -> Result<SecretString, PromptError> {
        (**self).read_secret(label)
    }
    fn message(&mut self, message: &str) -> Result<(), PromptError> {
        (**self).message(message)
    }
}
impl Terminal for SharedTerminal {
    fn stdin_is_tty(&self) -> bool {
        self.lock().is_ok_and(|terminal| terminal.stdin_is_tty())
    }
    fn stderr_is_tty(&self) -> bool {
        self.lock().is_ok_and(|terminal| terminal.stderr_is_tty())
    }
    fn read_line(&mut self, label: &str) -> Result<String, PromptError> {
        self.lock()
            .map_err(|_| PromptError::Io)?
            .read_line(&crate::output::terminal_text(label))
    }
    fn read_secret(&mut self, label: &str) -> Result<SecretString, PromptError> {
        self.lock()
            .map_err(|_| PromptError::Io)?
            .read_secret(&crate::output::terminal_text(label))
    }
    fn message(&mut self, message: &str) -> Result<(), PromptError> {
        self.lock()
            .map_err(|_| PromptError::Io)?
            .message(&crate::output::terminal_text(message))
    }
}

/// Standard-input/stderr terminal with rpassword hidden entry.
#[derive(Default)]
pub struct LiveTerminal;
fn input_error(error: io::Error) -> PromptError {
    match error.kind() {
        io::ErrorKind::Interrupted | io::ErrorKind::UnexpectedEof => PromptError::Aborted,
        _ => PromptError::Io,
    }
}
impl Terminal for LiveTerminal {
    fn stdin_is_tty(&self) -> bool {
        io::stdin().is_terminal()
    }
    fn stderr_is_tty(&self) -> bool {
        io::stderr().is_terminal()
    }
    fn read_line(&mut self, label: &str) -> Result<String, PromptError> {
        if !self.stdin_is_tty() || !self.stderr_is_tty() {
            return Err(PromptError::Unavailable);
        }
        let mut stderr = io::stderr().lock();
        stderr
            .write_all(crate::output::terminal_text(label).as_bytes())
            .map_err(input_error)?;
        stderr.flush().map_err(input_error)?;
        drop(stderr);
        let mut answer = String::new();
        if io::stdin().read_line(&mut answer).map_err(input_error)? == 0 {
            return Err(PromptError::Aborted);
        }
        Ok(answer)
    }
    fn read_secret(&mut self, label: &str) -> Result<SecretString, PromptError> {
        if !self.stdin_is_tty() {
            return Err(PromptError::Unavailable);
        }
        rpassword::prompt_password(crate::output::terminal_text(label))
            .map(SecretString::new)
            .map_err(input_error)
    }
    fn message(&mut self, message: &str) -> Result<(), PromptError> {
        let mut stderr = io::stderr().lock();
        writeln!(stderr, "{}", crate::output::terminal_text(message)).map_err(input_error)
    }
}

/// Context flags used for human prompts; no config or environment is read here.
#[derive(Clone, Copy, Debug)]
pub struct PromptContext {
    /// CLI uses IfTty; Never disables input even on a real terminal.
    pub policy: PromptPolicy,
    /// JSON output must never wait for terminal input.
    pub json: bool,
    /// Quiet output disables prompts.
    pub quiet: bool,
    /// Skip default confirmations and richer intent pickers.
    pub assume_yes: bool,
    /// Skip local signing-wallet confirmation for external signers.
    pub external_signer: bool,
    /// Skip richer intent pickers for the extension signer.
    pub extension_signer: bool,
}
impl Default for PromptContext {
    fn default() -> Self {
        Self {
            policy: PromptPolicy::IfTty,
            json: false,
            quiet: false,
            assume_yes: false,
            external_signer: false,
            extension_signer: false,
        }
    }
}
impl PromptContext {
    /// Python's human-mode stdin/stderr TTY test, with Never honored explicitly.
    pub fn interactive(&self, terminal: &dyn Terminal) -> bool {
        self.policy != PromptPolicy::Never
            && !self.json
            && !self.quiet
            && terminal.stdin_is_tty()
            && terminal.stderr_is_tty()
    }
    /// Hidden keyfile entry follows stdin-only IfTty, independently of JSON/quiet.
    pub fn password_interactive(&self, terminal: &dyn Terminal) -> bool {
        self.policy != PromptPolicy::Never && terminal.stdin_is_tty()
    }
}

/// Context-specific custom prompt flow selected by an intent policy.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PromptFlow {
    /// Ordinary typed value, parsed by the command's resolver.
    Plain,
    /// Local wallet selection, with command-specific requirements.
    Wallet {
        /// Require a usable coldkey.
        require_coldkey: bool,
        /// Accept a saved multisig book name.
        allow_multisig: bool,
    },
    /// Local hotkey selection in the chosen wallet.
    Hotkey,
    /// A new wallet name need not exist on disk yet.
    WalletName,
    /// A new hotkey name need not exist on disk yet.
    HotkeyName,
    /// A coldkey target accepts addresses and book names before local wallets.
    TargetWallet {
        /// Demand a usable local coldkey when a local wallet is chosen.
        require_coldkey: bool,
        /// Permit saved multisig names for this target.
        allow_multisig: bool,
    },
    /// A hotkey target accepts addresses, book names and wallet/hotkey references.
    TargetHotkey,
    /// Select an owned stake source and optionally its subnet.
    StakeSource {
        /// Intent field populated by the chosen source's subnet.
        netuid_field: Option<String>,
    },
    /// Select a staking destination before entering its amount.
    StakeTarget,
    /// Resolve an address-book, local key, or pasted destination.
    DestinationAccount,
    /// Select a root-claim source without falling back to an arbitrary hotkey.
    ClaimRootSource,
}

/// One required option or defaulted selection and its rendering metadata.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PromptSpec {
    /// Intent/context field name.
    pub field: String,
    /// Long option spelling, or positional display name.
    pub flag: String,
    /// Help text displayed before the prompt.
    pub help: Option<String>,
    /// Shape hint, such as integer or y/n.
    pub placeholder: Option<String>,
    /// Enter-to-accept default; defaulted options do not fail without a TTY.
    pub default: Option<String>,
    /// Positional values are replayed without an option token.
    pub positional: bool,
    /// Context resolver invoked for this prompt.
    pub flow: PromptFlow,
    /// Decorate an amount prompt with the current free-balance hint.
    pub free_balance: bool,
}
impl PromptSpec {
    /// Create a required ordinary option prompt.
    pub fn new(field: impl Into<String>, flag: impl Into<String>) -> Self {
        Self {
            field: field.into(),
            flag: flag.into(),
            help: None,
            placeholder: None,
            default: None,
            positional: false,
            flow: PromptFlow::Plain,
            free_balance: false,
        }
    }
    fn label(&self) -> String {
        let mut label = format!("  {}", self.flag.trim_start_matches('-'));
        if let Some(shape) = &self.placeholder {
            label.push_str(&format!(" ({shape})"));
        }
        if let Some(default) = &self.default {
            label.push_str(&format!(" [{default}]"));
        }
        label.push_str(": ");
        label
    }
    /// Whether a richer multi-field picker replaces ordinary typed entry.
    pub fn custom(&self) -> bool {
        matches!(
            self.flow,
            PromptFlow::StakeSource { .. }
                | PromptFlow::StakeTarget
                | PromptFlow::DestinationAccount
                | PromptFlow::ClaimRootSource
        )
    }
}

/// A parser rejection, or a resolver that already displayed its own error.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PromptValidation {
    /// Public validation text; only ordinary non-secret answers use this path.
    Message(String),
    /// A richer resolver already displayed its diagnostic.
    Reported,
}

/// Context-dependent address, wallet, type and picker resolution.
pub trait PromptResolver {
    /// Parse one ordinary answer; failures cause a re-ask.
    fn parse(&mut self, spec: &PromptSpec, raw: &str) -> Result<Value, PromptValidation>;
    /// Execute a richer multi-field picker and return replayable public tokens.
    fn custom(
        &mut self,
        _spec: &PromptSpec,
        _terminal: &mut dyn Terminal,
        _answers: &mut Map<String, Value>,
    ) -> Result<Vec<String>, PromptError> {
        Err(PromptError::UnsupportedFlow)
    }
    /// Add a free-balance hint without changing the prompt's position.
    fn decorate(&mut self, _spec: &mut PromptSpec) -> Result<(), PromptError> {
        Ok(())
    }
}

/// Strip exactly Python's whitespace, including ASCII U+001C through U+001F.
pub fn python_strip(value: &str) -> &str {
    value.trim_matches(|character: char| {
        character.is_whitespace() || matches!(character, '\u{1c}'..='\u{1f}')
    })
}

/// Prompt until a non-empty/defaulted ordinary answer parses successfully.
pub fn ask_with<T>(
    context: &PromptContext,
    terminal: &mut dyn Terminal,
    spec: &PromptSpec,
    mut parse: impl FnMut(&str) -> Result<T, PromptValidation>,
) -> Result<(T, String), PromptError> {
    if !context.interactive(terminal) {
        return Err(PromptError::Unavailable);
    }
    if let Some(help) = &spec.help {
        terminal.message(&format!("{}: {help}", spec.flag))?;
    }
    let label = spec.label();
    loop {
        // Callers can use visible prompts for sensitive payload text. Preserve
        // ordinary parsing while erasing the terminal's original owned answer.
        let answer = zeroize::Zeroizing::new(terminal.read_line(&label)?);
        let raw = python_strip(&answer);
        let raw = if raw.is_empty() {
            match &spec.default {
                Some(default) => default.as_str(),
                None => {
                    terminal.message("a value is required")?;
                    continue;
                }
            }
        } else {
            raw
        };
        match parse(raw) {
            Ok(value) => return Ok((value, raw.to_owned())),
            Err(PromptValidation::Message(error)) => {
                if error.is_empty() {
                    terminal.message(&format!("invalid value for `{}`", spec.flag))?;
                } else {
                    terminal.message(&error)?;
                }
            }
            Err(PromptValidation::Reported) => {}
        }
    }
}

/// Collect every missing option with validation, preserving Python's defaults.
pub fn fill_missing(
    context: &PromptContext,
    terminal: &mut dyn Terminal,
    missing: &[PromptSpec],
    answers: &mut Map<String, Value>,
    resolver: &mut dyn PromptResolver,
    history: &mut PromptHistory,
) -> Result<(), PromptError> {
    if !context.interactive(terminal) {
        let hard = missing
            .iter()
            .filter(|spec| spec.default.is_none())
            .map(|spec| spec.flag.clone())
            .collect::<Vec<_>>();
        return if hard.is_empty() {
            Ok(())
        } else {
            Err(PromptError::MissingRequired(hard))
        };
    }
    for spec in missing {
        if supplied(answers, &spec.field) {
            continue;
        }
        let mut spec = spec.clone();
        if spec.free_balance {
            resolver.decorate(&mut spec)?;
        }
        if spec.custom() {
            history.record_answers(resolver.custom(&spec, terminal, answers)?);
        } else {
            let (value, raw) =
                ask_with(context, terminal, &spec, |raw| resolver.parse(&spec, raw))?;
            answers.insert(spec.field.clone(), value);
            history.record_answers(if spec.positional {
                vec![raw]
            } else {
                vec![spec.flag, raw]
            });
        }
    }
    Ok(())
}

/// A non-null field is supplied, including false, zero and empty strings.
pub fn supplied(answers: &Map<String, Value>, field: &str) -> bool {
    answers.get(field).is_some_and(|value| !value.is_null())
}

/// Create signing-wallet/hotkey confirmations in Python's order.
pub fn signer_specs(
    wallet_name: &str,
    hotkey_name: &str,
    signer: &str,
    wallet_given: bool,
    hotkey_given: bool,
) -> Vec<PromptSpec> {
    let mut specs = Vec::new();
    if !wallet_given {
        let mut spec = PromptSpec::new("wallet", "--wallet");
        spec.help = Some(
            if signer == "hotkey" {
                "Wallet containing the signing hotkey."
            } else {
                "Wallet whose coldkey signs this transaction."
            }
            .into(),
        );
        spec.default = Some(wallet_name.into());
        spec.flow = PromptFlow::Wallet {
            require_coldkey: signer == "coldkey",
            allow_multisig: true,
        };
        specs.push(spec);
    }
    if signer == "hotkey" && !hotkey_given {
        let mut spec = PromptSpec::new("wallet_hotkey", "--wallet-hotkey");
        spec.help = Some("Hotkey (within the wallet) that signs this transaction.".into());
        spec.default = Some(hotkey_name.into());
        spec.flow = PromptFlow::Hotkey;
        specs.push(spec);
    }
    specs
}

/// Detect an option's primary/secondary spelling, including --flag=value.
pub fn option_supplied(args: &[String], spellings: &[&str]) -> bool {
    args.iter()
        .any(|token| spellings.contains(&token.split('=').next().unwrap_or(token)))
}

/// Default signing-wallet confirmation before a generic missing-value round.
pub fn signing_wallet_spec(
    context: &PromptContext,
    wallet_name: &str,
    wallet_given: bool,
    tier: &str,
    wallet_signs: bool,
    args: &[String],
) -> Option<PromptSpec> {
    if !matches!(tier, "tx" | "unlock")
        || !wallet_signs
        || wallet_given
        || context.assume_yes
        || context.external_signer
        || option_supplied(
            args,
            &["--wallet", "-w", "--yes", "-y", "--signer", "--ledger"],
        )
    {
        return None;
    }
    let mut spec = PromptSpec::new("wallet", "--wallet");
    spec.help = Some(
        if tier == "tx" {
            "Wallet that signs this transaction."
        } else {
            "Wallet this command targets."
        }
        .into(),
    );
    spec.default = Some(wallet_name.into());
    spec.flow = PromptFlow::Wallet {
        require_coldkey: false,
        allow_multisig: tier == "tx",
    };
    Some(spec)
}

/// Confirmed wallet/hotkey selection for one invocation.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct WalletSelection {
    /// Current configured or explicitly selected wallet.
    pub wallet_name: String,
    /// Current configured or explicitly selected hotkey.
    pub hotkey_name: String,
    /// Wallet was explicitly supplied or confirmed during this invocation.
    pub wallet_given: bool,
    /// Hotkey was explicitly supplied or confirmed during this invocation.
    pub hotkey_given: bool,
    /// Public names/accounts selected by prompts, available to the output layer.
    pub selected_accounts: Vec<(String, String)>,
}

/// Public-only listing and address lookup for wallet prompts.
pub trait WalletPromptLookup: crate::books::AccountLookup {
    /// Every wallet directory and its non-companion hotkey files.
    fn wallets(
        &self,
    ) -> Result<std::collections::BTreeMap<String, Vec<String>>, crate::books::ResolveError>;
}

/// Filesystem listing through the existing public-key-only account adapter.
pub struct FsWalletPromptLookup(crate::books::FsAccountLookup);
impl FsWalletPromptLookup {
    /// Expand the supplied root through the wallet library.
    pub fn new(root: impl Into<std::path::PathBuf>) -> Result<Self, crate::books::ResolveError> {
        crate::books::FsAccountLookup::new(root).map(Self)
    }
}
impl crate::books::AccountLookup for FsWalletPromptLookup {
    fn coldkey(&self, wallet: &str) -> Result<String, crate::books::ResolveError> {
        self.0.coldkey(wallet)
    }
    fn hotkey(&self, wallet: &str, hotkey: &str) -> Result<String, crate::books::ResolveError> {
        self.0.hotkey(wallet, hotkey)
    }
    fn candidates(&self) -> Vec<crate::books::NameCandidate> {
        self.0.candidates()
    }
}
impl WalletPromptLookup for FsWalletPromptLookup {
    fn wallets(
        &self,
    ) -> Result<std::collections::BTreeMap<String, Vec<String>>, crate::books::ResolveError> {
        use crate::books::ResolveError;
        let mut wallets = std::collections::BTreeMap::new();
        let root = self.0.root();
        if !root.is_dir() {
            return Ok(wallets);
        }
        let entries = std::fs::read_dir(root)
            .map_err(|_| ResolveError::new("cannot list wallet directory"))?;
        for entry in entries {
            let entry = entry.map_err(|_| ResolveError::new("cannot list wallet directory"))?;
            if !entry.path().is_dir() {
                continue;
            }
            let name = entry
                .file_name()
                .into_string()
                .map_err(|_| ResolveError::new("wallet directory name is not UTF-8"))?;
            let directory = entry.path().join("hotkeys");
            let mut hotkeys = Vec::new();
            if directory.is_dir() {
                for hotkey in std::fs::read_dir(directory)
                    .map_err(|_| ResolveError::new("cannot list hotkey directory"))?
                {
                    let hotkey =
                        hotkey.map_err(|_| ResolveError::new("cannot list hotkey directory"))?;
                    if !hotkey.path().is_file() {
                        continue;
                    }
                    let name = hotkey
                        .file_name()
                        .into_string()
                        .map_err(|_| ResolveError::new("hotkey file name is not UTF-8"))?;
                    if !name.ends_with(".lock") && !name.ends_with("pub.txt") {
                        hotkeys.push(name);
                    }
                }
            }
            hotkeys.sort();
            wallets.insert(name, hotkeys);
        }
        Ok(wallets)
    }
}

/// Python's unknown-name diagnostic, listing small sets and suggesting large sets.
pub fn unknown_name_error(kind: &str, raw: &str, known: &[String]) -> String {
    let quoted = rtao::primitives::pyfmt::string_repr(raw);
    if known.len() <= 8 {
        let mut known = known.to_vec();
        known.sort();
        let available = if known.is_empty() {
            "none".into()
        } else {
            known.join(", ")
        };
        return format!("no {kind} named {quoted} (available: {available})");
    }
    let mut matches = known
        .iter()
        .filter_map(|name| {
            let score = crate::books::sequence_ratio(name, raw);
            (score >= 0.5).then_some((score, name))
        })
        .collect::<Vec<_>>();
    matches.sort_by(|(left_score, left), (right_score, right)| {
        right_score
            .total_cmp(left_score)
            .then_with(|| right.cmp(left))
    });
    matches.truncate(3);
    if matches.is_empty() {
        format!(
            "no {kind} named {quoted} — `rtao wallet list` shows all {}",
            known.len()
        )
    } else {
        format!(
            "no {kind} named {quoted} — did you mean {}?",
            matches
                .iter()
                .map(|(_, name)| name.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        )
    }
}

fn multisig_no_local_keys_error(books: &crate::books::Books, name: &str) -> String {
    let members = books
        .multisig(name)
        .and_then(|entry| entry.get("signatories"))
        .and_then(Value::as_array)
        .map(|members| {
            members
                .iter()
                .map(|member| {
                    member
                        .as_str()
                        .map_or_else(|| member.to_string(), String::from)
                })
                .collect::<Vec<_>>()
                .join(", ")
        })
        .unwrap_or_default();
    let hint = if members.is_empty() {
        String::new()
    } else {
        format!(" — use a member wallet instead ({members})")
    };
    format!(
        "{} is a saved multisig: it has no key files of its own, and this command needs a local key{hint}",
        rtao::primitives::pyfmt::string_repr(name)
    )
}
fn validation(message: impl Into<String>) -> PromptValidation {
    PromptValidation::Message(message.into())
}

/// Parse an existing or new local wallet name, updating explicit-selection state.
pub fn parse_wallet(
    selection: &mut WalletSelection,
    books: &crate::books::Books,
    lookup: &dyn WalletPromptLookup,
    raw: &str,
    require_coldkey: bool,
    must_exist: bool,
    allow_multisig: bool,
) -> Result<String, PromptValidation> {
    if !must_exist {
        selection.wallet_name = raw.into();
        selection.wallet_given = true;
        return Ok(raw.into());
    }
    let known = lookup
        .wallets()
        .map_err(|error| validation(error.to_string()))?;
    if !known.contains_key(raw) {
        if books.multisig(raw).is_some() {
            if !allow_multisig {
                return Err(validation(multisig_no_local_keys_error(books, raw)));
            }
            selection.wallet_name = raw.into();
            selection.wallet_given = true;
            return Ok(raw.into());
        }
        let suggestions = known
            .keys()
            .cloned()
            .chain(
                books
                    .multisigs
                    .iter()
                    .filter_map(|entry| entry.get("name").and_then(Value::as_str))
                    .filter(|name| !name.is_empty())
                    .map(String::from),
            )
            .collect::<std::collections::BTreeSet<_>>()
            .into_iter()
            .collect::<Vec<_>>();
        return Err(validation(unknown_name_error("wallet", raw, &suggestions)));
    }
    let address = match lookup.coldkey(raw) {
        Ok(address) => Some(address),
        Err(error) if require_coldkey => {
            return Err(validation(format!(
                "wallet {} has no usable coldkey: {error}",
                rtao::primitives::pyfmt::string_repr(raw)
            )));
        }
        Err(_) => None,
    };
    selection.wallet_name = raw.into();
    selection.wallet_given = true;
    if let Some(address) = address {
        selection.selected_accounts.push((raw.into(), address));
    }
    Ok(raw.into())
}

/// Parse a signing hotkey name; targets can instead use parse_target_hotkey.
pub fn parse_hotkey(
    selection: &mut WalletSelection,
    lookup: &dyn WalletPromptLookup,
    raw: &str,
    must_exist: bool,
) -> Result<String, PromptValidation> {
    if !must_exist {
        selection.hotkey_name = raw.into();
        selection.hotkey_given = true;
        return Ok(raw.into());
    }
    let wallets = lookup
        .wallets()
        .map_err(|error| validation(error.to_string()))?;
    let known = wallets
        .get(&selection.wallet_name)
        .cloned()
        .unwrap_or_default();
    if !known.iter().any(|name| name == raw) {
        return Err(validation(unknown_name_error(
            &format!(
                "hotkey in wallet {}",
                rtao::primitives::pyfmt::string_repr(&selection.wallet_name)
            ),
            raw,
            &known,
        )));
    }
    let address = lookup
        .hotkey(&selection.wallet_name, raw)
        .map_err(|error| {
            validation(format!(
                "cannot open hotkey {} in wallet {}: {error}",
                rtao::primitives::pyfmt::string_repr(raw),
                rtao::primitives::pyfmt::string_repr(&selection.wallet_name)
            ))
        })?;
    selection.hotkey_name = raw.into();
    selection.hotkey_given = true;
    selection
        .selected_accounts
        .push((format!("{}/{raw}", selection.wallet_name), address));
    Ok(raw.into())
}

fn booked_address(books: &crate::books::Books, name: &str) -> Option<String> {
    books
        .addresses
        .iter()
        .find(|entry| entry.get("name").and_then(Value::as_str) == Some(name))
        .and_then(|entry| entry.get("address"))
        .and_then(Value::as_str)
        .filter(|address| !address.is_empty())
        .map(String::from)
}

/// Resolve a hotkey target without demanding a locally owned signing key.
pub fn parse_target_hotkey(
    selection: &mut WalletSelection,
    books: &crate::books::Books,
    lookup: &dyn WalletPromptLookup,
    raw: &str,
) -> Result<String, PromptValidation> {
    let wallets = lookup
        .wallets()
        .map_err(|error| validation(error.to_string()))?;
    let known = wallets
        .get(&selection.wallet_name)
        .cloned()
        .unwrap_or_default();
    if known.iter().any(|name| name == raw) {
        return parse_hotkey(selection, lookup, raw, true);
    }
    if crate::books::is_bittensor_address(raw) {
        return Ok(raw.into());
    }
    if let Some(address) = booked_address(books, raw) {
        selection
            .selected_accounts
            .push((raw.into(), address.clone()));
        return Ok(address);
    }
    if let Some((wallet, hotkey)) = raw.rsplit_once('/') {
        let wallet = if wallet.is_empty() {
            &selection.wallet_name
        } else {
            wallet
        };
        let address = lookup.hotkey(wallet, hotkey).map_err(|error| {
            validation(format!(
                "cannot open hotkey {}: {error}",
                rtao::primitives::pyfmt::string_repr(raw)
            ))
        })?;
        selection
            .selected_accounts
            .push((raw.into(), address.clone()));
        return Ok(address);
    }
    Err(validation(format!(
        "{} — or paste an ss58 address / address-book name",
        unknown_name_error(
            &format!(
                "hotkey in wallet {}",
                rtao::primitives::pyfmt::string_repr(&selection.wallet_name)
            ),
            raw,
            &known
        )
    )))
}

/// Resolve a coldkey target, retaining local-wallet selections as context state.
pub fn parse_target_wallet(
    selection: &mut WalletSelection,
    books: &crate::books::Books,
    lookup: &dyn WalletPromptLookup,
    raw: &str,
    require_coldkey: bool,
    allow_multisig: bool,
) -> Result<String, PromptValidation> {
    if crate::books::is_bittensor_address(raw) {
        return Ok(raw.into());
    }
    if let Some(address) = booked_address(books, raw) {
        selection
            .selected_accounts
            .push((raw.into(), address.clone()));
        return Ok(address);
    }
    parse_wallet(
        selection,
        books,
        lookup,
        raw,
        require_coldkey,
        true,
        allow_multisig,
    )
    .map_err(|error| match error {
        PromptValidation::Message(message) => validation(format!(
            "{message} — or paste an ss58 address / address-book name"
        )),
        error => error,
    })
}

/// Context-specific wallet confirmation options used by wallet command families.
#[derive(Clone, Debug)]
pub struct WalletConfirmation {
    /// Wallet-selection help text.
    pub help: String,
    /// Demand usable public coldkey material for an existing local wallet.
    pub require_coldkey: bool,
    /// Whether the wallet must already exist.
    pub must_exist: bool,
    /// Also confirm the hotkey when this help text is present.
    pub hotkey_help: Option<String>,
    /// Whether a local hotkey must already exist.
    pub hotkey_must_exist: bool,
    /// The final selection is a target that may be an external address.
    pub accept_address: bool,
    /// Saved multisig names are permitted by the command.
    pub allow_multisig: bool,
}
impl Default for WalletConfirmation {
    fn default() -> Self {
        Self {
            help: "Wallet this command targets.".into(),
            require_coldkey: true,
            must_exist: true,
            hotkey_help: None,
            hotkey_must_exist: true,
            accept_address: false,
            allow_multisig: true,
        }
    }
}

/// Confirm defaulted wallet/hotkey selections and return a pasted target address.
pub fn confirm_wallet(
    context: &PromptContext,
    terminal: &mut dyn Terminal,
    selection: &mut WalletSelection,
    books: &crate::books::Books,
    lookup: &dyn WalletPromptLookup,
    options: &WalletConfirmation,
    history: &mut PromptHistory,
) -> Result<Option<String>, PromptError> {
    let skip = context.assume_yes || context.external_signer;
    let wallets = lookup
        .wallets()
        .map_err(|error| PromptError::InvalidValue(error.to_string()))?;
    if !options.allow_multisig
        && books.multisig(&selection.wallet_name).is_some()
        && !wallets.contains_key(&selection.wallet_name)
        && (selection.wallet_given || skip || !context.interactive(terminal))
    {
        return Err(PromptError::InvalidValue(multisig_no_local_keys_error(
            books,
            &selection.wallet_name,
        )));
    }
    let mut specs = Vec::new();
    if !selection.wallet_given && !skip {
        let mut spec = PromptSpec::new("wallet", "--wallet");
        spec.help = Some(options.help.clone());
        spec.default = Some(selection.wallet_name.clone());
        spec.flow = if !options.must_exist {
            PromptFlow::WalletName
        } else if options.accept_address && options.hotkey_help.is_none() {
            PromptFlow::TargetWallet {
                require_coldkey: options.require_coldkey,
                allow_multisig: options.allow_multisig,
            }
        } else {
            PromptFlow::Wallet {
                require_coldkey: options.require_coldkey,
                allow_multisig: options.allow_multisig,
            }
        };
        specs.push(spec);
    }
    if let Some(help) = &options.hotkey_help
        && !selection.hotkey_given
        && !skip
    {
        let mut spec = PromptSpec::new("wallet_hotkey", "--wallet-hotkey");
        spec.help = Some(help.clone());
        if options.accept_address {
            let mut known = wallets
                .get(&selection.wallet_name)
                .cloned()
                .unwrap_or_default();
            known.sort();
            spec.default = known
                .iter()
                .any(|name| name == &selection.hotkey_name)
                .then(|| selection.hotkey_name.clone());
            spec.help = Some(if known.is_empty() {
                format!(
                    "{help} Wallet {} has no local hotkeys — paste the hotkey's ss58 address or an address-book name.",
                    rtao::primitives::pyfmt::string_repr(&selection.wallet_name)
                )
            } else {
                format!(
                    "{help} A local hotkey ({}{}), a pasted ss58 address, or an address-book name.",
                    known
                        .iter()
                        .take(8)
                        .map(String::as_str)
                        .collect::<Vec<_>>()
                        .join(", "),
                    if known.len() > 8 { ", …" } else { "" }
                )
            });
            spec.flow = PromptFlow::TargetHotkey;
        } else {
            spec.default = Some(selection.hotkey_name.clone());
            spec.flow = if options.hotkey_must_exist {
                PromptFlow::Hotkey
            } else {
                PromptFlow::HotkeyName
            };
        }
        specs.push(spec);
    }
    struct Resolver<'a> {
        selection: &'a mut WalletSelection,
        books: &'a crate::books::Books,
        lookup: &'a dyn WalletPromptLookup,
    }
    impl PromptResolver for Resolver<'_> {
        fn parse(&mut self, spec: &PromptSpec, raw: &str) -> Result<Value, PromptValidation> {
            let value = match spec.flow {
                PromptFlow::Wallet {
                    require_coldkey,
                    allow_multisig,
                } => parse_wallet(
                    self.selection,
                    self.books,
                    self.lookup,
                    raw,
                    require_coldkey,
                    true,
                    allow_multisig,
                ),
                PromptFlow::TargetWallet {
                    require_coldkey,
                    allow_multisig,
                } => parse_target_wallet(
                    self.selection,
                    self.books,
                    self.lookup,
                    raw,
                    require_coldkey,
                    allow_multisig,
                ),
                PromptFlow::WalletName => parse_wallet(
                    self.selection,
                    self.books,
                    self.lookup,
                    raw,
                    false,
                    false,
                    false,
                ),
                PromptFlow::Hotkey => parse_hotkey(self.selection, self.lookup, raw, true),
                PromptFlow::HotkeyName => parse_hotkey(self.selection, self.lookup, raw, false),
                PromptFlow::TargetHotkey => {
                    parse_target_hotkey(self.selection, self.books, self.lookup, raw)
                }
                _ => Err(validation("unsupported wallet confirmation flow")),
            }?;
            Ok(Value::String(value))
        }
    }
    let mut answers = Map::new();
    fill_missing(
        context,
        terminal,
        &specs,
        &mut answers,
        &mut Resolver {
            selection,
            books,
            lookup,
        },
        history,
    )?;
    selection.wallet_given = true;
    if options.hotkey_help.is_some() {
        selection.hotkey_given = true
    }
    if options.accept_address {
        for field in ["wallet_hotkey", "wallet"] {
            if let Some(address) = answers.get(field).and_then(Value::as_str)
                && crate::books::is_bittensor_address(address)
            {
                return Ok(Some(address.into()));
            }
        }
    }
    Ok(None)
}

/// Invocation-local public answers and SDK hint, never hidden-input values.
#[derive(Clone, Debug, Default)]
pub struct PromptHistory {
    entered_tokens: Vec<String>,
    sdk_hint: Option<String>,
}
impl PromptHistory {
    /// Record only ordinary public prompt answers.
    pub fn record_answers(&mut self, tokens: Vec<String>) {
        self.entered_tokens.extend(tokens);
    }
    /// Record the SDK expression for a successfully completed invocation.
    pub fn record_sdk_hint(&mut self, expression: String) {
        self.sdk_hint = Some(expression);
    }
    /// Render the actual submission command, dropping --json and --dry-run.
    pub fn replay_command(&self, argv: &[String]) -> String {
        command_line(argv, &self.entered_tokens, true)
    }
    /// Return public hints once after success; dry runs already show the replay.
    pub fn flush_hint(&mut self, argv: &[String], exit_code: u8) -> Option<String> {
        if exit_code != 0 || self.entered_tokens.is_empty() {
            return None;
        }
        if argv.iter().any(|token| token == "--dry-run") {
            self.entered_tokens.clear();
            return None;
        }
        let command = command_line(argv, &self.entered_tokens, false);
        self.entered_tokens.clear();
        let mut hint = format!("skip the prompts next time: {command}");
        if let Some(sdk) = &self.sdk_hint {
            hint.push_str(&format!(
                "\nthe same call from the sdk: {sdk}  (sub = bt.Subtensor())"
            ));
        }
        Some(hint)
    }
}
fn command_line(argv: &[String], answers: &[String], drop_dry: bool) -> String {
    let mut tokens = Vec::new();
    if let Some(program) = argv.first() {
        tokens.push(
            std::path::Path::new(program)
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .into_owned(),
        );
    }
    let mut arguments = argv.iter().skip(1).chain(answers);
    while let Some(token) = arguments.next() {
        let (flag, inline_value) = token
            .split_once('=')
            .map_or((token.as_str(), false), |(flag, _)| (flag, true));
        if matches!(
            flag,
            "--mnemonic"
                | "--seed"
                | "--private-key"
                | "--password"
                | "--new-password"
                | "--json-password"
        ) {
            if !inline_value {
                let _ = arguments.next();
            }
            continue;
        }
        if !drop_dry || !matches!(token.as_str(), "--dry-run" | "--json") {
            tokens.push(token.clone());
        }
    }
    tokens
        .iter()
        .map(|token| shell_quote(token))
        .collect::<Vec<_>>()
        .join(" ")
}
/// Quote a public token for a POSIX replay command, without executing it.
pub fn shell_quote(token: &str) -> String {
    if !token.is_empty()
        && token
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"_@%+=:,./-".contains(&byte))
    {
        return token.to_owned();
    }
    format!("'{}'", token.replace('\'', "'\"'\"'"))
}

/// Read hidden entry, re-asking empty/mismatched passwords only on a TTY.
///
/// Passwords are not trimmed: an all-space password is valid in Python.
pub fn prompt_password(
    context: &PromptContext,
    terminal: &mut dyn Terminal,
    message: &str,
    confirm: bool,
) -> Result<SecretString, PromptError> {
    if !context.password_interactive(terminal) {
        return Err(PromptError::Unavailable);
    }
    loop {
        let password = terminal.read_secret(message)?;
        if password.expose_secret().is_empty() {
            terminal.message("password cannot be empty")?;
            continue;
        }
        if confirm {
            let again = terminal.read_secret("Retype password: ")?;
            if again.expose_secret() != password.expose_secret() {
                terminal.message("passwords do not match")?;
                continue;
            }
        }
        return Ok(password);
    }
}

/// Adapter for wallet library callbacks; the library's default remains Never.
pub struct TerminalPasswordPrompt<T> {
    terminal: Mutex<T>,
    context: PromptContext,
    confirm: bool,
}
impl<T: Terminal> TerminalPasswordPrompt<T> {
    /// Supply the terminal and CLI flags explicitly.
    pub fn new(terminal: T, context: PromptContext, confirm: bool) -> Self {
        Self {
            terminal: Mutex::new(terminal),
            context,
            confirm,
        }
    }
}
impl<T: Terminal> PasswordPrompt for TerminalPasswordPrompt<T> {
    fn is_terminal(&self) -> bool {
        self.terminal
            .lock()
            .is_ok_and(|terminal| self.context.password_interactive(&*terminal))
    }
    fn prompt(&self, message: &str) -> Result<SecretString, WalletError> {
        let mut terminal = self
            .terminal
            .lock()
            .map_err(|_| WalletError::PromptUnavailable)?;
        prompt_password(&self.context, &mut *terminal, message, self.confirm).map_err(|error| {
            match error {
                PromptError::Unavailable => WalletError::PromptUnavailable,
                PromptError::Aborted => WalletError::NativePassword("password entry cancelled"),
                _ => WalletError::NativePassword("password entry failed"),
            }
        })
    }
}

/// Interactive coldkey unlock, with two retries only for wrong prompted input.
///
/// Explicit/env/file/Keychain/dialog values bypass this helper and attempt once.
/// The returned terminal and wallet errors contain no password value.
pub async fn unlock_prompted<T, F, Fut>(
    context: &PromptContext,
    terminal: &mut dyn Terminal,
    message: &str,
    mut unlock: F,
) -> Result<T, WalletError>
where
    F: FnMut(SecretString) -> Fut,
    Fut: Future<Output = Result<T, WalletError>>,
{
    for attempts_left in [2, 1, 0] {
        let password = prompt_password(context, terminal, message, false)
            .map_err(|_| WalletError::PromptUnavailable)?;
        match unlock(password).await {
            Ok(value) => return Ok(value),
            Err(WalletError::WrongPassword { .. }) if attempts_left > 0 => {
                terminal
                    .message("wrong password")
                    .map_err(|_| WalletError::PromptUnavailable)?;
            }
            Err(error) => return Err(error),
        }
    }
    Err(WalletError::PasswordRequired)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;
    use std::collections::VecDeque;
    #[derive(Default)]
    struct FakeTerminal {
        tty: bool,
        err_tty: bool,
        lines: VecDeque<Result<String, PromptError>>,
        hidden: VecDeque<Result<SecretString, PromptError>>,
        reads: Vec<String>,
        messages: Vec<String>,
    }
    impl FakeTerminal {
        fn interactive() -> Self {
            Self {
                tty: true,
                err_tty: true,
                ..Default::default()
            }
        }
    }
    impl Terminal for FakeTerminal {
        fn stdin_is_tty(&self) -> bool {
            self.tty
        }
        fn stderr_is_tty(&self) -> bool {
            self.err_tty
        }
        fn read_line(&mut self, label: &str) -> Result<String, PromptError> {
            self.reads.push(label.into());
            self.lines.pop_front().unwrap()
        }
        fn read_secret(&mut self, label: &str) -> Result<SecretString, PromptError> {
            self.reads.push(label.into());
            self.hidden.pop_front().unwrap()
        }
        fn message(&mut self, message: &str) -> Result<(), PromptError> {
            self.messages.push(message.into());
            Ok(())
        }
    }
    struct Resolver;
    impl PromptResolver for Resolver {
        fn parse(&mut self, _: &PromptSpec, raw: &str) -> Result<Value, PromptValidation> {
            raw.parse::<u64>()
                .map(Value::from)
                .map_err(|_| PromptValidation::Message("integer required".into()))
        }
    }
    fn secrets(values: &[&str]) -> VecDeque<Result<SecretString, PromptError>> {
        values
            .iter()
            .map(|value| Ok(SecretString::new((*value).into())))
            .collect()
    }

    #[test]
    fn shared_terminal_escapes_display_text_without_changing_answers() {
        struct Observed(Arc<Mutex<Vec<String>>>);
        const ANSWER: &str = "answer\u{1b}]52;c;payload\u{7}\u{202e}";
        impl Terminal for Observed {
            fn stdin_is_tty(&self) -> bool {
                true
            }
            fn stderr_is_tty(&self) -> bool {
                true
            }
            fn read_line(&mut self, label: &str) -> Result<String, PromptError> {
                self.0
                    .lock()
                    .map_err(|_| PromptError::Io)?
                    .push(label.into());
                Ok(ANSWER.into())
            }
            fn read_secret(&mut self, label: &str) -> Result<SecretString, PromptError> {
                self.0
                    .lock()
                    .map_err(|_| PromptError::Io)?
                    .push(label.into());
                Ok(SecretString::new(ANSWER.into()))
            }
            fn message(&mut self, text: &str) -> Result<(), PromptError> {
                self.0
                    .lock()
                    .map_err(|_| PromptError::Io)?
                    .push(text.into());
                Ok(())
            }
        }
        let seen = Arc::new(Mutex::new(Vec::new()));
        let mut terminal: SharedTerminal = Arc::new(Mutex::new(Box::new(Observed(seen.clone()))));
        let hostile = "Confirm signer\u{1b}[2J\r\n\u{7}\u{202e}\u{2066}: ";
        let expected = "Confirm signer\\x1b[2J\\r\\n\\x07\\u{202e}\\u{2066}: ";
        assert_eq!(terminal.read_line(hostile).unwrap(), ANSWER);
        assert_eq!(
            terminal.read_secret(hostile).unwrap().expose_secret(),
            ANSWER
        );
        terminal.message(hostile).unwrap();
        assert_eq!(*seen.lock().unwrap(), vec![expected, expected, expected]);
    }

    #[test]
    fn every_noninteractive_mode_refuses_reads_and_collects_missing_flags() {
        for (tty, err_tty, context) in [
            (false, true, PromptContext::default()),
            (true, false, PromptContext::default()),
            (
                true,
                true,
                PromptContext {
                    json: true,
                    ..Default::default()
                },
            ),
            (
                true,
                true,
                PromptContext {
                    quiet: true,
                    ..Default::default()
                },
            ),
            (
                true,
                true,
                PromptContext {
                    policy: PromptPolicy::Never,
                    ..Default::default()
                },
            ),
        ] {
            let mut terminal = FakeTerminal {
                tty,
                err_tty,
                ..Default::default()
            };
            let missing = [
                PromptSpec::new("n", "--netuid"),
                PromptSpec::new("a", "--amount"),
            ];
            let error = fill_missing(
                &context,
                &mut terminal,
                &missing,
                &mut Map::new(),
                &mut Resolver,
                &mut PromptHistory::default(),
            )
            .unwrap_err();
            assert_eq!(
                error,
                PromptError::MissingRequired(vec!["--netuid".into(), "--amount".into()])
            );
            assert_eq!(error.exit_code(), 2);
            assert!(terminal.reads.is_empty());
            if !tty || context.policy == PromptPolicy::Never {
                assert_eq!(
                    prompt_password(&context, &mut terminal, "hidden", true).unwrap_err(),
                    PromptError::Unavailable
                );
            }
        }
    }

    #[test]
    fn defaulted_options_are_retained_without_noninteractive_prompting() {
        let mut spec = PromptSpec::new("wallet", "--wallet");
        spec.default = Some("configured".into());
        let mut terminal = FakeTerminal::default();
        let mut answers = Map::from_iter([("wallet".into(), Value::String("configured".into()))]);
        fill_missing(
            &PromptContext::default(),
            &mut terminal,
            &[spec],
            &mut answers,
            &mut Resolver,
            &mut PromptHistory::default(),
        )
        .unwrap();
        assert_eq!(answers["wallet"], "configured");
        assert!(terminal.reads.is_empty());
    }

    #[test]
    fn empty_and_invalid_ordinary_answers_reask_and_default_is_enter_to_accept() {
        let mut terminal = FakeTerminal::interactive();
        terminal.lines = [" \n", "bad", "\u{1c}42\u{1f}"]
            .into_iter()
            .map(|value| Ok(value.into()))
            .collect();
        let mut history = PromptHistory::default();
        let mut answers = Map::new();
        fill_missing(
            &PromptContext::default(),
            &mut terminal,
            &[PromptSpec::new("count", "--count")],
            &mut answers,
            &mut Resolver,
            &mut history,
        )
        .unwrap();
        assert_eq!(answers["count"], 42);
        assert_eq!(
            terminal.messages,
            ["a value is required", "integer required"]
        );
        assert_eq!(terminal.reads.len(), 3);
        assert_eq!(
            history.replay_command(&["/bin/rtao".into(), "tx".into()]),
            "rtao tx --count 42"
        );
        terminal.lines.push_back(Ok("\n".into()));
        let mut spec = PromptSpec::new("count", "--count");
        spec.default = Some("7".into());
        assert_eq!(
            ask_with(&PromptContext::default(), &mut terminal, &spec, |raw| Ok::<
                _,
                PromptValidation,
            >(
                raw.parse::<u64>().unwrap()
            ))
            .unwrap(),
            (7, "7".into())
        );
    }

    #[test]
    fn hidden_empty_mismatch_retry_never_enters_public_history_or_messages() {
        let mut terminal = FakeTerminal::interactive();
        terminal.hidden = secrets(&["", "private-one", "private-other", "  ", "  "]);
        let secret = prompt_password(
            &PromptContext::default(),
            &mut terminal,
            "Create password: ",
            true,
        )
        .unwrap();
        assert_eq!(secret.expose_secret(), "  ");
        assert_eq!(
            terminal.reads,
            [
                "Create password: ",
                "Create password: ",
                "Retype password: ",
                "Create password: ",
                "Retype password: "
            ]
        );
        assert_eq!(
            terminal.messages,
            ["password cannot be empty", "passwords do not match"]
        );
        assert!(!format!("{:?}", terminal.messages).contains("private"));
    }

    #[test]
    fn hidden_keyfile_entry_uses_stdin_only_if_tty_even_in_json_quiet_or_yes_mode() {
        for context in [
            PromptContext {
                json: true,
                ..Default::default()
            },
            PromptContext {
                quiet: true,
                ..Default::default()
            },
            PromptContext {
                assume_yes: true,
                ..Default::default()
            },
        ] {
            let mut terminal = FakeTerminal {
                tty: true,
                err_tty: false,
                hidden: secrets(&["private-hidden"]),
                ..Default::default()
            };
            assert_eq!(
                prompt_password(&context, &mut terminal, "Unlock: ", false)
                    .unwrap()
                    .expose_secret(),
                "private-hidden"
            );
            assert!(terminal.messages.is_empty());
            assert_eq!(terminal.reads, ["Unlock: "]);
        }
    }

    #[test]
    fn abort_has_python_exit_status_and_preserves_no_answer() {
        let mut terminal = FakeTerminal::interactive();
        terminal.lines.push_back(Err(PromptError::Aborted));
        let error = ask_with(
            &PromptContext::default(),
            &mut terminal,
            &PromptSpec::new("n", "--n"),
            |_| Ok::<_, PromptValidation>(()),
        )
        .unwrap_err();
        assert_eq!(error.exit_code(), 130);
    }

    #[tokio::test]
    async fn wrong_prompted_password_gets_exactly_two_retries_other_errors_stop() {
        let mut terminal = FakeTerminal::interactive();
        terminal.hidden = secrets(&["private-1", "private-2", "private-3"]);
        let mut attempts = 0;
        let result = unlock_prompted(&PromptContext::default(), &mut terminal, "Unlock: ", |_| {
            attempts += 1;
            std::future::ready(if attempts == 3 {
                Ok(5)
            } else {
                Err(WalletError::WrongPassword {
                    source: rtao::wallet::PasswordSource::Prompt,
                })
            })
        })
        .await
        .unwrap();
        assert_eq!(result, 5);
        assert_eq!(attempts, 3);
        assert_eq!(terminal.messages, ["wrong password", "wrong password"]);
        let mut terminal = FakeTerminal::interactive();
        terminal.hidden = secrets(&["private"]);
        let error = unlock_prompted(&PromptContext::default(), &mut terminal, "Unlock: ", |_| {
            std::future::ready::<Result<(), _>>(Err(WalletError::PasswordRequired))
        })
        .await
        .unwrap_err();
        assert!(matches!(error, WalletError::PasswordRequired));
        assert_eq!(terminal.reads.len(), 1);
    }

    #[test]
    fn replay_quotes_public_answers_drops_only_dry_run_and_json_and_flushes_once() {
        let argv = ["/usr/bin/rtao", "tx", "transfer", "--dry-run", "--json"].map(String::from);
        let mut history = PromptHistory::default();
        history.record_answers(vec!["--dest".into(), "a'b $(false)".into()]);
        assert_eq!(
            history.replay_command(&argv),
            "rtao tx transfer --dest 'a'\"'\"'b $(false)'"
        );
        assert!(history.flush_hint(&argv, 1).is_none());
        assert!(history.flush_hint(&argv, 0).is_none());
        assert_eq!(history.replay_command(&argv), "rtao tx transfer");
        history.record_answers(vec!["--netuid".into(), "2".into()]);
        assert!(
            history
                .flush_hint(&["rtao".into()], 0)
                .unwrap()
                .contains("--netuid 2")
        );
        assert!(history.flush_hint(&["rtao".into()], 0).is_none());
    }

    #[test]
    fn replay_and_success_hints_omit_secret_flags_in_both_argument_forms() {
        let argv = [
            "rtao",
            "wallet",
            "regen-coldkey",
            "--mnemonic",
            "private-phrase-sentinel",
            "--seed=private-seed-sentinel",
            "--private-key",
            "private-key-sentinel",
            "--json-password",
            "private-json-backup-sentinel",
            "--json-password=private-inline-json-sentinel",
            "--wallet-password-file",
            "public-password-file",
            "--wallet",
            "visible",
        ]
        .map(String::from);
        let mut history = PromptHistory::default();
        history.record_answers(vec!["--netuid".into(), "2".into()]);
        let replay = history.replay_command(&argv);
        assert_eq!(
            replay,
            "rtao wallet regen-coldkey --wallet-password-file public-password-file --wallet visible --netuid 2"
        );
        assert!(!history.flush_hint(&argv, 0).unwrap().contains("private"));
    }

    #[test]
    fn signer_order_and_unlock_multisig_refusal_are_encoded_in_the_resolver_spec() {
        let specs = signer_specs("wallet", "hotkey", "hotkey", false, false);
        assert_eq!(
            specs
                .iter()
                .map(|spec| spec.field.as_str())
                .collect::<Vec<_>>(),
            ["wallet", "wallet_hotkey"]
        );
        assert_eq!(
            specs[0].flow,
            PromptFlow::Wallet {
                require_coldkey: false,
                allow_multisig: true
            }
        );
        let spec = signing_wallet_spec(
            &PromptContext::default(),
            "wallet",
            false,
            "unlock",
            true,
            &[],
        )
        .unwrap();
        assert_eq!(
            spec.flow,
            PromptFlow::Wallet {
                require_coldkey: false,
                allow_multisig: false
            }
        );
        for spelling in ["--wallet=x", "-w", "--yes", "--signer=ledger", "--ledger"] {
            assert!(
                signing_wallet_spec(
                    &PromptContext::default(),
                    "w",
                    false,
                    "tx",
                    true,
                    &[spelling.into()]
                )
                .is_none()
            );
        }
    }

    #[test]
    fn custom_flow_populates_multiple_fields_and_later_spec_does_not_ask_again() {
        struct Picker {
            decorated: usize,
            parsed: usize,
        }
        impl PromptResolver for Picker {
            fn parse(&mut self, _: &PromptSpec, raw: &str) -> Result<Value, PromptValidation> {
                self.parsed += 1;
                Ok(Value::String(raw.into()))
            }
            fn custom(
                &mut self,
                spec: &PromptSpec,
                _: &mut dyn Terminal,
                answers: &mut Map<String, Value>,
            ) -> Result<Vec<String>, PromptError> {
                assert_eq!(
                    spec.flow,
                    PromptFlow::StakeSource {
                        netuid_field: Some("netuid".into())
                    }
                );
                answers.insert("hotkey_ss58".into(), Value::String("public-hotkey".into()));
                answers.insert("netuid".into(), Value::from(8));
                Ok(vec![
                    "--hotkey".into(),
                    "public-hotkey".into(),
                    "--netuid".into(),
                    "8".into(),
                ])
            }
            fn decorate(&mut self, spec: &mut PromptSpec) -> Result<(), PromptError> {
                self.decorated += 1;
                spec.help = Some("Free balance: 5 TAO".into());
                Ok(())
            }
        }
        let mut source = PromptSpec::new("hotkey_ss58", "--hotkey");
        source.flow = PromptFlow::StakeSource {
            netuid_field: Some("netuid".into()),
        };
        let mut amount = PromptSpec::new("amount_tao", "--amount");
        amount.free_balance = true;
        let mut terminal = FakeTerminal::interactive();
        terminal.lines.push_back(Ok("4".into()));
        let mut resolver = Picker {
            decorated: 0,
            parsed: 0,
        };
        let mut answers = Map::new();
        let mut history = PromptHistory::default();
        fill_missing(
            &PromptContext::default(),
            &mut terminal,
            &[source, PromptSpec::new("netuid", "--netuid"), amount],
            &mut answers,
            &mut resolver,
            &mut history,
        )
        .unwrap();
        assert_eq!(resolver.decorated, 1);
        assert_eq!(resolver.parsed, 1);
        assert_eq!(terminal.reads.len(), 1);
        assert_eq!(answers["netuid"], 8);
        assert_eq!(
            history.replay_command(&["rtao".into()]),
            "rtao --hotkey public-hotkey --netuid 8 --amount 4"
        );
        assert_eq!(terminal.messages, ["--amount: Free balance: 5 TAO"]);
    }

    #[test]
    fn shared_terminal_callback_uses_the_same_terminal_and_hidden_authority() {
        let mut fixture = FakeTerminal {
            tty: true,
            err_tty: false,
            hidden: secrets(&["private-callback"]),
            ..Default::default()
        };
        fixture.lines.push_back(Ok("public-line".into()));
        let shared: SharedTerminal = Arc::new(Mutex::new(Box::new(fixture)));
        let prompt = TerminalPasswordPrompt::new(
            shared.clone(),
            PromptContext {
                json: true,
                ..Default::default()
            },
            false,
        );
        assert!(prompt.is_terminal());
        assert_eq!(
            prompt.prompt("Unlock: ").unwrap().expose_secret(),
            "private-callback"
        );
        assert_eq!(shared.clone().read_line("ordinary").unwrap(), "public-line");
    }

    const ALICE: &str = "5GrwvaEF5zXb26Fz9rcQpDWS57CtERHpNehXCPcNoHGKutQY";
    const BOB: &str = "5FHneW46xGXgs5mUiveU4sbTyGBzmstUspZC92UhjJM694ty";
    struct WalletLookup;
    impl crate::books::AccountLookup for WalletLookup {
        fn coldkey(&self, wallet: &str) -> Result<String, crate::books::ResolveError> {
            if wallet == "owner" {
                Ok(ALICE.into())
            } else {
                Err(crate::books::ResolveError::new("no public coldkey"))
            }
        }
        fn hotkey(&self, wallet: &str, hotkey: &str) -> Result<String, crate::books::ResolveError> {
            if matches!(wallet, "owner" | "hot-only") && hotkey == "miner" {
                Ok(BOB.into())
            } else {
                Err(crate::books::ResolveError::new("no public hotkey"))
            }
        }
    }
    impl WalletPromptLookup for WalletLookup {
        fn wallets(
            &self,
        ) -> Result<std::collections::BTreeMap<String, Vec<String>>, crate::books::ResolveError>
        {
            Ok(std::collections::BTreeMap::from([
                ("owner".into(), vec!["miner".into()]),
                ("hot-only".into(), vec!["miner".into()]),
            ]))
        }
    }
    fn prompt_books() -> crate::books::Books {
        crate::books::Books::from_values(
            serde_json::json!([
                {"name":"friend","address":ALICE}, {"name":"miner","address":ALICE},
            ]),
            Value::Null,
            serde_json::json!([{"name":"safe","signatories":["owner","hot-only"]}]),
        )
    }
    fn selection() -> WalletSelection {
        WalletSelection {
            wallet_name: "owner".into(),
            hotkey_name: "miner".into(),
            ..Default::default()
        }
    }

    #[test]
    fn local_wallet_validation_preserves_coldkey_requirement_and_saved_multisig_rules() {
        let books = prompt_books();
        let mut state = selection();
        assert!(
            parse_wallet(
                &mut state,
                &books,
                &WalletLookup,
                "hot-only",
                true,
                true,
                true
            )
            .is_err()
        );
        assert_eq!(state.wallet_name, "owner");
        assert_eq!(
            parse_wallet(
                &mut state,
                &books,
                &WalletLookup,
                "hot-only",
                false,
                true,
                true
            )
            .unwrap(),
            "hot-only"
        );
        assert!(state.wallet_given);
        assert!(state.selected_accounts.is_empty());
        assert_eq!(
            parse_wallet(&mut state, &books, &WalletLookup, "safe", true, true, true).unwrap(),
            "safe"
        );
        let error = parse_wallet(
            &mut state,
            &books,
            &WalletLookup,
            "safe",
            false,
            true,
            false,
        )
        .unwrap_err();
        assert!(
            matches!(error, PromptValidation::Message(message) if message.contains("no key files") && message.contains("owner, hot-only"))
        );
        let error = parse_wallet(
            &mut state,
            &books,
            &WalletLookup,
            "missing",
            true,
            true,
            true,
        )
        .unwrap_err();
        assert_eq!(
            error,
            PromptValidation::Message(
                "no wallet named 'missing' (available: hot-only, owner, safe)".into()
            )
        );
        assert_eq!(
            parse_wallet(&mut state, &books, &WalletLookup, "new", true, false, false).unwrap(),
            "new"
        );
        assert_eq!(state.wallet_name, "new");
    }

    #[test]
    fn target_resolution_prefers_local_hotkeys_then_pasted_addresses_books_and_references() {
        let books = prompt_books();
        let mut state = selection();
        assert_eq!(
            parse_target_hotkey(&mut state, &books, &WalletLookup, "miner").unwrap(),
            "miner"
        );
        assert_eq!(
            state.selected_accounts.last().unwrap(),
            &("owner/miner".into(), BOB.into())
        );
        assert_eq!(
            parse_target_hotkey(&mut state, &books, &WalletLookup, ALICE).unwrap(),
            ALICE
        );
        assert_eq!(
            parse_target_hotkey(&mut state, &books, &WalletLookup, "friend").unwrap(),
            ALICE
        );
        assert_eq!(
            parse_target_hotkey(&mut state, &books, &WalletLookup, "/miner").unwrap(),
            BOB
        );
        assert_eq!(
            parse_target_hotkey(&mut state, &books, &WalletLookup, "hot-only/miner").unwrap(),
            BOB
        );
        assert_eq!(
            parse_target_wallet(&mut state, &books, &WalletLookup, "friend", true, true).unwrap(),
            ALICE
        );
        assert_eq!(state.wallet_name, "owner");
        assert!(
            matches!(parse_target_wallet(&mut state, &books, &WalletLookup, "missing", true, true).unwrap_err(), PromptValidation::Message(message) if message.ends_with("— or paste an ss58 address / address-book name"))
        );
    }

    #[test]
    fn confirmation_reasks_unusable_multisig_default_and_silently_marks_noninteractive_defaults() {
        let books = prompt_books();
        let options = WalletConfirmation {
            allow_multisig: false,
            ..Default::default()
        };
        let mut state = selection();
        state.wallet_name = "safe".into();
        let mut terminal = FakeTerminal::interactive();
        terminal.lines = ["", "owner"]
            .map(|line| Ok(line.into()))
            .into_iter()
            .collect();
        let mut history = PromptHistory::default();
        assert!(
            confirm_wallet(
                &PromptContext::default(),
                &mut terminal,
                &mut state,
                &books,
                &WalletLookup,
                &options,
                &mut history
            )
            .unwrap()
            .is_none()
        );
        assert_eq!(state.wallet_name, "owner");
        assert!(state.wallet_given);
        assert_eq!(terminal.reads.len(), 2);
        assert!(
            terminal
                .messages
                .iter()
                .any(|message| message.contains("no key files"))
        );
        assert_eq!(
            history.replay_command(&["rtao".into()]),
            "rtao --wallet owner"
        );
        let mut state = selection();
        let mut terminal = FakeTerminal::default();
        let options = WalletConfirmation {
            hotkey_help: Some("Signing hotkey".into()),
            ..Default::default()
        };
        confirm_wallet(
            &PromptContext::default(),
            &mut terminal,
            &mut state,
            &books,
            &WalletLookup,
            &options,
            &mut PromptHistory::default(),
        )
        .unwrap();
        assert!(state.wallet_given && state.hotkey_given);
        assert!(terminal.reads.is_empty());
        state.wallet_name = "safe".into();
        assert_eq!(
            confirm_wallet(
                &PromptContext::default(),
                &mut terminal,
                &mut state,
                &books,
                &WalletLookup,
                &WalletConfirmation {
                    allow_multisig: false,
                    ..Default::default()
                },
                &mut PromptHistory::default()
            )
            .unwrap_err()
            .exit_code(),
            2
        );
    }

    #[test]
    fn confirmation_accepts_pasted_target_and_has_no_hotkey_default_without_local_key() {
        let books = prompt_books();
        let mut state = selection();
        let mut terminal = FakeTerminal::interactive();
        terminal.lines.push_back(Ok(BOB.into()));
        let address = confirm_wallet(
            &PromptContext::default(),
            &mut terminal,
            &mut state,
            &books,
            &WalletLookup,
            &WalletConfirmation {
                accept_address: true,
                ..Default::default()
            },
            &mut PromptHistory::default(),
        )
        .unwrap()
        .unwrap();
        assert_eq!(address, BOB);
        assert_eq!(state.wallet_name, "owner");
        let mut terminal = FakeTerminal::default();
        let mut state = selection();
        state.hotkey_name = "absent".into();
        let error = confirm_wallet(
            &PromptContext::default(),
            &mut terminal,
            &mut state,
            &books,
            &WalletLookup,
            &WalletConfirmation {
                accept_address: true,
                hotkey_help: Some("Target".into()),
                ..Default::default()
            },
            &mut PromptHistory::default(),
        )
        .unwrap_err();
        assert_eq!(
            error,
            PromptError::MissingRequired(vec!["--wallet-hotkey".into()])
        );
    }

    #[test]
    fn unknown_name_suggestions_use_python_sequence_matcher_cutoff_and_tie_order() {
        let known = [
            "wallet00", "wallet01", "wallet02", "wallet03", "wallet04", "wallet05", "wallet06",
            "wallet07", "wallet08",
        ]
        .map(String::from);
        assert_eq!(
            unknown_name_error("wallet", "wallet0", &known),
            "no wallet named 'wallet0' — did you mean wallet08, wallet07, wallet06?"
        );
        assert_eq!(
            unknown_name_error("wallet", "unrelatedXYZ", &known),
            "no wallet named 'unrelatedXYZ' — `rtao wallet list` shows all 9"
        );
    }
}
