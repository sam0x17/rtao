// Derived from RaoFoundation/subtensor sdk/python/bittensor/intents/base.py @ 850f2755bb05a3851f948489f0ab27b0645cebce (Apache-2.0); modified by rtao.
//! Typed intent contracts and their object-safe dispatch boundary.

use crate::{IntentError, IntentOutput};
use rtao_catalog::IntentSpec;
use rtao_client::{Client, DynView, View};
use rtao_primitives::{AccountId, Balance, NetUid, Spend, Tao};
use rtao_runtime::{CallBytes, CallDescriptor, RuntimeHandle};
use rtao_signer::{BoxFuture, IntentKeyView, KeyView, Role, SignerError, SignerRole};
use serde_json::{Map, Value};
use smallvec::SmallVec;
use std::{future::Future, time::SystemTime};

/// Injectable clock used by time-dependent intent construction.
pub trait Clock: Send + Sync {
    /// Current wall-clock instant.
    fn now(&self) -> SystemTime;
}
/// The process wall clock.
#[derive(Clone, Copy, Debug, Default)]
pub struct SystemClock;
impl Clock for SystemClock {
    fn now(&self) -> SystemTime {
        SystemTime::now()
    }
}

/// Origin privilege declared by a generated interface.
pub trait OriginMarker: Send + Sync + 'static {
    /// Exact catalog token.
    const NAME: &'static str;
}
/// Any signed origin.
pub enum Signed {}
/// The subnet's owning coldkey.
pub enum SubnetOwner {}
/// The chain's sudo origin.
pub enum Root {}
impl OriginMarker for Signed {
    const NAME: &'static str = "signed";
}
impl OriginMarker for SubnetOwner {
    const NAME: &'static str = "subnet_owner";
}
impl OriginMarker for Root {
    const NAME: &'static str = "root";
}

/// Public keys or a substituted proxy/multisig dispatch identity.
#[derive(Clone, Copy)]
pub enum OriginView<'a> {
    /// Attached keys, without an unlock capability.
    Keys(&'a dyn KeyView),
    /// Bare public dispatch identity.
    Account(AccountId),
}
impl KeyView for OriginView<'_> {
    fn hotkey_address(&self) -> Result<AccountId, SignerError> {
        match self {
            Self::Keys(keys) => keys.hotkey_address(),
            Self::Account(_) => Err(SignerError::KeyUnavailable(SignerRole::Hotkey)),
        }
    }
    fn hotkey_public_key(&self) -> Result<[u8; 32], SignerError> {
        match self {
            Self::Keys(keys) => keys.hotkey_public_key(),
            Self::Account(_) => Err(SignerError::KeyUnavailable(SignerRole::Hotkey)),
        }
    }
    fn coldkey_address(&self) -> Result<AccountId, SignerError> {
        match self {
            Self::Keys(keys) => keys.coldkey_address(),
            Self::Account(_) => Err(SignerError::KeyUnavailable(SignerRole::Coldkey)),
        }
    }
    fn bare_signer_account(&self) -> Option<AccountId> {
        match self {
            Self::Keys(keys) => keys.bare_signer_account(),
            Self::Account(account) => Some(*account),
        }
    }
}

/// Immutable materials for building one semantic call at one block.
pub struct BuildCx<'a, V: View + ?Sized> {
    /// Pinned state view.
    pub view: &'a V,
    /// Effective public dispatch origin.
    pub origin: OriginView<'a>,
    /// Runtime for call construction.
    pub runtime: &'a RuntimeHandle,
    /// Injectable wall clock.
    pub clock: &'a dyn Clock,
    /// Semantic intent's authenticated interface.
    pub spec: &'static IntentSpec,
}
impl<V: View + ?Sized> BuildCx<'_, V> {
    /// Resolve explicit or attached/matching-role hotkey defaults.
    pub fn hotkey_address(&self, explicit: Option<AccountId>) -> Result<AccountId, IntentError> {
        Ok(
            IntentKeyView::new(&self.origin, self.spec.signer.parse()?, self.spec.name)
                .hotkey_address(explicit)?,
        )
    }
    /// Resolve the effective hotkey's raw public key.
    pub fn hotkey_public_key(&self) -> Result<[u8; 32], IntentError> {
        Ok(
            IntentKeyView::new(&self.origin, self.spec.signer.parse()?, self.spec.name)
                .hotkey_public_key()?,
        )
    }
    /// Resolve attached or matching-role coldkey defaults.
    pub fn coldkey_address(&self) -> Result<AccountId, IntentError> {
        Ok(
            IntentKeyView::new(&self.origin, self.spec.signer.parse()?, self.spec.name)
                .coldkey_address()?,
        )
    }
}

/// Composed call and operation-specific result extras.
#[derive(Clone, Debug)]
pub struct Built {
    /// Runtime-stamped call.
    pub call: CallBytes,
    /// Verbatim Python build-time result data.
    pub extras: Map<String, Value>,
}
impl Built {
    /// Build a call without extras.
    pub fn new(call: CallBytes) -> Self {
        Self {
            call,
            extras: Map::new(),
        }
    }
}
impl From<CallBytes> for Built {
    fn from(call: CallBytes) -> Self {
        Self::new(call)
    }
}

/// Heading for structured pre-sign facts.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub enum FactsTitle {
    /// Cost breakdown.
    #[default]
    Fees,
    /// Simulated swap output.
    Quote,
    /// A user-defined heading.
    Custom(String),
}
impl FactsTitle {
    /// Renderer-visible text.
    pub fn as_str(&self) -> &str {
        match self {
            Self::Fees => "Fees",
            Self::Quote => "Quote",
            Self::Custom(value) => value,
        }
    }
}

/// State-aware preview assembled without signing.
#[derive(Clone, Debug, Default)]
pub struct IntentPreflight {
    /// Predicted effects.
    pub effects: Vec<String>,
    /// Nonfatal cautions.
    pub warnings: Vec<String>,
    /// Hard stops merged into policy violations.
    pub blocks: Vec<String>,
    /// Call's required free TAO, excluding shield carrier overhead.
    pub required_free: Option<Balance<Tao>>,
    /// Available free TAO at the pinned origin.
    pub available_free: Option<Balance<Tao>>,
    /// Exact estimate for the fully wrapped call.
    pub estimated_fee: Option<Balance<Tao>>,
    /// Structural preview facts.
    pub facts: Vec<(String, String)>,
    /// Heading for structural facts.
    pub facts_title: FactsTitle,
}

/// Effective origin, payer, state and wrapped call used by preflight hooks.
pub struct PreflightCx<'a, V: View + ?Sized> {
    /// Pinned state view.
    pub view: &'a V,
    /// Runtime belonging to the view.
    pub runtime: &'a RuntimeHandle,
    /// Injectable wall clock.
    pub clock: &'a dyn Clock,
    /// Authenticated interface.
    pub spec: &'static IntentSpec,
    /// Client for public-identity fee probes.
    pub client: &'a Client,
    /// Inner dispatch account.
    pub dispatch_origin: AccountId,
    /// Outer signing account.
    pub fee_payer: AccountId,
    /// Fully composed/wrapped call being reviewed.
    pub call: &'a CallBytes,
}
impl<V: View + ?Sized> PreflightCx<'_, V> {
    /// Probe the fully wrapped call without unlocking a key.
    pub async fn estimate_fee(&self) -> Result<Balance<Tao>, IntentError> {
        Ok(Balance::<Tao>::from_rao(
            self.client
                .estimate_weight(
                    self.call.clone(),
                    self.fee_payer,
                    rtao_primitives::CryptoType::Sr25519,
                )
                .await?
                .partial_fee,
        ))
    }
}

/// Generated, typed metadata for one intent.
pub trait IntentInterface: Send + Sync + 'static {
    /// Authenticated catalog record.
    const SPEC: &'static IntentSpec;
    /// Signing role required at compile time.
    type Role: Role;
    /// Required origin privilege.
    type Origin: OriginMarker;
    /// Typed interpretation of Python result extras.
    type Output: IntentOutput;
    /// Calls this intent may compose.
    const WRAPS: &'static [&'static CallDescriptor];
    /// Exact JSON-native arguments including the operation name.
    fn to_json(&self) -> Value;
}

/// Behavior hooks corresponding to Python's Intent ABC.
pub trait Intent: IntentInterface + Sized {
    /// One-line human summary.
    fn summary(&self) -> String;
    /// Build the semantic call against the effective origin.
    fn build<V: View + ?Sized>(
        &self,
        cx: &BuildCx<'_, V>,
    ) -> impl Future<Output = Result<Built, IntentError>> + Send;
    /// Predicted effects, defaulting to the summary.
    fn effects<V: View + ?Sized>(
        &self,
        _cx: &PreflightCx<'_, V>,
    ) -> impl Future<Output = Result<Vec<String>, IntentError>> + Send {
        async { Ok(vec![self.summary()]) }
    }
    /// Nonfatal warnings.
    fn warnings<V: View + ?Sized>(
        &self,
        _cx: &PreflightCx<'_, V>,
    ) -> impl Future<Output = Result<Vec<String>, IntentError>> + Send {
        async { Ok(vec![]) }
    }
    /// Hard stops before signing.
    fn blocks<V: View + ?Sized>(
        &self,
        _cx: &PreflightCx<'_, V>,
    ) -> impl Future<Output = Result<Vec<String>, IntentError>> + Send {
        async { Ok(vec![]) }
    }
    /// Compose the three preview hooks in Python's order.
    fn preflight<V: View + ?Sized>(
        &self,
        cx: &PreflightCx<'_, V>,
    ) -> impl Future<Output = Result<IntentPreflight, IntentError>> + Send {
        async {
            Ok(IntentPreflight {
                effects: self.effects(cx).await?,
                warnings: self.warnings(cx).await?,
                blocks: self.blocks(cx).await?,
                ..Default::default()
            })
        }
    }
    /// TAO leaving the dispatch account.
    fn spend(&self) -> Spend {
        Spend::None
    }
    /// Python's base default touches its own netuid field only.
    fn touches_netuids(&self) -> SmallVec<[NetUid; 2]> {
        self.to_json()
            .get("netuid")
            .and_then(Value::as_u64)
            .and_then(|value| u16::try_from(value).ok())
            .map(NetUid)
            .into_iter()
            .collect()
    }
    /// Whether the operation spans every subnet.
    fn affects_all_subnets(&self) -> bool {
        false
    }
    /// Apply an execution adapter after semantic, sudo and proxy composition.
    fn wrap_call<V: View + ?Sized>(
        &self,
        _cx: &BuildCx<'_, V>,
        call: CallBytes,
    ) -> impl Future<Output = Result<Built, IntentError>> + Send {
        async { Ok(Built::new(call)) }
    }
    /// Public origin substituted by adapters.
    fn origin_view<'a>(&'a self, keys: &'a dyn KeyView) -> OriginView<'a> {
        OriginView::Keys(keys)
    }
    /// Constructor validation before any reads or signing.
    fn validate(&self) -> Result<(), IntentError> {
        Ok(())
    }
    /// Inner intent whose policy and build semantics govern the operation.
    fn semantic_intent(&self) -> &dyn DynIntent {
        self
    }
    /// Exact bytes pinned by an adapter's first approval.
    fn inner_call_data(&self) -> Option<&CallBytes> {
        None
    }
}

/// Object-safe behavior twin for CLI and tool dispatch.
pub trait DynIntent: crate::PolicySubject + Send + Sync {
    /// Authenticated catalog record.
    fn spec(&self) -> &'static IntentSpec;
    /// Wrapped descriptors.
    fn wraps(&self) -> &'static [&'static CallDescriptor];
    /// Exact intent JSON.
    fn to_json_dyn(&self) -> Value;
    /// Human summary.
    fn summary_dyn(&self) -> String;
    /// Constructor validation.
    fn validate_dyn(&self) -> Result<(), IntentError>;
    /// Policy subject's TAO spend.
    fn spend_dyn(&self) -> Spend;
    /// Touched subnet IDs.
    fn touches_netuids_dyn(&self) -> SmallVec<[NetUid; 2]>;
    /// Global-subnet effect flag.
    fn affects_all_subnets_dyn(&self) -> bool;
    /// Semantic policy/build intent.
    fn semantic_intent_dyn(&self) -> &dyn DynIntent;
    /// Adapter's pinned call.
    fn inner_call_data_dyn(&self) -> Option<&CallBytes>;
    /// Public dispatch origin.
    fn origin_view_dyn<'a>(&'a self, keys: &'a dyn KeyView) -> OriginView<'a>;
    /// Dynamic build hook.
    fn build_dyn<'a>(
        &'a self,
        cx: &'a BuildCx<'a, dyn DynView + 'a>,
    ) -> BoxFuture<'a, Result<Built, IntentError>>;
    /// Dynamic preview hook.
    fn preflight_dyn<'a>(
        &'a self,
        cx: &'a PreflightCx<'a, dyn DynView + 'a>,
    ) -> BoxFuture<'a, Result<IntentPreflight, IntentError>>;
    /// Dynamic execution adapter hook.
    fn wrap_call_dyn<'a>(
        &'a self,
        cx: &'a BuildCx<'a, dyn DynView + 'a>,
        call: CallBytes,
    ) -> BoxFuture<'a, Result<Built, IntentError>>;
}
impl<I: Intent> DynIntent for I {
    fn spec(&self) -> &'static IntentSpec {
        I::SPEC
    }
    fn wraps(&self) -> &'static [&'static CallDescriptor] {
        I::WRAPS
    }
    fn to_json_dyn(&self) -> Value {
        self.to_json()
    }
    fn summary_dyn(&self) -> String {
        self.summary()
    }
    fn validate_dyn(&self) -> Result<(), IntentError> {
        self.validate()
    }
    fn spend_dyn(&self) -> Spend {
        Intent::spend(self)
    }
    fn touches_netuids_dyn(&self) -> SmallVec<[NetUid; 2]> {
        Intent::touches_netuids(self)
    }
    fn affects_all_subnets_dyn(&self) -> bool {
        Intent::affects_all_subnets(self)
    }
    fn semantic_intent_dyn(&self) -> &dyn DynIntent {
        self.semantic_intent()
    }
    fn inner_call_data_dyn(&self) -> Option<&CallBytes> {
        self.inner_call_data()
    }
    fn origin_view_dyn<'a>(&'a self, keys: &'a dyn KeyView) -> OriginView<'a> {
        self.origin_view(keys)
    }
    fn build_dyn<'a>(
        &'a self,
        cx: &'a BuildCx<'a, dyn DynView + 'a>,
    ) -> BoxFuture<'a, Result<Built, IntentError>> {
        Box::pin(self.build(cx))
    }
    fn preflight_dyn<'a>(
        &'a self,
        cx: &'a PreflightCx<'a, dyn DynView + 'a>,
    ) -> BoxFuture<'a, Result<IntentPreflight, IntentError>> {
        Box::pin(self.preflight(cx))
    }
    fn wrap_call_dyn<'a>(
        &'a self,
        cx: &'a BuildCx<'a, dyn DynView + 'a>,
        call: CallBytes,
    ) -> BoxFuture<'a, Result<Built, IntentError>> {
        Box::pin(self.wrap_call(cx, call))
    }
}
