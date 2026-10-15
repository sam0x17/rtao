// Derived from RaoFoundation/subtensor sdk/python/tests/harness/fake_substrate.py @ 850f2755bb05a3851f948489f0ab27b0645cebce (Apache-2.0); modified by rtao.
//! Shared chain state and JSON-RPC dispatch for the socket and in-process fakes.
//!
//! Seeds use the live metadata's storage hashers and SCALE encoder. Requests
//! therefore exercise client key construction and decoding just as node RPCs do.

use std::{
    collections::{BTreeMap, BTreeSet, VecDeque},
    fmt,
    sync::Arc,
};

use blake2::{
    Blake2b, Digest,
    digest::consts::{U8, U32},
};
use frame_metadata::{RuntimeMetadata, RuntimeMetadataPrefixed};
use parity_scale_codec::{Decode, Encode};
use rtao_core::value_ext::ValueExt;
use rtao_core::{
    CoreError,
    codec::{Value as CoreValue, storage::storage_prefix},
    runtime::{Runtime, TypeSpec},
};
use rtao_primitives::IntoCoreValue;
use scale_info::{TypeDef, TypeDefPrimitive};
use serde_json::{Value, json};

use crate::fixtures::Golden;

/// A raw JSON-RPC request, retaining its caller-selected identifier.
#[derive(Debug, Clone, PartialEq, serde::Deserialize)]
pub struct RpcRequest {
    /// JSON-RPC identifier.
    pub id: Value,
    /// RPC method name.
    pub method: String,
    /// Positional RPC parameters.
    #[serde(default = "empty_params")]
    pub params: Value,
}

/// A JSON-RPC error object.
#[derive(Debug, Clone, PartialEq)]
pub struct RpcFailure {
    /// JSON-RPC or node-defined error code.
    pub code: i64,
    /// Error message.
    pub message: String,
    /// Optional node-specific details.
    pub data: Option<Value>,
}

/// One subscription item emitted alongside a reply.
#[derive(Debug, Clone, PartialEq)]
pub struct Notification {
    /// Server-selected subscription identifier.
    pub subscription: String,
    /// Subscription result, without the JSON-RPC envelope.
    pub result: Value,
}

/// A response plus subscription items, or an intentionally unanswered request.
#[derive(Debug, Clone, PartialEq)]
pub struct Reply {
    /// `None` deliberately leaves the request unanswered.
    pub outcome: Option<Result<Value, RpcFailure>>,
    /// Subscription items emitted before or after the response.
    pub notifications: Vec<Notification>,
    /// Send the subscription items before the acknowledgement when true.
    pub early: bool,
}

impl Reply {
    /// Return a successful JSON-RPC result.
    pub fn result(value: Value) -> Self {
        Self {
            outcome: Some(Ok(value)),
            notifications: Vec::new(),
            early: false,
        }
    }

    /// Return a JSON-RPC error.
    pub fn error(code: i64, message: impl Into<String>, data: Option<Value>) -> Self {
        Self {
            outcome: Some(Err(RpcFailure {
                code,
                message: message.into(),
                data,
            })),
            notifications: Vec::new(),
            early: false,
        }
    }

    /// Deliberately provide no response.
    pub fn silent() -> Self {
        Self {
            outcome: None,
            notifications: Vec::new(),
            early: false,
        }
    }

    /// Acknowledge a subscription and emit the supplied items.
    pub fn subscription(id: impl Into<String>, initial_items: Vec<Value>, early: bool) -> Self {
        let id = id.into();
        Self {
            outcome: Some(Ok(Value::String(id.clone()))),
            notifications: initial_items
                .into_iter()
                .map(|result| Notification {
                    subscription: id.clone(),
                    result,
                })
                .collect(),
            early,
        }
    }
}

/// Scripted finite notifications for one subscription request.
#[derive(Debug, Clone, PartialEq)]
pub struct SubscriptionScript {
    /// Notification result values, in order.
    pub items: Vec<Value>,
    /// Emit the items before acknowledging the subscription.
    pub early: bool,
}

impl SubscriptionScript {
    /// Emit items after the acknowledgement.
    pub fn new(items: Vec<Value>) -> Self {
        Self {
            items,
            early: false,
        }
    }
}

/// Extrinsic lifecycle statuses emitted by `author_submitAndWatchExtrinsic`.
#[derive(Debug, Clone, PartialEq)]
pub enum SubmitScript {
    /// Emit inclusion and finalization, seeding SCALE-encoded events in that block.
    InBlockThenFinalized {
        /// Python-shaped System.Events records.
        events: Value,
    },
    /// Reject the extrinsic with an optional test diagnostic.
    Invalid(String),
    /// Another transaction supplanted this extrinsic.
    Usurped(String),
    /// The transaction pool dropped the extrinsic.
    Dropped,
    /// Inclusion was retracted by a fork.
    Retracted(String),
    /// The node reports a finality timeout for the supplied block.
    FinalityTimeout(String),
    /// Emit an exact sequence, including retraction and reinclusion paths.
    Statuses(Vec<Value>),
}

type Handler = Box<dyn FnMut(&mut FakeState, &RpcRequest) -> Reply + Send>;
type Storage = BTreeMap<Vec<u8>, Option<Vec<u8>>>;

#[derive(Clone)]
struct Block {
    number: u64,
    parent: String,
    extrinsics: Vec<Vec<u8>>,
}

/// Metadata-backed state shared by `FakeNode` and `FakeChain`.
pub struct FakeState {
    /// Current parsed metadata used to encode new seeds.
    pub runtime: Arc<Runtime>,
    /// Every dispatched request, including scripted replies.
    pub requests: Vec<RpcRequest>,
    /// Complete SCALE extrinsics submitted through either author method.
    pub submissions: Vec<Vec<u8>>,
    /// Current best block number.
    pub block: u64,
    /// Current finalized block number.
    pub finalized: u64,
    /// Genesis block hash.
    pub genesis_hash: String,
    /// Transaction fee in native rao.
    pub fee: u64,
    /// Transaction weight as `ref_time` and `proof_size`.
    pub weight: Value,
    /// Exact resolved DEFAULT_STORAGE table, including entries absent from metadata.
    pub python_storage_defaults: BTreeMap<(String, String), Value>,
    /// Exact resolved DEFAULT_CONSTANTS table.
    pub python_constant_defaults: BTreeMap<(String, String), Value>,
    /// Exact resolved DEFAULT_RUNTIME table.
    pub python_runtime_defaults: BTreeMap<(String, String), Value>,
    storage: Storage,
    storage_at: BTreeMap<String, Storage>,
    storage_defaults: BTreeMap<Vec<u8>, Vec<u8>>,
    runtimes_at: BTreeMap<String, Arc<Runtime>>,
    runtime_results: BTreeMap<(String, Vec<u8>), Vec<u8>>,
    runtime_defaults: BTreeMap<String, Vec<u8>>,
    nonces: BTreeMap<String, u64>,
    blocks: BTreeMap<String, Block>,
    canonical: BTreeMap<u64, String>,
    handlers: BTreeMap<String, Handler>,
    dispatching: bool,
    results: BTreeMap<String, Value>,
    replies: BTreeMap<String, VecDeque<Reply>>,
    subscriptions: BTreeMap<String, VecDeque<SubscriptionScript>>,
    submit_scripts: VecDeque<SubmitScript>,
    next_subscription: u64,
    next_fork: u64,
}

impl fmt::Debug for FakeState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("FakeState")
            .field("spec_version", &self.runtime.spec_version)
            .field("block", &self.block)
            .field("finalized", &self.finalized)
            .field("storage_entries", &self.storage.len())
            .field("requests", &self.requests.len())
            .field("submissions", &self.submissions.len())
            .finish_non_exhaustive()
    }
}

impl Default for FakeState {
    #[allow(clippy::panic)] // Checked-in metadata and extraction are validated build inputs.
    fn default() -> Self {
        match Self::try_default() {
            Ok(state) => state,
            Err(error) => panic!("checked-in fake-state fixtures are invalid: {error}"),
        }
    }
}

impl FakeState {
    /// Parse the checked-in finney runtime and load extracted Python defaults.
    pub fn try_default() -> Result<Self, CoreError> {
        let runtime = Runtime::parse(
            include_bytes!("../../../../vendor/metadata/finney/478.v15.scale"),
            478,
            1,
            42,
        )
        .map_err(CoreError::from)?;
        let mut state = Self::new(Arc::new(runtime));
        state.genesis_hash =
            "0x2f0555cc76fc2840a25a6ea3b9637146806f1f44b090c175ffde2a7e5ab36c03".into();
        state.load_python_defaults()?;
        Ok(state)
    }

    /// Create an empty fake at block 100, with native runtime defaults on misses.
    pub fn new(runtime: Arc<Runtime>) -> Self {
        let mut state = Self {
            runtime,
            requests: Vec::new(),
            submissions: Vec::new(),
            block: 100,
            finalized: 100,
            genesis_hash: zero_hash(),
            fee: 124_414,
            weight: json!({"ref_time": 1_000_000, "proof_size": 3_593}),
            python_storage_defaults: BTreeMap::new(),
            python_constant_defaults: BTreeMap::new(),
            python_runtime_defaults: BTreeMap::new(),
            storage: BTreeMap::new(),
            storage_at: BTreeMap::new(),
            storage_defaults: BTreeMap::new(),
            runtimes_at: BTreeMap::new(),
            runtime_results: BTreeMap::new(),
            runtime_defaults: BTreeMap::new(),
            nonces: BTreeMap::new(),
            blocks: BTreeMap::new(),
            canonical: BTreeMap::new(),
            handlers: BTreeMap::new(),
            dispatching: false,
            results: BTreeMap::new(),
            replies: BTreeMap::new(),
            subscriptions: BTreeMap::new(),
            submit_scripts: VecDeque::new(),
            next_subscription: 0,
            next_fork: 0,
        };
        state.ensure_canonical(100);
        state
    }

    /// Adopt the golden's metadata and recorded chain identity.
    pub fn from_golden(golden: &Golden) -> Result<Self, CoreError> {
        let metadata = crate::fixtures::metadata_bytes(&golden.metadata.v15_hex)
            .map_err(|e| CoreError::Codec(e.to_string()))?;
        let runtime = Runtime::parse(
            &metadata,
            golden.network.spec_version,
            golden.network.transaction_version,
            golden.network.ss58_format,
        )
        .map_err(CoreError::from)?;
        let mut state = Self::new(Arc::new(runtime));
        state.genesis_hash.clone_from(&golden.network.genesis_hash);
        Ok(state)
    }

    /// Override a method with a handler that can mutate the shared state.
    pub fn set_handler(
        &mut self,
        method: impl Into<String>,
        handler: impl FnMut(&mut Self, &RpcRequest) -> Reply + Send + 'static,
    ) {
        self.handlers.insert(method.into(), Box::new(handler));
    }

    /// Override a method with a persistent raw JSON result.
    pub fn set_result(&mut self, method: impl Into<String>, result: Value) {
        self.results.insert(method.into(), result);
    }

    /// Queue one reply ahead of handlers and built-in dispatch.
    pub fn queue_reply(&mut self, method: impl Into<String>, reply: Reply) {
        self.replies
            .entry(method.into())
            .or_default()
            .push_back(reply);
    }

    /// Queue one finite subscription script.
    pub fn queue_subscription(&mut self, method: impl Into<String>, script: SubscriptionScript) {
        self.subscriptions
            .entry(method.into())
            .or_default()
            .push_back(script);
    }

    /// Queue one submission lifecycle; an empty queue includes and finalizes.
    pub fn queue_submit(&mut self, script: SubmitScript) {
        self.submit_scripts.push_back(script);
    }

    /// Set the next nonce returned for an SS58 account.
    pub fn set_nonce(&mut self, address: impl Into<String>, nonce: u64) {
        self.nonces.insert(address.into(), nonce);
    }

    /// Seed complete raw storage bytes. `None` explicitly overrides defaults with a miss.
    pub fn seed_encoded(&mut self, key: Vec<u8>, value: impl Into<Option<Vec<u8>>>) {
        self.storage.insert(key, value.into());
    }

    /// Seed complete raw storage bytes at one block hash.
    pub fn seed_at(&mut self, hash: impl Into<String>, key: Vec<u8>, value: Option<Vec<u8>>) {
        let hash = hash.into();
        if self.block_hash(self.block).as_deref() == Some(hash.as_str()) {
            self.storage.insert(key.clone(), value.clone());
        }
        self.storage_at
            .entry(hash)
            .or_insert_with(|| self.storage.clone())
            .insert(key, value);
    }

    /// Encode typed key/value carriers through the live core, without a descriptor.
    pub fn seed_typed<K: IntoCoreValue, V: IntoCoreValue>(
        &mut self,
        pallet: &str,
        item: &str,
        params: K,
        value: V,
    ) -> Result<(), CoreError> {
        let params = params.into_core_value();
        let params = match params {
            CoreValue::Tuple(items) | CoreValue::List(items) => items,
            CoreValue::Null => Vec::new(),
            value => vec![value],
        };
        self.seed_core(pallet, item, &params, value.into_core_value())
    }

    /// Seed a typed storage descriptor using its names and the live metadata's hashers.
    pub fn seed<K: IntoCoreValue, V: IntoCoreValue>(
        &mut self,
        descriptor: &rtao_runtime::StorageDescriptor<K, V>,
        params: K,
        value: V,
    ) -> Result<(), CoreError> {
        self.seed_typed(
            descriptor.surface.pallet,
            descriptor.surface.name,
            params,
            value,
        )
    }

    /// Seed Python-shaped JSON, completing omitted composite fields with native zero values.
    pub fn seed_python(
        &mut self,
        pallet: &str,
        item: &str,
        params: &[Value],
        value: Value,
    ) -> Result<(), CoreError> {
        let params = params
            .iter()
            .map(json_to_core)
            .collect::<Result<Vec<_>, _>>()?;
        let info = self
            .runtime
            .storage_entry(pallet, item)
            .ok_or_else(|| CoreError::NotInRuntime(format!("storage {pallet}.{item}")))?;
        let value = fill_defaults(&self.runtime, info.value_type, value)?;
        self.seed_core(pallet, item, &params, value)
    }

    fn seed_core(
        &mut self,
        pallet: &str,
        item: &str,
        params: &[CoreValue],
        value: CoreValue,
    ) -> Result<(), CoreError> {
        let info = self
            .runtime
            .storage_entry(pallet, item)
            .ok_or_else(|| CoreError::NotInRuntime(format!("storage {pallet}.{item}")))?;
        if params.len() != info.key_types.len() {
            return Err(CoreError::Codec(format!(
                "{pallet}.{item} requires {} key components, got {}",
                info.key_types.len(),
                params.len()
            )));
        }
        let key = self
            .runtime
            .storage_key(info, params)
            .map_err(CoreError::from)?;
        let value = self
            .runtime
            .encode_spec(&TypeSpec::Id(info.value_type), &value)
            .map_err(CoreError::from)?;
        self.seed_encoded(key, Some(value));
        Ok(())
    }

    /// Answer every map key of an item with one encoded Python-shaped value.
    pub fn seed_default(
        &mut self,
        pallet: &str,
        item: &str,
        value: Value,
    ) -> Result<(), CoreError> {
        let info = self
            .runtime
            .storage_entry(pallet, item)
            .ok_or_else(|| CoreError::NotInRuntime(format!("storage {pallet}.{item}")))?;
        let value = fill_defaults(&self.runtime, info.value_type, value)?;
        let bytes = self
            .runtime
            .encode_spec(&TypeSpec::Id(info.value_type), &value)
            .map_err(CoreError::from)?;
        self.storage_defaults.insert(storage_prefix(info), bytes);
        Ok(())
    }

    /// Encode runtime-API output for any encoded input to this method.
    pub fn seed_runtime(&mut self, api: &str, method: &str, value: Value) -> Result<(), CoreError> {
        let info = rtao_core::ops::runtime_api::runtime_api_method(&self.runtime, api, method)?;
        let value = fill_defaults(&self.runtime, info.output, value)?;
        let bytes = self
            .runtime
            .encode_spec(&TypeSpec::Id(info.output), &value)
            .map_err(CoreError::from)?;
        self.runtime_defaults
            .insert(format!("{api}_{method}"), bytes);
        Ok(())
    }

    /// Set an exact raw `state_call` response, keyed by method and SCALE input.
    pub fn seed_runtime_encoded(
        &mut self,
        method: impl Into<String>,
        input: Vec<u8>,
        output: Vec<u8>,
    ) {
        self.runtime_results.insert((method.into(), input), output);
    }

    /// Patch a constant in the served metadata and reparse the resulting runtime.
    pub fn seed_constant(
        &mut self,
        pallet: &str,
        name: &str,
        value: Value,
    ) -> Result<(), CoreError> {
        let constant = self
            .runtime
            .constant(pallet, name)
            .ok_or_else(|| CoreError::NotInRuntime(format!("constant {pallet}.{name}")))?;
        let value = fill_defaults(&self.runtime, constant.ty, value)?;
        let encoded = self
            .runtime
            .encode_spec(&TypeSpec::Id(constant.ty), &value)
            .map_err(CoreError::from)?;
        let mut metadata =
            RuntimeMetadataPrefixed::decode(&mut self.runtime.metadata_bytes.as_slice())
                .map_err(|e| CoreError::Codec(e.to_string()))?;
        let constants = match &mut metadata.1 {
            RuntimeMetadata::V15(metadata) => metadata
                .pallets
                .iter_mut()
                .find(|p| p.name == pallet)
                .map(|p| &mut p.constants),
            RuntimeMetadata::V14(metadata) => metadata
                .pallets
                .iter_mut()
                .find(|p| p.name == pallet)
                .map(|p| &mut p.constants),
            _ => {
                return Err(CoreError::Codec(
                    "fake constant patch requires V14 or V15".into(),
                ));
            }
        }
        .ok_or_else(|| CoreError::NotInRuntime(format!("pallet {pallet}")))?;
        let constant = constants
            .iter_mut()
            .find(|c| c.name == name)
            .ok_or_else(|| CoreError::NotInRuntime(format!("constant {pallet}.{name}")))?;
        constant.value = encoded;
        self.runtime = Arc::new(
            Runtime::parse(
                &metadata.encode(),
                self.runtime.spec_version,
                self.runtime.transaction_version,
                self.runtime.ss58_format,
            )
            .map_err(CoreError::from)?,
        );
        Ok(())
    }

    /// Encode System.Events records for the supplied block.
    pub fn seed_events(&mut self, hash: impl Into<String>, events: Value) -> Result<(), CoreError> {
        let info = self
            .runtime
            .storage_entry("System", "Events")
            .ok_or_else(|| CoreError::NotInRuntime("System.Events".into()))?;
        let events = fill_defaults(&self.runtime, info.value_type, events)?;
        let bytes = self
            .runtime
            .encode_spec(&TypeSpec::Id(info.value_type), &events)
            .map_err(CoreError::from)?;
        self.seed_at(hash, storage_prefix(info), Some(bytes));
        Ok(())
    }

    /// Associate different metadata and runtime versions with a historical block.
    pub fn set_runtime_at(&mut self, hash: impl Into<String>, runtime: Arc<Runtime>) {
        self.runtimes_at.insert(hash.into(), runtime);
    }

    /// Return the canonical hash, with the configured identity at height zero.
    pub fn block_hash(&self, number: u64) -> Option<String> {
        if number == 0 {
            Some(self.genesis_hash.clone())
        } else {
            self.canonical.get(&number).cloned()
        }
    }

    /// Return a Substrate JSON header for a known hash.
    pub fn header(&self, hash: &str) -> Option<Value> {
        if hash == self.genesis_hash {
            let mut raw = header_json(0, &zero_hash());
            raw["hash"] = Value::String(hash.to_owned());
            return Some(raw);
        }
        self.blocks.get(hash).map(|block| {
            let mut raw = header_json(
                block.number,
                if block.number == 1 {
                    &self.genesis_hash
                } else {
                    &block.parent
                },
            );
            raw["hash"] = Value::String(hash.to_owned());
            raw
        })
    }

    /// Advance best height while preserving the current finality lag.
    pub fn advance(&mut self, count: u64) -> Vec<Value> {
        let lag = self.block.saturating_sub(self.finalized);
        let mut headers = Vec::new();
        for _ in 0..count {
            if let Some(hash) = self.block_hash(self.block) {
                self.storage_at
                    .entry(hash)
                    .or_insert_with(|| self.storage.clone());
            }
            self.block = self.block.saturating_add(1);
            self.ensure_canonical(self.block);
            if let Some(hash) = self.block_hash(self.block) {
                self.storage_at
                    .entry(hash.clone())
                    .or_insert_with(|| self.storage.clone());
                if let Some(header) = self.header(&hash) {
                    headers.push(header);
                }
            }
        }
        self.finalized = self.block.saturating_sub(lag);
        headers
    }

    /// Set finality lag without moving the best head.
    pub fn set_finalized(&mut self, number: u64) -> Result<(), CoreError> {
        if number > self.block {
            return Err(CoreError::Codec(
                "finalized height exceeds best height".into(),
            ));
        }
        self.finalized = number;
        self.ensure_canonical(number);
        Ok(())
    }

    /// Create a noncanonical child block with an independently addressable hash.
    pub fn fork(&mut self, parent: &str) -> Result<String, CoreError> {
        let number = if parent == self.genesis_hash {
            1
        } else {
            self.blocks
                .get(parent)
                .ok_or_else(|| CoreError::Codec(format!("unknown fork parent {parent}")))?
                .number
                .saturating_add(1)
        };
        self.next_fork = self.next_fork.saturating_add(1);
        let hash = hex_bytes(&Blake2b::<U32>::digest(
            (parent, number, self.next_fork).encode(),
        ));
        self.blocks.insert(
            hash.clone(),
            Block {
                number,
                parent: parent.into(),
                extrinsics: Vec::new(),
            },
        );
        self.storage_at.insert(hash.clone(), self.storage.clone());
        Ok(hash)
    }

    /// Select an existing fork as best and update its canonical ancestors.
    ///
    /// A fork cannot replace a finalized block. Its storage snapshot becomes
    /// the active state, so subsequent seeds and reads follow that branch.
    pub fn set_best(&mut self, hash: &str) -> Result<(), CoreError> {
        let block = self
            .blocks
            .get(hash)
            .ok_or_else(|| CoreError::Codec(format!("unknown best hash {hash}")))?
            .clone();
        let mut ancestry = Vec::new();
        let mut current = hash.to_owned();
        loop {
            if current == self.genesis_hash {
                break;
            }
            let ancestor = self
                .blocks
                .get(&current)
                .ok_or_else(|| CoreError::Codec(format!("missing ancestor {current}")))?;
            if ancestor.number <= self.finalized {
                if self.block_hash(ancestor.number).as_deref() != Some(current.as_str()) {
                    return Err(CoreError::Codec("fork replaces finalized history".into()));
                }
                break;
            }
            ancestry.push((ancestor.number, current));
            current = ancestor.parent.clone();
        }
        if block.number < self.finalized {
            return Err(CoreError::Codec(
                "best height precedes finalized height".into(),
            ));
        }
        for (number, hash) in ancestry {
            self.canonical.insert(number, hash);
        }
        self.canonical.retain(|number, _| *number <= block.number);
        self.block = block.number;
        if let Some(storage) = self.storage_at.get(hash) {
            self.storage = storage.clone();
        }
        Ok(())
    }

    fn ensure_canonical(&mut self, number: u64) {
        for height in 1..=number {
            if self.canonical.contains_key(&height) {
                continue;
            }
            let hash = format!("0x{height:064x}");
            let parent = self
                .block_hash(height.saturating_sub(1))
                .unwrap_or_else(zero_hash);
            self.canonical.insert(height, hash.clone());
            self.blocks.insert(
                hash,
                Block {
                    number: height,
                    parent,
                    extrinsics: Vec::new(),
                },
            );
        }
    }

    /// Dispatch one raw RPC. Queued replies, handlers, and fixed results precede built-ins.
    ///
    /// A handler may delegate to `dispatch` for the ordinary response. That
    /// internal delegation records no second incoming request; separate calls
    /// from a transport remain separately recorded even when identical.
    pub fn dispatch(&mut self, request: &RpcRequest) -> Reply {
        if self.dispatching {
            return self.dispatch_inner(request);
        }
        self.requests.push(request.clone());
        self.dispatching = true;
        let reply = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            self.dispatch_inner(request)
        }));
        self.dispatching = false;
        match reply {
            Ok(reply) => reply,
            Err(panic) => std::panic::resume_unwind(panic),
        }
    }

    fn dispatch_inner(&mut self, request: &RpcRequest) -> Reply {
        if let Some(reply) = self
            .replies
            .get_mut(&request.method)
            .and_then(VecDeque::pop_front)
        {
            return reply;
        }
        if let Some(mut handler) = self.handlers.remove(&request.method) {
            let reply = handler(self, request);
            self.handlers
                .entry(request.method.clone())
                .or_insert(handler);
            return reply;
        }
        if let Some(value) = self.results.get(&request.method) {
            return Reply::result(value.clone());
        }
        match self.dispatch_builtin(request) {
            Ok(reply) => reply,
            Err(error) => Reply::error(-32602, error.to_string(), None),
        }
    }

    fn dispatch_builtin(&mut self, request: &RpcRequest) -> Result<Reply, CoreError> {
        let params = request
            .params
            .as_array()
            .ok_or_else(|| CoreError::Codec("RPC params must be an array".into()))?;
        let result = match request.method.as_str() {
            "system_chain" => json!("FakeChain"),
            "system_name" => json!("rtao-testkit"),
            "system_version" => json!(env!("CARGO_PKG_VERSION")),
            "system_properties" => {
                json!({"ss58Format": self.runtime.ss58_format, "tokenDecimals": 9, "tokenSymbol": "TAO"})
            }
            "rpc_methods" => {
                json!({"version": 1, "methods": ["system_chain", "system_name", "system_version", "system_properties", "system_health", "rpc_methods", "chain_getHead", "chain_getBlockHash", "chain_getHeader", "chain_getBlock", "chain_getFinalizedHead", "state_getMetadata", "state_getRuntimeVersion", "state_getStorage", "state_getStorageAt", "state_getStorageHash", "state_queryStorageAt", "state_getKeysPaged", "state_getKeys", "state_call", "chain_subscribeNewHeads", "chain_subscribeAllHeads", "chain_subscribeFinalizedHeads", "state_subscribeRuntimeVersion", "state_subscribeStorage", "chain_unsubscribeNewHeads", "chain_unsubscribeAllHeads", "chain_unsubscribeFinalizedHeads", "state_unsubscribeRuntimeVersion", "state_unsubscribeStorage", "author_submitExtrinsic", "author_submitAndWatchExtrinsic", "author_unwatchExtrinsic", "system_accountNextIndex", "account_nextIndex", "payment_queryInfo"]})
            }
            "system_health" => json!({"peers": 0, "isSyncing": false, "shouldHavePeers": false}),
            "chain_getBlockHash" => {
                let number = params
                    .first()
                    .filter(|v| !v.is_null())
                    .map(parse_number)
                    .transpose()?
                    .unwrap_or(self.block);
                self.block_hash(number)
                    .map(Value::String)
                    .unwrap_or(Value::Null)
            }
            "chain_getFinalizedHead" => json!(self.block_hash(self.finalized)),
            "chain_getHead" => json!(self.block_hash(self.block)),
            "chain_getHeader" => self
                .header(&self.hash_param(params.first())?)
                .unwrap_or(Value::Null),
            "chain_getBlock" => {
                let hash = self.hash_param(params.first())?;
                match self.blocks.get(&hash) {
                    Some(block) => {
                        json!({"block": {"header": self.header(&hash), "extrinsics": block.extrinsics.iter().map(|bytes| hex_bytes(bytes)).collect::<Vec<_>>()}, "justifications": null})
                    }
                    None if hash == self.genesis_hash => {
                        json!({"block": {"header": header_json(0, &zero_hash()), "extrinsics": []}, "justifications": null})
                    }
                    None => Value::Null,
                }
            }
            "state_getRuntimeVersion" => runtime_version(self.runtime_for(params.first())?),
            "state_getMetadata" => {
                json!(hex_bytes(&self.runtime_for(params.first())?.metadata_bytes))
            }
            "state_getStorage" | "state_getStorageAt" | "state_getStorageHash" => {
                let key = hex_param(params.first(), "storage key")?;
                let hash = self.optional_hash(params.get(1))?;
                self.storage_value(&key, hash.as_deref())
                    .map(|bytes| {
                        json!(if request.method == "state_getStorageHash" {
                            hex_bytes(&Blake2b::<U32>::digest(bytes))
                        } else {
                            hex_bytes(bytes)
                        })
                    })
                    .unwrap_or(Value::Null)
            }
            "state_getKeysPaged" | "state_getKeys" => {
                let prefix = params
                    .first()
                    .filter(|v| !v.is_null())
                    .map(|v| hex_param(Some(v), "storage prefix"))
                    .transpose()?
                    .unwrap_or_default();
                let (count, start, hash) = if request.method == "state_getKeysPaged" {
                    (
                        params
                            .get(1)
                            .map(parse_number)
                            .transpose()?
                            .unwrap_or(1_000),
                        params
                            .get(2)
                            .filter(|v| !v.is_null())
                            .map(|v| hex_param(Some(v), "start key"))
                            .transpose()?,
                        self.optional_hash(params.get(3))?,
                    )
                } else {
                    (u64::MAX, None, self.optional_hash(params.get(1))?)
                };
                let store = self.storage_for(hash.as_deref());
                Value::Array(
                    store
                        .iter()
                        .filter(|(key, value)| {
                            value.is_some()
                                && key.starts_with(&prefix)
                                && start.as_ref().is_none_or(|start| *key > start)
                        })
                        .take(usize::try_from(count).unwrap_or(usize::MAX))
                        .map(|(key, _)| json!(hex_bytes(key)))
                        .collect(),
                )
            }
            "state_queryStorageAt" => {
                let keys = params
                    .first()
                    .and_then(Value::as_array)
                    .ok_or_else(|| CoreError::Codec("storage keys must be an array".into()))?;
                let hash = self.hash_param(params.get(1))?;
                let changes = keys
                    .iter()
                    .map(|key| {
                        let bytes = hex_param(Some(key), "storage key")?;
                        Ok(json!([
                            hex_bytes(&bytes),
                            self.storage_value(&bytes, Some(&hash)).map(hex_bytes)
                        ]))
                    })
                    .collect::<Result<Vec<_>, CoreError>>()?;
                json!([{"block": hash, "changes": changes}])
            }
            "state_call" => self.state_call(params)?,
            "system_accountNextIndex" | "account_nextIndex" => {
                let address = str_param(params.first(), "address")?;
                json!(self.nonces.get(address).copied().unwrap_or(0))
            }
            "payment_queryInfo" => {
                json!({"weight": self.weight, "class": "normal", "partialFee": self.fee.to_string()})
            }
            "author_submitExtrinsic" | "author_submitAndWatchExtrinsic" => {
                return self.submit(params, request.method.ends_with("AndWatchExtrinsic"));
            }
            "chain_subscribeNewHeads"
            | "chain_subscribeAllHeads"
            | "chain_subscribeFinalizedHeads"
            | "state_subscribeRuntimeVersion"
            | "state_subscribeStorage" => return self.subscribe(request, params),
            "chain_unsubscribeNewHeads"
            | "chain_unsubscribeAllHeads"
            | "chain_unsubscribeFinalizedHeads"
            | "state_unsubscribeRuntimeVersion"
            | "state_unsubscribeStorage"
            | "author_unwatchExtrinsic" => json!(true),
            _ => {
                return Ok(Reply::error(
                    -32601,
                    format!("method {} not found", request.method),
                    None,
                ));
            }
        };
        Ok(Reply::result(result))
    }

    fn subscribe(&mut self, request: &RpcRequest, params: &[Value]) -> Result<Reply, CoreError> {
        self.next_subscription = self.next_subscription.saturating_add(1);
        let id = self.next_subscription.to_string();
        if let Some(script) = self
            .subscriptions
            .get_mut(&request.method)
            .and_then(VecDeque::pop_front)
        {
            return Ok(Reply::subscription(id, script.items, script.early));
        }
        let initial = match request.method.as_str() {
            "state_subscribeRuntimeVersion" => runtime_version(&self.runtime),
            "state_subscribeStorage" => {
                let keys = params.first().and_then(Value::as_array).ok_or_else(|| {
                    CoreError::Codec("storage subscription keys must be an array".into())
                })?;
                let changes = keys
                    .iter()
                    .map(|key| {
                        let bytes = hex_param(Some(key), "storage key")?;
                        Ok(json!([
                            hex_bytes(&bytes),
                            self.storage_value(&bytes, None).map(hex_bytes)
                        ]))
                    })
                    .collect::<Result<Vec<_>, CoreError>>()?;
                json!({"block": self.block_hash(self.block), "changes": changes})
            }
            _ => {
                let number = if request.method == "chain_subscribeFinalizedHeads" {
                    self.finalized
                } else {
                    self.block
                };
                self.block_hash(number)
                    .and_then(|h| self.header(&h))
                    .unwrap_or(Value::Null)
            }
        };
        Ok(Reply::subscription(id, vec![initial], false))
    }

    fn submit(&mut self, params: &[Value], watch: bool) -> Result<Reply, CoreError> {
        let bytes = hex_param(params.first(), "extrinsic")?;
        let hash = hex_bytes(&Blake2b::<U32>::digest(&bytes));
        self.submissions.push(bytes.clone());
        let script = self.submit_scripts.pop_front().unwrap_or(SubmitScript::InBlockThenFinalized {
            events: json!([{"phase": "ApplyExtrinsic", "extrinsic_idx": 0, "module_id": "System", "event_id": "ExtrinsicSuccess", "attributes": {"dispatch_info": {"weight": self.weight, "class": "Normal", "pays_fee": "Yes"}}, "topics": []}]),
        });
        let events = match script {
            SubmitScript::InBlockThenFinalized { events } => events,
            script => {
                let statuses = match script {
                    SubmitScript::Invalid(_) => vec![json!("invalid")],
                    SubmitScript::Usurped(hash) => vec![json!({"usurped": hash})],
                    SubmitScript::Dropped => vec![json!("dropped")],
                    SubmitScript::Retracted(hash) => vec![json!({"retracted": hash})],
                    SubmitScript::FinalityTimeout(hash) => vec![json!({"finalityTimeout": hash})],
                    SubmitScript::Statuses(statuses) => statuses,
                    SubmitScript::InBlockThenFinalized { .. } => Vec::new(),
                };
                // Retraction scripts can include the same extrinsic in distinct
                // fork blocks. Make every reported inclusion independently
                // queryable, but consume its account nonce only once.
                let included: BTreeSet<_> = statuses
                    .iter()
                    .filter_map(|status| {
                        status
                            .get("inBlock")
                            .or_else(|| status.get("finalized"))
                            .and_then(Value::as_str)
                    })
                    .collect();
                if !included.is_empty() {
                    self.consume_nonce(&bytes);
                }
                for block_hash in included {
                    if let Some(block) = self.blocks.get_mut(block_hash) {
                        block.extrinsics.push(bytes.clone());
                    }
                }
                if !watch {
                    return Ok(Reply::result(json!(hash)));
                }
                self.next_subscription = self.next_subscription.saturating_add(1);
                return Ok(Reply::subscription(
                    self.next_subscription.to_string(),
                    statuses,
                    false,
                ));
            }
        };
        self.consume_nonce(&bytes);
        self.advance(1);
        let block_hash = self
            .block_hash(self.block)
            .ok_or_else(|| CoreError::Codec("missing submission block".into()))?;
        if let Some(block) = self.blocks.get_mut(&block_hash) {
            block.extrinsics.push(bytes);
        }
        self.seed_events(block_hash.clone(), events)?;
        if !watch {
            return Ok(Reply::result(json!(hash)));
        }
        let statuses = vec![
            json!("ready"),
            json!({"inBlock": block_hash}),
            json!({"finalized": block_hash}),
        ];
        self.next_subscription = self.next_subscription.saturating_add(1);
        Ok(Reply::subscription(
            self.next_subscription.to_string(),
            statuses,
            false,
        ))
    }

    fn consume_nonce(&mut self, bytes: &[u8]) {
        // Decode real submitted bytes solely for observable account nonce state.
        // Arbitrary raw fixtures remain useful for transport-only tests.
        if let Ok(extrinsic) = self.runtime.decode_extrinsic(bytes, true)
            && let Some(address) = extrinsic.field("address").and_then(ValueExt::as_str)
        {
            let next = extrinsic
                .field("nonce")
                .and_then(ValueExt::as_u128)
                .and_then(|nonce| u64::try_from(nonce).ok())
                .unwrap_or_else(|| self.nonces.get(address).copied().unwrap_or(0))
                .saturating_add(1);
            self.nonces.insert(address.into(), next);
        }
    }

    fn state_call(&self, params: &[Value]) -> Result<Value, CoreError> {
        let method = str_param(params.first(), "runtime API method")?;
        let input = hex_param(params.get(1), "runtime API input")?;
        let runtime = self.runtime_for(params.get(2))?;
        if method == "Metadata_metadata_at_version" {
            if input.len() != 4 {
                return Err(CoreError::Codec(
                    "metadata version must be a SCALE u32".into(),
                ));
            }
            let requested =
                u32::from_le_bytes(input.as_slice().try_into().map_err(|_| {
                    CoreError::Codec("metadata version must be a SCALE u32".into())
                })?);
            let version = if runtime.is_v15 { 15 } else { 14 };
            return Ok(json!(hex_bytes(&if requested == version {
                Some(runtime.metadata_bytes.clone()).encode()
            } else {
                Option::<Vec<u8>>::None.encode()
            })));
        }
        if method == "Metadata_metadata_versions" {
            return Ok(json!(hex_bytes(
                &vec![if runtime.is_v15 { 15u32 } else { 14u32 }].encode()
            )));
        }
        if let Some(output) = self
            .runtime_results
            .get(&(method.into(), input.clone()))
            .or_else(|| self.runtime_defaults.get(method))
        {
            return Ok(json!(hex_bytes(output)));
        }
        if method == "TransactionPaymentApi_query_info" {
            let value = json!({"weight": self.weight, "class": "Normal", "partial_fee": self.fee});
            let info = rtao_core::ops::runtime_api::runtime_api_method(
                runtime,
                "TransactionPaymentApi",
                "query_info",
            )?;
            let value = fill_defaults(runtime, info.output, value)?;
            return Ok(json!(hex_bytes(
                &runtime
                    .encode_spec(&TypeSpec::Id(info.output), &value)
                    .map_err(CoreError::from)?
            )));
        }
        Err(CoreError::NotInRuntime(format!(
            "unseeded runtime API {method}"
        )))
    }

    fn runtime_for(&self, hash: Option<&Value>) -> Result<&Runtime, CoreError> {
        let hash = self.optional_hash(hash)?;
        Ok(hash
            .as_deref()
            .and_then(|h| self.runtimes_at.get(h))
            .unwrap_or(&self.runtime))
    }

    fn optional_hash(&self, hash: Option<&Value>) -> Result<Option<String>, CoreError> {
        hash.filter(|v| !v.is_null())
            .map(|v| {
                let hash = str_param(Some(v), "block hash")?;
                if crate::fixtures::decode_fixed_hex::<32>(hash).is_err() {
                    return Err(CoreError::Codec("block hash must contain 32 bytes".into()));
                }
                Ok(hash.into())
            })
            .transpose()
    }

    fn hash_param(&self, hash: Option<&Value>) -> Result<String, CoreError> {
        Ok(self
            .optional_hash(hash)?
            .unwrap_or_else(|| self.block_hash(self.block).unwrap_or_else(zero_hash)))
    }

    fn storage_for(&self, hash: Option<&str>) -> &Storage {
        let current = self.block_hash(self.block);
        if hash == current.as_deref() {
            &self.storage
        } else {
            hash.and_then(|h| self.storage_at.get(h))
                .unwrap_or(&self.storage)
        }
    }

    fn storage_value(&self, key: &[u8], hash: Option<&str>) -> Option<&[u8]> {
        let store = self.storage_for(hash);
        if let Some(value) = store.get(key) {
            return value.as_deref();
        }
        self.storage_defaults
            .iter()
            .find(|(prefix, _)| key.starts_with(prefix))
            .map(|(_, bytes)| bytes.as_slice())
    }

    /// Load the three resolved defaults directly from the pinned Rust extraction.
    ///
    /// Removed runtime entries remain in the public source tables. A scalar
    /// zero default for a sequence becomes the native empty sequence, as the
    /// chain cannot encode Python's permissive scalar sentinel for that item.
    pub fn load_python_defaults(&mut self) -> Result<(), CoreError> {
        let tables: Value = rtao_primitives::json::from_slice(include_bytes!(
            "../../../../vendor/upstream/extracted/named_tables.json"
        ))
        .map_err(|e| CoreError::Codec(e.to_string()))?;
        let sources = tables
            .get("tables")
            .ok_or_else(|| CoreError::Codec("missing extracted tables".into()))?;
        let values = sources
            .get("sdk/python/tests/harness/fake_substrate.py")
            .and_then(|v| v.get("values"))
            .ok_or_else(|| CoreError::Codec("missing extracted fake-substrate tables".into()))?;
        self.python_storage_defaults = extracted_table(values, "DEFAULT_STORAGE")?;
        self.python_constant_defaults = extracted_table(values, "DEFAULT_CONSTANTS")?;
        self.python_runtime_defaults = extracted_table(values, "DEFAULT_RUNTIME")?;
        for ((pallet, item), value) in self.python_storage_defaults.clone() {
            if let Some(info) = self.runtime.storage_entry(&pallet, &item) {
                let value = if value.as_u64() == Some(0)
                    && matches!(
                        self.runtime
                            .resolve(info.value_type)
                            .map_err(CoreError::from)?
                            .type_def,
                        TypeDef::Sequence(_)
                    ) {
                    json!([])
                } else {
                    value
                };
                self.seed_default(&pallet, &item, value)?;
            }
        }
        for ((pallet, item), value) in self.python_constant_defaults.clone() {
            if self.runtime.constant(&pallet, &item).is_some() {
                self.seed_constant(&pallet, &item, value)?;
            }
        }
        for ((api, method), value) in self.python_runtime_defaults.clone() {
            if rtao_core::ops::runtime_api::runtime_api_method(&self.runtime, &api, &method).is_ok()
            {
                self.seed_runtime(&api, &method, value)?;
            }
        }
        Ok(())
    }

    /// Load an oracle seed dump against the supplied metadata.
    ///
    /// `storage`, `storage_defaults`, `constants`, and `runtime` are arrays of
    /// objects containing `pallet`/`item` or `api`/`method` and `value`. Storage
    /// entries also contain `params`. Optional constructor fields are `fee`,
    /// `weight`, `block`, `finalized`, and an SS58-to-nonce `nonces` object.
    /// Unknown top-level fields and malformed entries fail before being used.
    pub fn from_python_seed(runtime: Arc<Runtime>, seed: &Value) -> Result<Self, CoreError> {
        let object = seed
            .as_object()
            .ok_or_else(|| CoreError::Codec("oracle seeds must be an object".into()))?;
        for key in object.keys() {
            if ![
                "storage",
                "storage_defaults",
                "constants",
                "runtime",
                "fee",
                "weight",
                "block",
                "finalized",
                "nonces",
            ]
            .contains(&key.as_str())
            {
                return Err(CoreError::Codec(format!("unknown oracle seed field {key}")));
            }
        }
        let mut state = Self::new(runtime);
        state.load_python_defaults()?;
        if let Some(value) = seed.get("fee") {
            state.fee = value
                .get("rao")
                .unwrap_or(value)
                .as_u64()
                .ok_or_else(|| CoreError::Codec("oracle fee must be a native u64".into()))?;
        }
        if let Some(value) = seed.get("weight") {
            state.weight = value.clone();
        }
        if let Some(value) = seed.get("block") {
            state.block = parse_number(value)?;
            state.ensure_canonical(state.block);
        }
        state.finalized = seed
            .get("finalized")
            .map(parse_number)
            .transpose()?
            .unwrap_or(state.block);
        if state.finalized > state.block {
            return Err(CoreError::Codec(
                "oracle finalized height exceeds best height".into(),
            ));
        }
        for section in ["storage", "storage_defaults", "constants", "runtime"] {
            let Some(entries) = seed.get(section) else {
                continue;
            };
            for entry in entries
                .as_array()
                .ok_or_else(|| CoreError::Codec(format!("oracle {section} must be an array")))?
            {
                let value = entry
                    .get("value")
                    .ok_or_else(|| CoreError::Codec(format!("oracle {section} entry lacks value")))?
                    .clone();
                let name = |field: &str| {
                    entry.get(field).and_then(Value::as_str).ok_or_else(|| {
                        CoreError::Codec(format!("oracle {section} entry lacks {field}"))
                    })
                };
                match section {
                    "storage" => state.seed_python(
                        name("pallet")?,
                        name("item")?,
                        match entry.get("params") {
                            None => &[],
                            Some(params) => {
                                params.as_array().map(Vec::as_slice).ok_or_else(|| {
                                    CoreError::Codec(
                                        "oracle storage params must be an array".into(),
                                    )
                                })?
                            }
                        },
                        value,
                    )?,
                    "storage_defaults" => {
                        state.seed_default(name("pallet")?, name("item")?, value)?
                    }
                    "constants" => state.seed_constant(name("pallet")?, name("item")?, value)?,
                    "runtime" => state.seed_runtime(name("api")?, name("method")?, value)?,
                    _ => {}
                }
            }
        }
        if let Some(nonces) = seed.get("nonces") {
            for (address, nonce) in nonces
                .as_object()
                .ok_or_else(|| CoreError::Codec("oracle nonces must be an object".into()))?
            {
                state.set_nonce(address, parse_number(nonce)?);
            }
        }
        Ok(state)
    }
}

fn zero_hash() -> String {
    format!("0x{}", "00".repeat(32))
}
fn empty_params() -> Value {
    json!([])
}
fn hex_bytes(bytes: &[u8]) -> String {
    format!("0x{}", hex::encode(bytes))
}
fn header_json(number: u64, parent: &str) -> Value {
    json!({"number": format!("0x{number:x}"), "parentHash": parent, "stateRoot": zero_hash(), "extrinsicsRoot": zero_hash(), "digest": {"logs": []}})
}
fn runtime_version(runtime: &Runtime) -> Value {
    let mut versions = BTreeMap::<&str, u32>::new();
    for surface in rtao_runtime::runtime_apis::ALL {
        if runtime.apis.iter().any(|api| {
            api.name == surface.api && api.methods.iter().any(|method| method.name == surface.name)
        }) {
            versions
                .entry(surface.api)
                .and_modify(|version| *version = (*version).max(surface.version))
                .or_insert(surface.version);
        }
    }
    let apis = versions
        .into_iter()
        .map(|(name, version)| {
            json!([
                format!("0x{}", hex::encode(Blake2b::<U8>::digest(name.as_bytes()))),
                version
            ])
        })
        .collect::<Vec<_>>();
    json!({"specName": "node-subtensor", "implName": "fake-subtensor", "authoringVersion": 1, "specVersion": runtime.spec_version, "implVersion": 1, "transactionVersion": runtime.transaction_version, "stateVersion": 1, "apis": apis})
}
fn str_param<'a>(value: Option<&'a Value>, name: &str) -> Result<&'a str, CoreError> {
    value
        .and_then(Value::as_str)
        .ok_or_else(|| CoreError::Codec(format!("{name} must be a string")))
}
fn hex_param(value: Option<&Value>, name: &str) -> Result<Vec<u8>, CoreError> {
    let value = str_param(value, name)?;
    crate::fixtures::decode_hex(value).map_err(|e| CoreError::Codec(e.to_string()))
}
fn parse_number(value: &Value) -> Result<u64, CoreError> {
    if let Some(number) = value.as_u64() {
        return Ok(number);
    }
    if let Some(number) = value.as_str() {
        return if let Some(digits) = number.strip_prefix("0x") {
            u64::from_str_radix(digits, 16)
        } else {
            number.parse()
        }
        .map_err(|e| CoreError::Codec(format!("invalid block number: {e}")));
    }
    Err(CoreError::Codec(
        "block number must be a u64 or hex string".into(),
    ))
}

fn extracted_table(
    values: &Value,
    name: &str,
) -> Result<BTreeMap<(String, String), Value>, CoreError> {
    let entries = values
        .get(name)
        .and_then(|v| v.get("value"))
        .and_then(|v| v.get("dict"))
        .and_then(Value::as_array)
        .ok_or_else(|| CoreError::Codec(format!("missing extracted {name}")))?;
    let mut result = BTreeMap::new();
    for entry in entries {
        let pair = entry
            .as_array()
            .filter(|p| p.len() == 2)
            .ok_or_else(|| CoreError::Codec(format!("invalid extracted {name} pair")))?;
        let key = pair
            .first()
            .and_then(|v| v.get("tuple"))
            .and_then(Value::as_array)
            .filter(|p| p.len() == 2)
            .ok_or_else(|| CoreError::Codec(format!("invalid extracted {name} key")))?;
        let first = str_param(key.first(), "table key")?;
        let second = str_param(key.get(1), "table key")?;
        let value = pair
            .get(1)
            .ok_or_else(|| CoreError::Codec("missing table value".into()))?;
        result.insert((first.into(), second.into()), extracted_value(value)?);
    }
    Ok(result)
}

fn extracted_value(value: &Value) -> Result<Value, CoreError> {
    if let Some(dict) = value.get("dict").and_then(Value::as_array) {
        let mut object = serde_json::Map::new();
        for pair in dict {
            let pair = pair
                .as_array()
                .filter(|p| p.len() == 2)
                .ok_or_else(|| CoreError::Codec("invalid extracted dictionary pair".into()))?;
            object.insert(
                str_param(pair.first(), "dictionary key")?.into(),
                extracted_value(
                    pair.get(1)
                        .ok_or_else(|| CoreError::Codec("missing dictionary value".into()))?,
                )?,
            );
        }
        return Ok(Value::Object(object));
    }
    if let Some(items) = value.get("tuple").and_then(Value::as_array) {
        return items
            .iter()
            .map(extracted_value)
            .collect::<Result<Vec<_>, _>>()
            .map(Value::Array);
    }
    if let Some(items) = value.as_array() {
        return items
            .iter()
            .map(extracted_value)
            .collect::<Result<Vec<_>, _>>()
            .map(Value::Array);
    }
    Ok(value.clone())
}

/// Convert Python-shaped JSON into the core's lenient input carrier.
///
/// Integer precision is retained through the full signed/unsigned 128-bit
/// range and the unsigned 256-bit range. Floats are rejected because SCALE
/// numeric inputs are integral.
pub fn json_to_core(value: &Value) -> Result<CoreValue, CoreError> {
    match value {
        Value::Null => Ok(CoreValue::Null),
        Value::Bool(value) => Ok(CoreValue::Bool(*value)),
        Value::String(value) => Ok(CoreValue::Str(value.clone())),
        Value::Number(number) => match number
            .as_i128()
            .map(CoreValue::Int)
            .or_else(|| number.as_u128().map(CoreValue::Uint))
        {
            Some(value) => Ok(value),
            None => decimal_u256(&number.to_string()).map(CoreValue::U256),
        },
        Value::Array(items) => items
            .iter()
            .map(json_to_core)
            .collect::<Result<Vec<_>, _>>()
            .map(CoreValue::List),
        Value::Object(object) => object
            .iter()
            .map(|(name, value)| Ok((CoreValue::Str(name.clone()), json_to_core(value)?)))
            .collect::<Result<Vec<_>, _>>()
            .map(CoreValue::Dict),
    }
}

fn decimal_u256(digits: &str) -> Result<[u8; 32], CoreError> {
    if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return Err(CoreError::Codec(
            "seed number must be an integer within 256 bits".into(),
        ));
    }
    let mut bytes = [0u8; 32];
    for digit in digits.bytes() {
        let mut carry = u16::from(digit - b'0');
        for byte in &mut bytes {
            let value = u16::from(*byte) * 10 + carry;
            *byte = u8::try_from(value & 0xff)
                .map_err(|_| CoreError::Codec("invalid u256 byte".into()))?;
            carry = value >> 8;
        }
        if carry != 0 {
            return Err(CoreError::Codec("seed integer exceeds u256".into()));
        }
    }
    Ok(bytes)
}

/// Complete absent composite fields using the portable registry, then retain
/// the core's account, byte, compact and enum coercions for supplied values.
pub fn fill_defaults(
    runtime: &Runtime,
    type_id: u32,
    value: Value,
) -> Result<CoreValue, CoreError> {
    fill(runtime, type_id, Some(&value), 0)
}

fn fill(
    runtime: &Runtime,
    id: u32,
    value: Option<&Value>,
    depth: usize,
) -> Result<CoreValue, CoreError> {
    if depth > 128 {
        return Err(CoreError::Codec(
            "seed default recursion exceeds 128 levels".into(),
        ));
    }
    let ty = runtime.resolve(id).map_err(CoreError::from)?;
    let last = ty.path.segments.last().map(String::as_str);
    if last == Some("AccountId32") {
        return value
            .map(json_to_core)
            .unwrap_or_else(|| Ok(CoreValue::Bytes(vec![0; 32])));
    }
    if last == Some("MultiAddress") && value.is_some_and(Value::is_string) {
        return json_to_core(value.ok_or_else(|| CoreError::Codec("missing address".into()))?);
    }
    if last == Some("Era") && ty.path.segments.first().map(String::as_str) == Some("sp_runtime") {
        return value
            .map(json_to_core)
            .unwrap_or_else(|| Ok(CoreValue::Str("00".into())));
    }
    if last == Some("BTreeMap") && value.is_none() {
        return Ok(CoreValue::Dict(Vec::new()));
    }
    if Some(id) == runtime.outer_event_type
        && let Some(value) = value.filter(|v| v.get("module_id").is_some())
    {
        let pallet = str_param(value.get("module_id"), "event module_id")?;
        let name = str_param(value.get("event_id"), "event event_id")?;
        let mut event = serde_json::Map::new();
        event.insert(
            name.into(),
            value.get("attributes").cloned().unwrap_or(Value::Null),
        );
        let mut outer = serde_json::Map::new();
        outer.insert(pallet.into(), Value::Object(event));
        return fill(
            runtime,
            id,
            Some(&Value::Object(outer)),
            depth.saturating_add(1),
        );
    }
    if last == Some("EventRecord")
        && let Some(Value::Object(object)) = value
    {
        let mut object = object.clone();
        if object.get("phase").and_then(Value::as_str) == Some("ApplyExtrinsic") {
            object.insert(
                "phase".into(),
                json!({"ApplyExtrinsic": object.get("extrinsic_idx").cloned().unwrap_or(json!(0))}),
            );
        }
        if !object.contains_key("event") && object.contains_key("module_id") {
            object.insert("event".into(), json!({"module_id": object.get("module_id"), "event_id": object.get("event_id"), "attributes": object.get("attributes")}));
        }
        if let TypeDef::Composite(composite) = &ty.type_def {
            let fields = composite
                .fields
                .iter()
                .map(|field| {
                    let name = field
                        .name
                        .as_deref()
                        .ok_or_else(|| CoreError::Codec("unnamed event-record field".into()))?;
                    Ok((
                        CoreValue::Str(name.into()),
                        fill(
                            runtime,
                            field.ty.id,
                            object.get(name),
                            depth.saturating_add(1),
                        )?,
                    ))
                })
                .collect::<Result<Vec<_>, CoreError>>()?;
            return Ok(CoreValue::Dict(fields));
        }
    }
    match &ty.type_def {
        TypeDef::Composite(composite) => {
            if composite.fields.is_empty() {
                return Ok(CoreValue::Null);
            }
            if composite.fields.iter().all(|f| f.name.is_some()) {
                if value.is_some_and(|v| !v.is_object()) {
                    return json_to_core(
                        value.ok_or_else(|| CoreError::Codec("missing composite value".into()))?,
                    );
                }
                let fields = composite
                    .fields
                    .iter()
                    .map(|field| {
                        let name = field.name.as_deref().ok_or_else(|| {
                            CoreError::Codec("unnamed field in named composite".into())
                        })?;
                        Ok((
                            CoreValue::Str(name.into()),
                            fill(
                                runtime,
                                field.ty.id,
                                value.and_then(|v| v.get(name)),
                                depth.saturating_add(1),
                            )?,
                        ))
                    })
                    .collect::<Result<Vec<_>, CoreError>>()?;
                Ok(CoreValue::Dict(fields))
            } else if composite.fields.len() == 1 {
                let field = composite
                    .fields
                    .first()
                    .ok_or_else(|| CoreError::Codec("missing newtype field".into()))?;
                fill(runtime, field.ty.id, value, depth.saturating_add(1))
            } else {
                if let Some(value) = value {
                    let items = value.as_array().ok_or_else(|| {
                        CoreError::Codec("unnamed composite seed must be an array".into())
                    })?;
                    if items.len() != composite.fields.len() {
                        return Err(CoreError::Codec(
                            "unnamed composite seed has wrong field count".into(),
                        ));
                    }
                }
                composite
                    .fields
                    .iter()
                    .enumerate()
                    .map(|(i, field)| {
                        fill(
                            runtime,
                            field.ty.id,
                            value.and_then(|v| v.get(i)),
                            depth.saturating_add(1),
                        )
                    })
                    .collect::<Result<Vec<_>, _>>()
                    .map(CoreValue::Tuple)
            }
        }
        TypeDef::Tuple(tuple) => {
            if let Some(value) = value {
                let items = value
                    .as_array()
                    .ok_or_else(|| CoreError::Codec("tuple seed must be an array".into()))?;
                if items.len() != tuple.fields.len() {
                    return Err(CoreError::Codec("tuple seed has wrong field count".into()));
                }
            }
            tuple
                .fields
                .iter()
                .enumerate()
                .map(|(i, field)| {
                    fill(
                        runtime,
                        field.id,
                        value.and_then(|v| v.get(i)),
                        depth.saturating_add(1),
                    )
                })
                .collect::<Result<Vec<_>, _>>()
                .map(CoreValue::Tuple)
        }
        TypeDef::Array(array) => {
            if let Some(Value::String(_)) = value {
                return json_to_core(
                    value.ok_or_else(|| CoreError::Codec("missing array value".into()))?,
                );
            }
            if value.is_some_and(|v| !v.is_array()) {
                return json_to_core(
                    value.ok_or_else(|| CoreError::Codec("missing array value".into()))?,
                );
            }
            let length = usize::try_from(array.len)
                .map_err(|_| CoreError::Codec("array length exceeds usize".into()))?;
            if let Some(items) = value.and_then(Value::as_array)
                && items.len() != length
            {
                return Err(CoreError::Codec(format!(
                    "seed array needs {length} elements"
                )));
            }
            (0..length)
                .map(|i| {
                    fill(
                        runtime,
                        array.type_param.id,
                        value.and_then(|v| v.get(i)),
                        depth.saturating_add(1),
                    )
                })
                .collect::<Result<Vec<_>, _>>()
                .map(CoreValue::List)
        }
        TypeDef::Sequence(sequence) => match value {
            Some(Value::Array(items)) => items
                .iter()
                .map(|item| {
                    fill(
                        runtime,
                        sequence.type_param.id,
                        Some(item),
                        depth.saturating_add(1),
                    )
                })
                .collect::<Result<Vec<_>, _>>()
                .map(CoreValue::List),
            Some(value) => json_to_core(value),
            None => Ok(CoreValue::List(Vec::new())),
        },
        TypeDef::Variant(variants) if last == Some("Option") => match value {
            None | Some(Value::Null) => Ok(CoreValue::Null),
            Some(value) => {
                let some = variants
                    .variants
                    .iter()
                    .find(|v| v.name == "Some")
                    .and_then(|v| v.fields.first())
                    .ok_or_else(|| CoreError::Codec("Option lacks Some field".into()))?;
                fill(runtime, some.ty.id, Some(value), depth.saturating_add(1))
            }
        },
        TypeDef::Variant(variants) => {
            let (chosen, payload) = match value {
                Some(Value::String(name)) => {
                    (variants.variants.iter().find(|v| &v.name == name), None)
                }
                Some(Value::Object(object)) if object.len() == 1 => {
                    let (name, payload) = object
                        .iter()
                        .next()
                        .ok_or_else(|| CoreError::Codec("empty variant object".into()))?;
                    (
                        variants.variants.iter().find(|v| &v.name == name),
                        Some(payload),
                    )
                }
                None => (variants.variants.iter().min_by_key(|v| v.index), None),
                Some(value) => return json_to_core(value),
            };
            let chosen = chosen.ok_or_else(|| {
                CoreError::Codec(format!("seed selects no inhabited variant for type {id}"))
            })?;
            if chosen.fields.is_empty() {
                return Ok(CoreValue::Str(chosen.name.clone()));
            }
            let filled = if chosen.fields.iter().all(|f| f.name.is_some()) {
                chosen
                    .fields
                    .iter()
                    .map(|field| {
                        let name = field
                            .name
                            .as_deref()
                            .ok_or_else(|| CoreError::Codec("unnamed variant field".into()))?;
                        Ok((
                            CoreValue::Str(name.into()),
                            fill(
                                runtime,
                                field.ty.id,
                                payload.and_then(|v| v.get(name)),
                                depth.saturating_add(1),
                            )?,
                        ))
                    })
                    .collect::<Result<Vec<_>, CoreError>>()
                    .map(CoreValue::Dict)?
            } else if chosen.fields.len() == 1 {
                fill(
                    runtime,
                    chosen
                        .fields
                        .first()
                        .ok_or_else(|| CoreError::Codec("missing variant field".into()))?
                        .ty
                        .id,
                    payload,
                    depth.saturating_add(1),
                )?
            } else {
                chosen
                    .fields
                    .iter()
                    .enumerate()
                    .map(|(i, field)| {
                        fill(
                            runtime,
                            field.ty.id,
                            payload.and_then(|v| v.get(i)),
                            depth.saturating_add(1),
                        )
                    })
                    .collect::<Result<Vec<_>, _>>()
                    .map(CoreValue::Tuple)?
            };
            Ok(CoreValue::Dict(vec![(
                CoreValue::Str(chosen.name.clone()),
                filled,
            )]))
        }
        TypeDef::Compact(compact) => fill(
            runtime,
            compact.type_param.id,
            value,
            depth.saturating_add(1),
        ),
        TypeDef::Primitive(primitive) => value.map(json_to_core).unwrap_or_else(|| {
            Ok(match primitive {
                TypeDefPrimitive::Bool => CoreValue::Bool(false),
                TypeDefPrimitive::Str => CoreValue::Str(String::new()),
                TypeDefPrimitive::Char => CoreValue::Str("\0".into()),
                TypeDefPrimitive::U256 | TypeDefPrimitive::I256 => CoreValue::U256([0; 32]),
                _ => CoreValue::Int(0),
            })
        }),
        TypeDef::BitSequence(_) => value
            .map(json_to_core)
            .unwrap_or_else(|| Ok(CoreValue::List(Vec::new()))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn call(state: &mut FakeState, method: &str, params: Value) -> Result<Value, CoreError> {
        match state
            .dispatch(&RpcRequest {
                id: json!(1),
                method: method.into(),
                params,
            })
            .outcome
        {
            Some(Ok(value)) => Ok(value),
            Some(Err(error)) => Err(CoreError::Rpc(error.message)),
            None => Err(CoreError::Rpc("unexpected silent reply".into())),
        }
    }

    #[test]
    fn extracted_defaults_encode_through_pinned_metadata() -> Result<(), CoreError> {
        let mut state = FakeState::try_default()?;
        assert_eq!(state.python_storage_defaults.len(), 22);
        assert_eq!(state.python_constant_defaults.len(), 3);
        assert_eq!(state.python_runtime_defaults.len(), 3);
        let info = state
            .runtime
            .storage_entry("SubtensorModule", "Tempo")
            .ok_or_else(|| CoreError::NotInRuntime("Tempo".into()))?;
        let key = state
            .runtime
            .storage_key(info, &[CoreValue::Int(1)])
            .map_err(CoreError::from)?;
        assert_eq!(
            call(&mut state, "state_getStorage", json!([hex_bytes(&key)]))?,
            json!(hex_bytes(&360u16.encode()))
        );
        let slot = state
            .runtime
            .constant("Aura", "SlotDuration")
            .ok_or_else(|| CoreError::NotInRuntime("Aura.SlotDuration".into()))?;
        assert_eq!(slot.value, 12_000u64.encode());
        state.seed(
            &rtao_runtime::storage::subtensor_module::TEMPO,
            (rtao_primitives::NetUid(1),),
            7u16,
        )?;
        assert_eq!(
            call(&mut state, "state_getStorage", json!([hex_bytes(&key)]))?,
            json!(hex_bytes(&7u16.encode()))
        );
        Ok(())
    }

    #[test]
    fn partial_account_seeds_fill_nested_fields_and_keep_ss58_keys() -> Result<(), CoreError> {
        let mut state = FakeState::try_default()?;
        let address = rtao_core::keys::ss58_from_public([7; 32], 42);
        state.seed_python(
            "System",
            "Account",
            &[json!(address)],
            json!({"data": {"free": 91}}),
        )?;
        let info = state
            .runtime
            .storage_entry("System", "Account")
            .ok_or_else(|| CoreError::NotInRuntime("Account".into()))?;
        let key = state
            .runtime
            .storage_key(info, &[CoreValue::Bytes(vec![7; 32])])
            .map_err(CoreError::from)?;
        let value = rtao_core::ops::storage::decode_storage_bytes(
            &state.runtime,
            info,
            state.storage_value(&key, None),
        )?;
        let value = rtao_core::DynValue::new(value).to_json();
        assert_eq!(value.get("nonce"), Some(&json!(0)));
        assert_eq!(value.pointer("/data/free"), Some(&json!(91)));
        assert_eq!(value.pointer("/data/reserved"), Some(&json!(0)));
        assert_eq!(value.pointer("/data/frozen"), Some(&json!(0)));
        Ok(())
    }

    #[test]
    fn storage_history_preserves_misses_and_pagination_is_exclusive() -> Result<(), CoreError> {
        let mut state = FakeState::try_default()?;
        state.seed_encoded(vec![1, 0], vec![7]);
        let original = state
            .block_hash(state.block)
            .ok_or_else(|| CoreError::Codec("missing head".into()))?;
        state.advance(1);
        state.seed_encoded(vec![1, 0], vec![8]);
        state.seed_encoded(vec![1, 1], vec![9]);
        assert_eq!(
            call(&mut state, "state_getStorage", json!(["0x0100", original]))?,
            json!("0x07")
        );
        assert_eq!(
            call(&mut state, "state_getStorage", json!(["0x0101", original]))?,
            Value::Null
        );
        assert_eq!(
            call(&mut state, "state_getKeysPaged", json!(["0x01", 1, null]))?,
            json!(["0x0100"])
        );
        assert_eq!(
            call(
                &mut state,
                "state_getKeysPaged",
                json!(["0x01", 5, "0x0100"])
            )?,
            json!(["0x0101"])
        );
        state.seed_encoded(vec![1, 0], None);
        assert_eq!(
            call(&mut state, "state_getStorage", json!(["0x0100"]))?,
            Value::Null
        );
        Ok(())
    }

    #[test]
    fn metadata_rpc_has_real_scale_option_framing_and_versions() -> Result<(), CoreError> {
        let mut state = FakeState::try_default()?;
        let metadata = call(
            &mut state,
            "state_call",
            json!(["Metadata_metadata_at_version", "0x0f000000"]),
        )?;
        assert_eq!(
            rtao_core::ops::metadata::decode_v15(&metadata)?,
            Some(state.runtime.metadata_bytes.clone())
        );
        let missing = call(
            &mut state,
            "state_call",
            json!(["Metadata_metadata_at_version", "0x10000000"]),
        )?;
        assert_eq!(rtao_core::ops::metadata::decode_v15(&missing)?, None);
        let mut upgraded =
            Runtime::parse(&state.runtime.metadata_bytes, 479, 2, 42).map_err(CoreError::from)?;
        upgraded.spec_version = 479;
        let hash = state
            .block_hash(98)
            .ok_or_else(|| CoreError::Codec("missing block".into()))?;
        state.set_runtime_at(hash.clone(), Arc::new(upgraded));
        assert_eq!(
            call(&mut state, "state_getRuntimeVersion", json!([hash]))?.get("specVersion"),
            Some(&json!(479))
        );
        assert_eq!(
            call(&mut state, "state_getRuntimeVersion", json!([]))?.get("specVersion"),
            Some(&json!(478))
        );
        Ok(())
    }

    #[test]
    fn head_advancement_keeps_finality_lag_and_forks_have_distinct_hashes() -> Result<(), CoreError>
    {
        let mut state = FakeState::try_default()?;
        state.set_finalized(97)?;
        let headers = state.advance(3);
        assert_eq!(headers.len(), 3);
        assert_eq!((state.block, state.finalized), (103, 100));
        let parent = state
            .block_hash(102)
            .ok_or_else(|| CoreError::Codec("missing parent".into()))?;
        let fork = state.fork(&parent)?;
        assert_ne!(Some(fork.clone()), state.block_hash(103));
        assert_eq!(
            state
                .header(&fork)
                .and_then(|h| h.get("parentHash").cloned()),
            Some(json!(parent))
        );
        state.set_best(&fork)?;
        assert_eq!(state.block_hash(103), Some(fork));
        let finalized = state
            .block_hash(99)
            .ok_or_else(|| CoreError::Codec("missing history".into()))?;
        let forbidden = state.fork(&finalized)?;
        assert!(state.set_best(&forbidden).is_err());
        assert!(state.set_finalized(104).is_err());
        Ok(())
    }

    #[test]
    fn handlers_mutate_state_and_scripts_are_fifo() -> Result<(), CoreError> {
        let mut state = FakeState::try_default()?;
        state.set_handler("custom", |state, _| {
            state.fee += 1;
            Reply::result(json!(state.fee))
        });
        state.queue_reply("custom", Reply::result(json!("first")));
        assert_eq!(call(&mut state, "custom", json!([]))?, json!("first"));
        assert_eq!(call(&mut state, "custom", json!([]))?, json!(124_415));
        assert_eq!(call(&mut state, "custom", json!([]))?, json!(124_416));
        assert_eq!(state.requests.len(), 3);
        let malformed: RpcRequest = serde_json::from_value(json!({"id": 2, "method": "custom"}))
            .map_err(|e| CoreError::Codec(e.to_string()))?;
        assert_eq!(malformed.params, json!([]));
        Ok(())
    }

    #[test]
    fn delegated_author_handlers_preserve_one_request_per_transport_call_and_identical_retries()
    -> Result<(), CoreError> {
        let mut state = FakeState::try_default()?;
        state.set_handler("author_submitExtrinsic", |state, request| {
            state.dispatch(request)
        });
        let request = RpcRequest {
            id: json!(1),
            method: "author_submitExtrinsic".into(),
            params: json!(["0x010203"]),
        };
        let first = state.dispatch(&request);
        assert!(matches!(&first.outcome, Some(Ok(_))));
        assert_eq!(state.requests, [request.clone()]);
        assert_eq!(state.submissions, [vec![1, 2, 3]]);

        // An actual second incoming call must never disappear through
        // deduplication, even if the JSON-RPC ID and signed bytes are identical.
        let second = state.dispatch(&request);
        assert_eq!(second.outcome, first.outcome);
        assert_eq!(state.requests, [request.clone(), request]);
        assert_eq!(state.submissions, [vec![1, 2, 3], vec![1, 2, 3]]);
        Ok(())
    }

    #[test]
    #[allow(clippy::panic)] // The recovered injected panic must not suppress later requests.
    fn panicking_handlers_do_not_disable_transport_request_recording() -> Result<(), CoreError> {
        let mut state = FakeState::try_default()?;
        state.set_handler("system_health", |_, _| panic!("injected handler failure"));
        let request = RpcRequest {
            id: json!(1),
            method: "system_health".into(),
            params: json!([]),
        };
        assert!(
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                state.dispatch(&request)
            }))
            .is_err()
        );
        let next = state.dispatch(&request);
        assert!(matches!(next.outcome, Some(Ok(_))));
        assert_eq!(state.requests, [request.clone(), request]);
        Ok(())
    }

    #[test]
    fn fee_runtime_api_decodes_using_metadata() -> Result<(), CoreError> {
        let mut state = FakeState::try_default()?;
        state.fee = 999;
        let encoded = call(
            &mut state,
            "state_call",
            json!(["TransactionPaymentApi_query_info", "0x00"]),
        )?;
        let info = rtao_core::ops::runtime_api::runtime_api_method(
            &state.runtime,
            "TransactionPaymentApi",
            "query_info",
        )?;
        let decoded = rtao_core::DynValue::new(rtao_core::ops::runtime_api::decode_result(
            &state.runtime,
            info,
            &encoded,
        )?)
        .to_json();
        assert_eq!(decoded.get("partial_fee"), Some(&json!(999)));
        assert_eq!(decoded.pointer("/weight/ref_time"), Some(&json!(1_000_000)));
        Ok(())
    }

    #[test]
    fn flattened_events_reencode_to_exact_golden_bytes() -> Result<(), CoreError> {
        let golden = Golden::from_slice(include_bytes!("../../../../vendor/upstream/golden.json"))
            .map_err(|e| CoreError::Codec(e.to_string()))?;
        let mut state = FakeState::from_golden(&golden)?;
        let events = golden
            .sections
            .get("events")
            .ok_or_else(|| CoreError::Codec("missing events fixture".into()))?;
        let hash = str_param(events.get("block_hash"), "event hash")?;
        state.seed_events(
            hash,
            events
                .get("decoded")
                .ok_or_else(|| CoreError::Codec("missing decoded events".into()))?
                .clone(),
        )?;
        let info = state
            .runtime
            .storage_entry("System", "Events")
            .ok_or_else(|| CoreError::NotInRuntime("System.Events".into()))?;
        let actual = hex_bytes(
            state
                .storage_value(&storage_prefix(info), Some(hash))
                .ok_or_else(|| CoreError::Codec("events seed missing".into()))?,
        );
        assert_eq!(
            Some(actual.as_str()),
            events.get("raw_hex").and_then(Value::as_str)
        );
        Ok(())
    }

    #[test]
    fn submit_lifecycle_records_scale_bytes_and_can_notify_before_ack() -> Result<(), CoreError> {
        let mut state = FakeState::try_default()?;
        let reply = state.dispatch(&RpcRequest {
            id: json!(1),
            method: "author_submitAndWatchExtrinsic".into(),
            params: json!(["0x010203"]),
        });
        assert_eq!(state.submissions, vec![vec![1, 2, 3]]);
        assert_eq!(reply.notifications.len(), 3);
        assert_eq!(
            reply.notifications.first().map(|n| &n.result),
            Some(&json!("ready"))
        );
        state.queue_subscription(
            "chain_subscribeNewHeads",
            SubscriptionScript {
                items: vec![json!(1), json!(2)],
                early: true,
            },
        );
        let reply = state.dispatch(&RpcRequest {
            id: json!(2),
            method: "chain_subscribeNewHeads".into(),
            params: json!([]),
        });
        assert!(reply.early);
        assert_eq!(reply.notifications.len(), 2);
        let included_height = state.block;
        state.queue_submit(SubmitScript::Invalid("invalid proof".into()));
        let rejected = state.dispatch(&RpcRequest {
            id: json!(3),
            method: "author_submitAndWatchExtrinsic".into(),
            params: json!(["0x010203"]),
        });
        assert_eq!(state.block, included_height);
        assert_eq!(
            rejected.notifications.first().map(|n| &n.result),
            Some(&json!("invalid"))
        );
        Ok(())
    }

    #[test]
    fn oracle_seeds_share_encoding_and_reject_malformed_native_balances() -> Result<(), CoreError> {
        let runtime = FakeState::try_default()?.runtime;
        let state = FakeState::from_python_seed(
            runtime.clone(),
            &json!({"fee": 45, "block": 102, "storage": [{"pallet": "SubtensorModule", "item": "Tempo", "params": [1], "value": 7}], "nonces": {"alice": 5}}),
        )?;
        assert_eq!((state.fee, state.block, state.finalized), (45, 102, 102));
        assert_eq!(state.nonces.get("alice"), Some(&5));
        assert!(FakeState::from_python_seed(runtime.clone(), &json!({"fee": -1})).is_err());
        assert!(FakeState::from_python_seed(runtime, &json!({"typo": []})).is_err());
        Ok(())
    }

    #[test]
    fn seeds_preserve_full_width_integers_and_enforce_chain_bounds() -> Result<(), CoreError> {
        let value: Value = serde_json::from_str(
            "115792089237316195423570985008687907853269984665640564039457584007913129639935",
        )
        .map_err(|e| CoreError::Codec(e.to_string()))?;
        assert_eq!(json_to_core(&value)?, CoreValue::U256([255; 32]));
        let overflow: Value = serde_json::from_str(
            "115792089237316195423570985008687907853269984665640564039457584007913129639936",
        )
        .map_err(|e| CoreError::Codec(e.to_string()))?;
        assert!(json_to_core(&overflow).is_err());
        assert!(json_to_core(&json!(1.5)).is_err());
        let mut state = FakeState::try_default()?;
        assert!(
            state
                .seed_python("SubtensorModule", "Tempo", &[json!(1)], json!(65_536))
                .is_err()
        );
        assert!(
            state
                .seed_python("SubtensorModule", "Tempo", &[], json!(7))
                .is_err()
        );
        assert!(
            state
                .seed_python("System", "Account", &[json!("invalid ss58")], json!({}))
                .is_err()
        );
        Ok(())
    }
}
