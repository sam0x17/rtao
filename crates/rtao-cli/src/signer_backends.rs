// Derived from RaoFoundation/subtensor sdk/python/bittensor/cli/context.py @ 850f2755bb05a3851f948489f0ab27b0645cebce (Apache-2.0); modified by rtao.
//! Public external-account discovery and lazy, account-bound signing sessions.
//!
//! A pinned account needs no device or browser for planning. Without a pin,
//! Ledger and extension backends discover a public identity before planning;
//! obtaining the signer remains behind the normal confirmation and durable hook.

use std::{
    collections::BTreeMap,
    sync::{
        Arc, Weak,
        atomic::{AtomicBool, Ordering},
    },
};

use parking_lot::Mutex;
use rtao::{
    primitives::AccountId,
    signer::{BoxFuture, DynKeySource, KeyView, Signer, SignerError, SignerRole},
};
use tokio::sync::Mutex as AsyncMutex;

use crate::{
    call::{RawSignerFactory, SigningSelection, WalletSignerFactory},
    context::{AppContext, ExternalSignerFactory},
    error::CliError,
    globals::Globals,
};

mod native;

/// Public backend options retained for a single reviewed account.
#[derive(Clone, Debug)]
pub struct BackendRequest {
    /// Selected backend and its public CLI options.
    pub globals: Globals,
    /// Account supplied by a flag, member reference, config or companion wallet.
    pub expected: Option<AccountId>,
    /// Remembered extension choice; filtering and a new selection may supersede it.
    pub default_address: Option<AccountId>,
    /// Permit an automatic browser tab only for a visible terminal session.
    pub open_browser: bool,
    /// Permit an interactive choice among advertised extension accounts.
    pub interactive: bool,
}

/// An acquired session must own its transport and dispose of it on drop.
///
/// Implementations must not retain a session in a process-global cache. This
/// ownership rule also closes sessions when a submission future is cancelled.
pub trait BackendSession: Send + Sync {
    /// The signer tied to this session's selected account.
    fn signer(&self) -> Arc<dyn Signer>;
    /// Report a chain outcome and close its UI/transport, including on failure.
    fn release(&self, success: bool) -> BoxFuture<'_, Result<(), CliError>>;
}

/// Injectable external boundary separating public discovery from authorization.
///
/// No private key is read by discovery. `acquire` runs only when the executor
/// asks the lazy key source for its signer, after confirmation and durable hooks.
pub trait SigningBackend: Send + Sync {
    /// Reject an unavailable backend without touching any external resource.
    fn validate(&self, _request: &BackendRequest) -> Result<(), CliError> {
        Ok(())
    }
    /// Discover an explicitly selected backend's public account, without signing.
    fn discover<'a>(
        &'a self,
        request: &'a BackendRequest,
    ) -> BoxFuture<'a, Result<AccountId, CliError>>;
    /// Open a signing session for the exact account used in the reviewed plan.
    fn acquire<'a>(
        &'a self,
        request: &'a BackendRequest,
        reviewed: AccountId,
    ) -> BoxFuture<'a, Result<Arc<dyn BackendSession>, CliError>>;
}

/// Production Ledger, Vault and extension factory, with an injectable boundary.
///
/// Constructing a factory is inert. The registry contains weak references only;
/// cancellation cannot leave an acquired source alive through the factory.
pub struct NativeSignerFactory {
    context: AppContext,
    backend: Arc<dyn SigningBackend>,
    active: Mutex<BTreeMap<usize, Weak<LazyState>>>,
}
impl NativeSignerFactory {
    /// Bind CLI output, account books and terminal policy without opening a backend.
    pub fn new(context: &AppContext) -> Self {
        Self::with_backend(context, Arc::new(native::NativeBackend::new(context)))
    }
    /// Use an injected public-discovery and signing boundary.
    pub fn with_backend(context: &AppContext, backend: Arc<dyn SigningBackend>) -> Self {
        Self {
            context: context.clone(),
            backend,
            active: Mutex::new(BTreeMap::new()),
        }
    }
    fn request(&self, globals: &Globals) -> Result<BackendRequest, CliError> {
        let mut context = self.context.clone();
        context.globals = globals.clone();
        let backend = globals
            .signer_backend
            .as_deref()
            .unwrap_or("wallet")
            .trim()
            .to_lowercase();
        if !matches!(backend.as_str(), "ledger" | "vault" | "extension") {
            return Err(CliError::input(format!(
                "unknown external signing backend {backend:?}"
            )));
        }
        context.globals.signer_backend = Some(backend.clone());
        let mut expected = context.external_signer_address()?;
        let default_address = (backend == "extension")
            .then(|| {
                context
                    .config
                    .get("signer_address")
                    .and_then(serde_json::Value::as_str)
                    .and_then(|value| value.parse().ok())
            })
            .flatten();
        // Python treats a saved extension account as a preference, while an
        // explicit flag or selected multisig member binds the transaction.
        if backend == "extension"
            && globals.signer_address.as_deref().is_none_or(str::is_empty)
            && globals.signatories.is_empty()
        {
            expected = None;
        }
        if globals
            .signer_address
            .as_deref()
            .is_some_and(|value| !value.is_empty())
            && expected.is_none()
        {
            return Err(CliError::input(
                "--signer-address is not a valid SS58 address or known account reference",
            ));
        }
        if backend == "vault" && expected.is_none() {
            return Err(CliError::input("the Vault signer needs an account address").with_help("pass --signer-address, a saved multisig member, or a wallet whose public coldkey matches Polkadot Vault"));
        }
        let terminal = context
            .terminal
            .lock()
            .map_err(|_| CliError::failure("terminal lock poisoned"))?;
        let request = BackendRequest {
            globals: context.globals,
            expected,
            default_address,
            open_browser: !globals.quiet && terminal.stderr_is_tty(),
            interactive: !globals.json && terminal.stdin_is_tty(),
        };
        self.backend.validate(&request)?;
        Ok(request)
    }
    async fn keys_for(
        &self,
        globals: &Globals,
        role: Option<SignerRole>,
    ) -> Result<Arc<dyn DynKeySource>, CliError> {
        let mut request = self.request(globals)?;
        let account = match request.expected {
            Some(account) => account,
            None if globals.dry_run => {
                return Err(CliError::input(
                    "an external signer preview needs a public account address",
                )
                .with_help(
                    "pass --signer-address so planning can run without opening a device or browser",
                ));
            }
            None => self.backend.discover(&request).await?,
        };
        request.expected = Some(account);
        let state = Arc::new(LazyState {
            backend: self.backend.clone(),
            request,
            account,
            role,
            session: AsyncMutex::new(None),
            closed: AtomicBool::new(false),
        });
        let keys = Arc::new(LazyKeys(state.clone()));
        let id = Arc::as_ptr(&keys) as usize;
        let mut active = self.active.lock();
        active.retain(|_, state| state.strong_count() > 0);
        active.insert(id, Arc::downgrade(&state));
        Ok(keys)
    }
    async fn release_keys(&self, keys: &dyn DynKeySource, success: bool) -> Result<(), CliError> {
        let id = keys as *const dyn DynKeySource as *const () as usize;
        let state = self
            .active
            .lock()
            .remove(&id)
            .and_then(|state| state.upgrade());
        if let Some(state) = state {
            state.closed.store(true, Ordering::Release);
            // Taking ownership makes cancellation of release dispose of the
            // transport as well. A preview has no session and reports nothing.
            let session = state.session.lock().await.take();
            if let Some(session) = session {
                session.release(success).await?;
            }
        }
        Ok(())
    }
}
impl ExternalSignerFactory for NativeSignerFactory {
    fn keys<'a>(
        &'a self,
        globals: &'a Globals,
        role: SignerRole,
    ) -> BoxFuture<'a, Result<Arc<dyn DynKeySource>, CliError>> {
        Box::pin(self.keys_for(globals, Some(role)))
    }
    fn release<'a>(
        &'a self,
        _globals: &'a Globals,
        keys: &'a dyn DynKeySource,
        success: bool,
    ) -> BoxFuture<'a, Result<(), CliError>> {
        Box::pin(self.release_keys(keys, success))
    }
}
impl RawSignerFactory for NativeSignerFactory {
    fn public_selection<'a>(
        &'a self,
        ctx: &'a AppContext,
        selection: &'a SigningSelection,
    ) -> BoxFuture<'a, Result<SigningSelection, CliError>> {
        Box::pin(async move {
            if selection.backend == "wallet" {
                return Ok(selection.clone());
            }
            let keys = self.source(ctx, selection).await?;
            let account = keys
                .bare_signer_account()
                .ok_or_else(|| CliError::input("external signer has no public identity"))?;
            self.release_keys(keys.as_ref(), false).await?;
            Ok(SigningSelection {
                address: Some(account),
                ..selection.clone()
            })
        })
    }
    fn source<'a>(
        &'a self,
        ctx: &'a AppContext,
        selection: &'a SigningSelection,
    ) -> BoxFuture<'a, Result<Arc<dyn DynKeySource>, CliError>> {
        Box::pin(async move {
            if selection.backend == "wallet" {
                return WalletSignerFactory.source(ctx, selection).await;
            }
            let mut globals = ctx.globals.clone();
            globals.wallet_name = selection.wallet.clone();
            globals.signer_backend = Some(selection.backend.clone());
            if let Some(account) = selection.address {
                globals.signer_address = Some(account.to_string());
            }
            self.keys_for(&globals, None).await
        })
    }
    fn release<'a>(
        &'a self,
        _selection: &'a SigningSelection,
        keys: &'a dyn DynKeySource,
        success: bool,
    ) -> BoxFuture<'a, Result<(), CliError>> {
        Box::pin(self.release_keys(keys, success))
    }
}

struct LazyState {
    backend: Arc<dyn SigningBackend>,
    request: BackendRequest,
    account: AccountId,
    role: Option<SignerRole>,
    session: AsyncMutex<Option<Arc<dyn BackendSession>>>,
    closed: AtomicBool,
}
struct LazyKeys(Arc<LazyState>);
impl KeyView for LazyKeys {
    fn hotkey_address(&self) -> Result<AccountId, SignerError> {
        Err(SignerError::KeyUnavailable(SignerRole::Hotkey))
    }
    fn hotkey_public_key(&self) -> Result<[u8; 32], SignerError> {
        Err(SignerError::KeyUnavailable(SignerRole::Hotkey))
    }
    fn coldkey_address(&self) -> Result<AccountId, SignerError> {
        Err(SignerError::KeyUnavailable(SignerRole::Coldkey))
    }
    fn bare_signer_account(&self) -> Option<AccountId> {
        Some(self.0.account)
    }
}
impl DynKeySource for LazyKeys {
    fn signer_for<'a>(
        &'a self,
        role: SignerRole,
    ) -> BoxFuture<'a, Result<Arc<dyn Signer>, SignerError>> {
        Box::pin(async move {
            if self.0.role.is_some_and(|expected| expected != role) {
                return Err(SignerError::RoleUnavailable(role));
            }
            let mut slot = self.0.session.lock().await;
            if self.0.closed.load(Ordering::Acquire) {
                return Err(SignerError::KeyView(
                    "external signing session is closed".into(),
                ));
            }
            if let Some(session) = slot.as_ref() {
                return Ok(session.signer());
            }
            let session = self
                .0
                .backend
                .acquire(&self.0.request, self.0.account)
                .await
                .map_err(|error| SignerError::KeyView(error.to_string()))?;
            let signer = session.signer();
            if signer.account() != self.0.account {
                let _ = session.release(false).await;
                return Err(SignerError::KeyView("external signer account does not match the reviewed account; no transaction was signed".into()));
            }
            *slot = Some(session);
            Ok(signer)
        })
    }
}
