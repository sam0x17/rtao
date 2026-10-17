// Derived from RaoFoundation/subtensor sdk/python/bittensor/cli/context.py @ 850f2755bb05a3851f948489f0ab27b0645cebce (Apache-2.0); modified by rtao.
// Derived from RaoFoundation/subtensor sdk/python/bittensor/cli/call.py @ 850f2755bb05a3851f948489f0ab27b0645cebce (Apache-2.0); modified by rtao.
// Derived from RaoFoundation/subtensor sdk/python/bittensor/extension/picker.py @ 850f2755bb05a3851f948489f0ab27b0645cebce (Apache-2.0); modified by rtao.
use std::sync::Arc;

use rtao::{primitives::AccountId, signer::BoxFuture};

use super::{BackendRequest, BackendSession, SigningBackend};
use crate::{context::AppContext, error::CliError};

pub(super) struct NativeBackend {
    #[cfg(any(feature = "ledger", feature = "vault", feature = "extension"))]
    context: AppContext,
}
impl NativeBackend {
    pub(super) fn new(context: &AppContext) -> Self {
        #[cfg(not(any(feature = "ledger", feature = "vault", feature = "extension")))]
        let _ = context;
        Self {
            #[cfg(any(feature = "ledger", feature = "vault", feature = "extension"))]
            context: context.clone(),
        }
    }
    fn backend(request: &BackendRequest) -> &str {
        request.globals.signer_backend.as_deref().unwrap_or("")
    }
}
impl SigningBackend for NativeBackend {
    fn validate(&self, request: &BackendRequest) -> Result<(), CliError> {
        let backend = Self::backend(request);
        let available = matches!(backend, "ledger" if cfg!(feature = "ledger"))
            || matches!(backend, "vault" if cfg!(feature = "vault"))
            || matches!(backend, "extension" if cfg!(feature = "extension"));
        if available {
            Ok(())
        } else {
            Err(CliError::input(format!(
                "{} signing requires the corresponding CLI feature",
                Self::backend(request)
            )))
        }
    }
    fn discover<'a>(
        &'a self,
        request: &'a BackendRequest,
    ) -> BoxFuture<'a, Result<AccountId, CliError>> {
        Box::pin(async move {
            match Self::backend(request) {
                #[cfg(feature = "ledger")]
                "ledger" => {
                    use rtao::signer::Signer;
                    let signer = rtao::signer::LedgerSigner::open(ledger_options(request))
                        .await
                        .map_err(failure)?;
                    let account = signer.account();
                    self.context
                        .output
                        .message(&format!(
                            "using Ledger account {account} (m/44'/354'/{}'/0'/{}')",
                            request.globals.ledger_account, request.globals.ledger_index
                        ))
                        .map_err(failure)?;
                    Ok(account)
                }
                #[cfg(feature = "extension")]
                "extension" => {
                    let (_, _, selection) = self.extension_selection(request, None, true).await?;
                    Ok(AccountId(selection.public_key))
                }
                _ => Err(CliError::input(
                    "this signing backend cannot discover an account; supply a public signer address",
                )),
            }
        })
    }
    fn acquire<'a>(
        &'a self,
        request: &'a BackendRequest,
        reviewed: AccountId,
    ) -> BoxFuture<'a, Result<Arc<dyn BackendSession>, CliError>> {
        Box::pin(async move {
            #[cfg(not(any(feature = "vault", feature = "extension")))]
            let _ = reviewed;
            match Self::backend(request) {
                #[cfg(feature = "ledger")]
                "ledger" => {
                    let signer = rtao::signer::LedgerSigner::open(ledger_options(request))
                        .await
                        .map_err(failure)?;
                    Ok(Arc::new(NativeSession::Ledger(Arc::new(signer)))
                        as Arc<dyn BackendSession>)
                }
                #[cfg(feature = "vault")]
                "vault" => {
                    let output = self.context.output.clone();
                    let signer = rtao::signer::vault::VaultSigner::with_options(
                        &reviewed.to_string(),
                        rtao::signer::vault::VaultOptions {
                            server: rtao::signer::vault::VaultServerOptions {
                                open_browser: request.open_browser,
                                browser: self.browser(request),
                                ..Default::default()
                            },
                            on_status: Some(Arc::new(move |message| {
                                let _ = output.message(message);
                            })),
                            ..Default::default()
                        },
                    )
                    .map_err(failure)?;
                    signer
                        .set_summary(request.summary.clone())
                        .map_err(failure)?;
                    let signer = Arc::new(signer);
                    // Prepare the camera before the client starts a mortal era.
                    // Cancellation drops this owned signer and stops its page.
                    signer.warm_up().await.map_err(failure)?;
                    Ok(Arc::new(NativeSession::Vault(signer)) as Arc<dyn BackendSession>)
                }
                #[cfg(feature = "extension")]
                "extension" => {
                    let (url, token, selection) = self
                        .extension_selection(request, Some(reviewed), false)
                        .await?;
                    let signer = rtao::signer::extension::open_extension_signer(
                        &url,
                        token.as_str(),
                        selection,
                    )
                    .await
                    .map_err(failure)?;
                    Ok(Arc::new(NativeSession::Extension(Arc::new(signer)))
                        as Arc<dyn BackendSession>)
                }
                _ => Err(CliError::input("external signing backend is unavailable")),
            }
        })
    }
}
#[cfg(any(feature = "vault", feature = "extension"))]
impl NativeBackend {
    fn browser(&self, request: &BackendRequest) -> Option<String> {
        request
            .globals
            .extension_browser
            .clone()
            .filter(|value| !value.is_empty())
            .or_else(|| {
                self.context
                    .config
                    .get("extension_browser")
                    .filter(|value| !value.is_null())
                    .map(crate::globals::config_string)
            })
    }
}
#[cfg(feature = "ledger")]
fn ledger_options(request: &BackendRequest) -> rtao::signer::LedgerOptions {
    rtao::signer::LedgerOptions {
        account: request.globals.ledger_account,
        index: request.globals.ledger_index,
        ..Default::default()
    }
}

#[cfg(feature = "extension")]
impl NativeBackend {
    async fn extension_selection(
        &self,
        request: &BackendRequest,
        reviewed: Option<AccountId>,
        fresh: bool,
    ) -> Result<
        (
            String,
            zeroize::Zeroizing<String>,
            rtao::signer::extension::ExtensionAccountSelection,
        ),
        CliError,
    > {
        use rtao::signer::extension::{BridgeClient, EnsureBridgeOptions, ensure_bridge};
        let mut options = EnsureBridgeOptions::from_environment().map_err(failure)?;
        if let Some(url) = &request.globals.extension_bridge_url {
            options.url = url.clone();
        }
        options.allow_spawn = true;
        options.open_browser = request.open_browser;
        options.browser = self.browser(request);
        options.fresh = fresh;
        options.on_waiting = Some(crate::commands::extension::waiting_observer(
            &self.context.output,
        ));
        let url = ensure_bridge(&options).await.map_err(failure)?;
        let token = options
            .paths
            .read_token()
            .map_err(failure)?
            .ok_or_else(|| CliError::failure("extension bridge client token is missing"))?;
        let client = BridgeClient::new(&url, token.as_str()).map_err(failure)?;
        let accounts = client.list_accounts().await;
        client.close().await;
        let accounts = accounts.map_err(failure)?;
        let selection = choose_extension_account(&self.context, request, &accounts, reviewed)?;
        if let Some(store) = &self.context.config_store {
            store
                .set_value("signer_address", &selection.account.address)
                .await
                .map_err(failure)?;
        }
        self.context
            .output
            .message(&format!(
                "using extension account {} ({}, {})",
                selection.account.name, selection.account.address, selection.account.source
            ))
            .map_err(failure)?;
        Ok((url, token, selection))
    }
}

#[cfg(feature = "extension")]
fn extension_account_hint<'a>(
    accounts: impl IntoIterator<Item = &'a rtao::signer::extension::ExtensionAccount>,
) -> String {
    accounts
        .into_iter()
        .map(|account| format!("{} ({}, {})", account.name, account.address, account.source))
        .collect::<Vec<_>>()
        .join(", ")
}

#[cfg(feature = "extension")]
fn choose_extension_account(
    context: &AppContext,
    request: &BackendRequest,
    accounts: &[rtao::signer::extension::ExtensionAccount],
    reviewed: Option<AccountId>,
) -> Result<rtao::signer::extension::ExtensionAccountSelection, CliError> {
    use rtao::signer::extension::{ExtensionAccountSelection, account_crypto_type};
    let matches = |account: &rtao::signer::extension::ExtensionAccount, expected: AccountId| {
        account
            .address
            .parse::<AccountId>()
            .is_ok_and(|actual| actual == expected)
    };
    let selected = if let Some(expected) = reviewed.or(request.expected) {
        accounts
            .iter()
            .find(|account| matches(account, expected))
            .ok_or_else(|| {
                CliError::input(format!(
                    "the reviewed extension account is unavailable; no transaction was signed; available: {}",
                    extension_account_hint(accounts)
                ))
            })?
    } else {
        let filtered: Vec<_> = accounts
            .iter()
            .filter(|account| {
                request
                    .globals
                    .extension_source
                    .as_ref()
                    .is_none_or(|source| &account.source == source)
            })
            .collect();
        match filtered.as_slice() {
            [] => {
                return Err(CliError::input(format!(
                    "no extension account matched the given filters; available: {}",
                    extension_account_hint(accounts)
                )));
            }
            [account] => *account,
            _ => {
                if let Some(account) = request.default_address.and_then(|expected| {
                    let mut saved = filtered
                        .iter()
                        .copied()
                        .filter(|account| matches(account, expected));
                    let first = saved.next()?;
                    saved.next().is_none().then_some(first)
                }) {
                    account
                } else {
                    if !request.interactive {
                        return Err(CliError::input(format!(
                            "multiple extension accounts available; pass --signer-address or run in a terminal: {}",
                            extension_account_hint(filtered.iter().copied())
                        )));
                    }
                    let default_index = request
                        .default_address
                        .and_then(|expected| {
                            filtered
                                .iter()
                                .position(|account| matches(account, expected))
                        })
                        .unwrap_or(0);
                    let mut terminal = context
                        .terminal
                        .lock()
                        .map_err(|_| CliError::failure("terminal lock poisoned"))?;
                    for (index, account) in filtered.iter().enumerate() {
                        terminal
                            .message(&crate::output::terminal_text(&format!(
                                "{}: {} ({}, {}){}",
                                index + 1,
                                account.name,
                                account.address,
                                account.source,
                                if request
                                    .default_address
                                    .is_some_and(|expected| matches(account, expected))
                                {
                                    " (last used)"
                                } else {
                                    ""
                                }
                            )))
                            .map_err(failure)?;
                    }
                    loop {
                        let raw = terminal
                            .read_line(&format!(
                                "Select extension account [{}]: ",
                                default_index + 1
                            ))
                            .map_err(|error| CliError {
                                code: error.exit_code(),
                                ..crate::error::CliError::failure(error.to_string())
                            })?;
                        let raw = crate::prompt::python_strip(&raw);
                        let index = if raw.is_empty() {
                            Some(default_index)
                        } else {
                            rtao::primitives::pyfmt::parse_integer(raw)
                                .and_then(|index| index.to_string().parse::<usize>().ok())
                                .and_then(|index| index.checked_sub(1))
                        };
                        if let Some(account) = index.and_then(|index| filtered.get(index)) {
                            break *account;
                        }
                        terminal
                            .message("Enter a number from the list")
                            .map_err(failure)?;
                    }
                }
            }
        }
    };
    let public_key = selected
        .address
        .parse::<AccountId>()
        .map_err(|_| CliError::input("extension returned an invalid SS58 address"))?
        .0;
    let crypto_type = account_crypto_type(&selected.account_type).map_err(failure)?;
    Ok(ExtensionAccountSelection {
        account: selected.clone(),
        public_key,
        crypto_type,
        ss58_format: 42,
    })
}

#[cfg(any(feature = "ledger", feature = "vault", feature = "extension"))]
enum NativeSession {
    #[cfg(feature = "ledger")]
    Ledger(Arc<rtao::signer::LedgerSigner>),
    #[cfg(feature = "vault")]
    Vault(Arc<rtao::signer::vault::VaultSigner>),
    #[cfg(feature = "extension")]
    Extension(Arc<rtao::signer::extension::ExtensionSigner>),
}
#[cfg(any(feature = "ledger", feature = "vault", feature = "extension"))]
impl BackendSession for NativeSession {
    fn signer(&self) -> Arc<dyn rtao::signer::Signer> {
        match self {
            #[cfg(feature = "ledger")]
            Self::Ledger(signer) => signer.clone(),
            #[cfg(feature = "vault")]
            Self::Vault(signer) => signer.clone(),
            #[cfg(feature = "extension")]
            Self::Extension(signer) => signer.clone(),
        }
    }
    fn release(&self, success: bool) -> BoxFuture<'_, Result<(), CliError>> {
        Box::pin(async move {
            match self {
                #[cfg(feature = "ledger")]
                Self::Ledger(_) => {
                    let _ = success;
                    Ok(())
                }
                #[cfg(feature = "vault")]
                Self::Vault(signer) => {
                    let report = tokio::time::timeout(
                        std::time::Duration::from_secs(3),
                        signer.report_transaction_result(success),
                    )
                    .await
                    .map_err(|_| CliError::failure("Vault result notification timed out"))
                    .and_then(|result| result.map_err(failure));
                    let close = signer.close().await.map_err(failure);
                    report.and(close)
                }
                #[cfg(feature = "extension")]
                Self::Extension(signer) => {
                    let report = tokio::time::timeout(
                        std::time::Duration::from_secs(3),
                        signer.report_transaction_result(success),
                    )
                    .await
                    .map_err(|_| CliError::failure("extension result notification timed out"))
                    .and_then(|result| result.map_err(failure));
                    signer.close().await;
                    report
                }
            }
        })
    }
}
#[cfg(any(feature = "ledger", feature = "vault", feature = "extension"))]
fn failure(error: impl std::fmt::Display) -> CliError {
    CliError::failure(error.to_string())
}

#[cfg(all(test, feature = "extension"))]
mod tests {
    use super::*;
    use crate::{
        application::command_tree,
        books::Books,
        globals::{Globals, Tier, with_globals},
        prompt::{PromptError, Terminal},
    };
    use rtao::signer::extension::ExtensionAccount;
    use std::{
        collections::VecDeque,
        path::Path,
        sync::{Arc, Mutex},
    };

    struct TestTerminal(VecDeque<String>, Arc<Mutex<Vec<String>>>);
    impl Terminal for TestTerminal {
        fn stdin_is_tty(&self) -> bool {
            true
        }
        fn stderr_is_tty(&self) -> bool {
            true
        }
        fn read_line(&mut self, prompt: &str) -> Result<String, PromptError> {
            self.1
                .lock()
                .map_err(|_| PromptError::Unavailable)?
                .push(prompt.to_owned());
            self.0.pop_front().ok_or(PromptError::Aborted)
        }
        fn read_secret(&mut self, _: &str) -> Result<rtao::wallet::SecretString, PromptError> {
            Err(PromptError::Unavailable)
        }
        fn message(&mut self, message: &str) -> Result<(), PromptError> {
            self.1
                .lock()
                .map_err(|_| PromptError::Unavailable)?
                .push(message.to_owned());
            Ok(())
        }
    }
    type PickerFixture = (AppContext, BackendRequest, Arc<Mutex<Vec<String>>>);
    fn fixture(lines: &[&str]) -> Result<(AppContext, BackendRequest), Box<dyn std::error::Error>> {
        let (context, request, _) = fixture_with_output(lines)?;
        Ok((context, request))
    }
    fn fixture_with_output(lines: &[&str]) -> Result<PickerFixture, Box<dyn std::error::Error>> {
        let root = command_tree([with_globals(clap::Command::new("submit"), Tier::Tx)])
            .try_get_matches_from(["rtao", "submit", "--signer", "extension"])?;
        let mut globals = Globals::resolve(
            &root,
            &Default::default(),
            Path::new("/isolated/m7/native-picker"),
            |_| None,
        )?;
        globals.apply(root.subcommand().ok_or("submit")?.1, &[])?;
        let request = BackendRequest {
            globals: globals.clone(),
            expected: None,
            default_address: None,
            open_browser: false,
            interactive: false,
            summary: None,
        };
        let output = Arc::new(Mutex::new(Vec::new()));
        let context = AppContext::new(
            globals,
            Default::default(),
            Books::default(),
            Arc::new(Mutex::new(Box::new(TestTerminal(
                lines.iter().map(|line| (*line).to_owned()).collect(),
                output.clone(),
            )))),
        );
        Ok((context, request, output))
    }
    fn account(seed: u8, source: &str) -> ExtensionAccount {
        ExtensionAccount {
            address: AccountId([seed; 32]).to_string(),
            name: format!("account {seed}"),
            source: source.into(),
            account_type: "sr25519".into(),
        }
    }

    #[test]
    fn chooser_errors_include_available_accounts_after_the_correct_filter()
    -> Result<(), Box<dyn std::error::Error>> {
        let (context, mut request) = fixture(&[])?;
        let candidates = [
            account(1, "chosen"),
            account(2, "other"),
            account(3, "chosen"),
        ];
        request.globals.extension_source = Some("chosen".into());
        let ambiguous = choose_extension_account(&context, &request, &candidates, None)
            .err()
            .ok_or("expected ambiguity")?;
        assert_eq!(
            ambiguous.message,
            format!(
                "multiple extension accounts available; pass --signer-address or run in a terminal: account 1 ({}, chosen), account 3 ({}, chosen)",
                candidates[0].address, candidates[2].address
            )
        );
        request.globals.extension_source = Some("missing".into());
        let missing = choose_extension_account(&context, &request, &candidates, None)
            .err()
            .ok_or("expected no matching filter")?;
        let hint = format!(
            "account 1 ({}, chosen), account 2 ({}, other), account 3 ({}, chosen)",
            candidates[0].address, candidates[1].address, candidates[2].address
        );
        assert_eq!(
            missing.message,
            format!("no extension account matched the given filters; available: {hint}")
        );
        let unavailable =
            choose_extension_account(&context, &request, &candidates, Some(AccountId([4; 32])))
                .err()
                .ok_or("expected unavailable reviewed identity")?;
        assert_eq!(
            unavailable.message,
            format!(
                "the reviewed extension account is unavailable; no transaction was signed; available: {hint}"
            )
        );
        Ok(())
    }
    #[test]
    fn empty_explicit_browser_uses_config_while_whitespace_remains_explicit()
    -> Result<(), Box<dyn std::error::Error>> {
        let (mut context, mut request) = fixture(&[])?;
        context
            .config
            .insert("extension_browser".into(), serde_json::json!("firefox"));
        let backend = NativeBackend::new(&context);
        for (explicit, expected) in [
            (None, "firefox"),
            (Some(""), "firefox"),
            (Some("chrome"), "chrome"),
            (Some(" "), " "),
        ] {
            request.globals.extension_browser = explicit.map(str::to_owned);
            assert_eq!(backend.browser(&request).as_deref(), Some(expected));
        }
        Ok(())
    }
    #[test]
    fn saved_missing_or_filtered_account_can_yield_a_new_filtered_identity()
    -> Result<(), Box<dyn std::error::Error>> {
        let (context, mut request) = fixture(&[])?;
        request.globals.extension_source = Some("chosen-source".into());
        for saved in [AccountId([2; 32]), AccountId([3; 32])] {
            request.default_address = Some(saved);
            let selection = choose_extension_account(
                &context,
                &request,
                &[account(1, "chosen-source"), account(2, "other-source")],
                None,
            )?;
            assert_eq!(selection.public_key, [1; 32]);
        }
        Ok(())
    }
    #[test]
    fn remembered_account_wins_only_within_filtered_candidates()
    -> Result<(), Box<dyn std::error::Error>> {
        let (context, mut request) = fixture(&[])?;
        request.globals.extension_source = Some("chosen-source".into());
        request.default_address = Some(AccountId([2; 32]));
        let candidates = [
            account(1, "chosen-source"),
            account(2, "chosen-source"),
            account(3, "other-source"),
        ];
        assert_eq!(
            choose_extension_account(&context, &request, &candidates, None)?.public_key,
            [2; 32]
        );
        request.default_address = Some(AccountId([3; 32]));
        assert!(choose_extension_account(&context, &request, &candidates, None).is_err());
        Ok(())
    }
    #[test]
    fn duplicated_remembered_identity_requires_selection_after_filtering()
    -> Result<(), Box<dyn std::error::Error>> {
        let (context, mut request) = fixture(&[])?;
        request.default_address = Some(AccountId([2; 32]));
        let candidates = [
            account(2, "first"),
            account(2, "second"),
            account(3, "third"),
        ];
        assert!(choose_extension_account(&context, &request, &candidates, None).is_err());

        let (context, mut request) = fixture(&["2"])?;
        request.default_address = Some(AccountId([2; 32]));
        request.interactive = true;
        let selection = choose_extension_account(&context, &request, &candidates, None)?;
        assert_eq!(selection.account.source, "second");

        request.globals.extension_source = Some("first".into());
        assert_eq!(
            choose_extension_account(&context, &request, &candidates, None)?
                .account
                .source,
            "first"
        );
        Ok(())
    }
    #[test]
    fn blank_choice_preserves_the_first_saved_identity_even_when_it_is_duplicated()
    -> Result<(), Box<dyn std::error::Error>> {
        let (context, mut request, output) = fixture_with_output(&[""])?;
        request.default_address = Some(AccountId([2; 32]));
        request.interactive = true;
        let candidates = [
            account(1, "other"),
            account(2, "first"),
            account(2, "second"),
        ];
        let selection = choose_extension_account(&context, &request, &candidates, None)?;
        assert_eq!(selection.account, candidates[1]);
        let output = output.lock().map_err(|_| "trace lock")?;
        assert_eq!(
            output.last().map(String::as_str),
            Some("Select extension account [2]: ")
        );
        assert!(!output[0].contains("last used"));
        assert!(output[1].ends_with(" (last used)"));
        assert!(output[2].ends_with(" (last used)"));
        Ok(())
    }
    #[test]
    fn interactive_choice_accepts_python_decimal_spellings_and_retries_invalid_indices()
    -> Result<(), Box<dyn std::error::Error>> {
        let candidates: Vec<_> = (1..=10).map(|seed| account(seed, "fixture")).collect();
        for (answer, expected) in [("２", 2), ("1_0", 10), (" +１ ", 1)] {
            let (context, mut request) = fixture(&[answer])?;
            request.interactive = true;
            assert_eq!(
                choose_extension_account(&context, &request, &candidates, None)?.account,
                candidates[expected - 1],
                "answer {answer:?}"
            );
        }
        let (context, mut request, output) = fixture_with_output(&[
            "0",
            "-1",
            "11",
            "1__0",
            "1 0",
            "184467440737095516160000000000",
            "2",
        ])?;
        request.interactive = true;
        assert_eq!(
            choose_extension_account(&context, &request, &candidates, None)?.account,
            candidates[1]
        );
        let output = output.lock().map_err(|_| "trace lock")?;
        assert_eq!(
            output
                .iter()
                .filter(|line| line.as_str() == "Enter a number from the list")
                .count(),
            6
        );
        assert_eq!(
            output
                .iter()
                .filter(|line| line.as_str() == "Select extension account [1]: ")
                .count(),
            7
        );
        Ok(())
    }
    #[test]
    fn interactive_account_pick_and_explicit_pin_have_distinct_precedence()
    -> Result<(), Box<dyn std::error::Error>> {
        let (context, mut request) = fixture(&["2"])?;
        request.interactive = true;
        let candidates = [account(1, "one"), account(2, "two")];
        assert_eq!(
            choose_extension_account(&context, &request, &candidates, None)?.public_key,
            [2; 32]
        );
        request.expected = Some(AccountId([1; 32]));
        request.globals.extension_source = Some("two".into());
        assert_eq!(
            choose_extension_account(&context, &request, &candidates, None)?.public_key,
            [1; 32]
        );
        Ok(())
    }
    #[test]
    fn reviewed_identity_matches_public_key_across_ss58_prefixes_and_never_falls_back()
    -> Result<(), Box<dyn std::error::Error>> {
        let (context, request) = fixture(&[])?;
        let mut advertised = account(1, "chosen-source");
        advertised.address = AccountId([1; 32]).to_ss58(0);
        assert_eq!(
            choose_extension_account(
                &context,
                &request,
                &[advertised.clone()],
                Some(AccountId([1; 32]))
            )?
            .public_key,
            [1; 32]
        );
        assert!(
            choose_extension_account(&context, &request, &[advertised], Some(AccountId([2; 32])))
                .is_err()
        );
        Ok(())
    }
}
