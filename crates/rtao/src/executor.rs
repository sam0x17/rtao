//! Facade construction of an executor with generated JSON tool dispatch.

use std::{ops::Deref, sync::Arc};

use rtao_client::Client;
use rtao_tx::{Clock, IntentRegistry, Policy};

/// Transaction executor with the facade's enabled intent catalog attached.
///
/// All planning, execution and submission methods are those of
/// [`rtao_tx::Executor`]. When the `intents` feature is enabled, `new` also
/// installs the generated parser used by `execute_tool`.
#[derive(Clone)]
pub struct Executor(rtao_tx::Executor);

impl Executor {
    /// Share a verified client and its nonce manager.
    pub fn new(client: Client) -> Self {
        #[cfg(feature = "intents")]
        let executor = rtao_intents::executor(client);
        #[cfg(not(feature = "intents"))]
        let executor = rtao_tx::Executor::new(client);
        Self(executor)
    }

    /// Install the default semantic safety policy.
    pub fn with_policy(self, policy: Policy) -> Self {
        Self(self.0.with_policy(policy))
    }

    /// Supply a clock for deterministic time-dependent construction.
    pub fn with_clock(self, clock: Arc<dyn Clock>) -> Self {
        Self(self.0.with_clock(clock))
    }

    /// Replace the generated parser with an application-owned registry.
    pub fn with_registry(self, registry: Arc<dyn IntentRegistry>) -> Self {
        Self(self.0.with_registry(registry))
    }

    /// Recover the transaction-layer executor, retaining its registry and policy.
    pub fn into_inner(self) -> rtao_tx::Executor {
        self.0
    }
}

impl Deref for Executor {
    type Target = rtao_tx::Executor;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}
