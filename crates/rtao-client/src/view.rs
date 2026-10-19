// Derived from RaoFoundation/subtensor sdk/python/bittensor/_transport/interface.py @ 850f2755bb05a3851f948489f0ab27b0645cebce (Apache-2.0); modified by rtao.
// Derived from RaoFoundation/subtensor sdk/python/bittensor/snapshot.py @ 850f2755bb05a3851f948489f0ab27b0645cebce (Apache-2.0); modified by rtao.
// Derived from RaoFoundation/subtensor sdk/python/bittensor/_transport/storage.py @ 850f2755bb05a3851f948489f0ab27b0645cebce (Apache-2.0); modified by rtao.
// Derived from RaoFoundation/subtensor sdk/python/bittensor/_substrate.py @ 850f2755bb05a3851f948489f0ab27b0645cebce (Apache-2.0); modified by rtao.
//! Read views whose multi-request operations retain one block identity.

use std::{
    collections::{BTreeMap, BTreeSet},
    future::Future,
    sync::Arc,
    time::{Duration, SystemTime},
};

use bytes::Bytes;
use futures::{Stream, future::BoxFuture};
use rtao_core::{CoreError, DynValue, codec::Value};
use rtao_primitives::{H256, IntoCoreValue};
use rtao_rpc::ReadMethod;
use rtao_runtime::{ConstantDescriptor, RuntimeApiDescriptor, RuntimeHandle, StorageDescriptor};
use serde_json::{Value as JsonValue, json};

use crate::{Client, ClientError};

/// Select a chain head or resolve a historical block exactly once.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BlockRef {
    /// The node's current best block.
    Best,
    /// The node's finalized block.
    Finalized,
    /// Resolve a block height to its current canonical hash.
    Number(u64),
    /// Read an already resolved block identity.
    Hash(H256),
}

/// Head selected when pinning an unpinned client.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum HeadPolicy {
    /// Preserve Python's best-head default.
    #[default]
    Best,
    /// Pin reads to finalized history.
    Finalized,
}

impl From<HeadPolicy> for BlockRef {
    fn from(policy: HeadPolicy) -> Self {
        match policy {
            HeadPolicy::Best => Self::Best,
            HeadPolicy::Finalized => Self::Finalized,
        }
    }
}

/// One block identity together with the runtime that decodes its state.
#[derive(Clone, Debug)]
pub struct PinnedBlock {
    /// Height reported by the pinned header.
    pub number: u64,
    /// Immutable block identity used by every state request.
    pub hash: H256,
    /// Runtime selected using this block's parent version.
    pub runtime: RuntimeHandle,
}

/// Descriptor family used by a read observation.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum ReadKind {
    /// A storage value or map-key scan.
    Storage,
    /// A runtime API invocation.
    RuntimeApi,
    /// A constant decoded from metadata.
    Constant,
}

/// One descriptor actually accessed by a view.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct ReadTouch {
    /// Descriptor family.
    pub kind: ReadKind,
    /// Storage/constant pallet or runtime API name.
    pub pallet_or_api: String,
    /// Storage item, API method or constant name.
    pub item: String,
    /// Pinned state identity, or `None` for an unpinned metadata read.
    pub at: Option<H256>,
}

/// Optional synchronous observer for descriptor and block coverage.
///
/// Observers must return promptly. Reads proceed without an observer by default.
pub trait ReadObserver: std::fmt::Debug + Send + Sync {
    /// Receive an access immediately before the corresponding read.
    fn on_read(&self, touch: ReadTouch);
}

/// A cloneable client view pinned to one immutable block.
#[derive(Clone, Debug)]
pub struct Snapshot {
    pub(crate) client: Client,
    pub(crate) block: PinnedBlock,
    observer: Option<Arc<dyn ReadObserver>>,
}

impl Snapshot {
    /// Adopt a resolved block and its matching runtime.
    pub fn new(client: Client, block: PinnedBlock) -> Self {
        Self {
            client,
            block,
            observer: None,
        }
    }
    /// Attach an observer retained by clones and repeated pinning.
    pub fn with_read_observer(mut self, observer: Arc<dyn ReadObserver>) -> Self {
        self.observer = Some(observer);
        self
    }
    /// Borrow the pinned block identity and runtime.
    pub fn block(&self) -> &PinnedBlock {
        &self.block
    }
    /// Borrow the shared client that owns this view's transport.
    pub fn client(&self) -> &Client {
        &self.client
    }

    /// Clone this snapshot, or resolve a sibling using the same connection.
    ///
    /// An omitted reference or this snapshot's height/hash performs no RPC.
    /// A sibling retains the read observer and owns its resolved immutable hash.
    /// Resolving a new sibling requires an open connection, unlike Python's
    /// lazy height-only sibling construction.
    pub async fn at(&self, block: Option<BlockRef>) -> Result<Self, ClientError> {
        match block {
            None => Ok(self.clone()),
            Some(BlockRef::Hash(hash)) if hash == self.block.hash => Ok(self.clone()),
            Some(BlockRef::Number(number)) if number == self.block.number => Ok(self.clone()),
            Some(block) => {
                let mut sibling = self.client.at(block).await?;
                sibling.observer = self.observer.clone();
                Ok(sibling)
            }
        }
    }

    /// Copy this connection's cached display symbols without requesting chain state.
    pub async fn token_symbols(&self) -> BTreeMap<rtao_primitives::NetUid, String> {
        self.client.token_symbols().await
    }

    /// Tag a chain-native amount with this connection's cached display symbol.
    pub async fn balance(
        &self,
        rao: u64,
        netuid: rtao_primitives::NetUid,
    ) -> rtao_primitives::DynBalance {
        self.client.balance(rao, netuid).await
    }

    /// Describe this block, or resolve a requested height while retaining observers.
    pub async fn block_info(
        &self,
        block: Option<u64>,
    ) -> Result<Option<crate::BlockInfo>, ClientError> {
        let Some(number) = block.filter(|number| *number != self.block.number) else {
            return crate::block_info::fetch(self).await;
        };
        let Some(mut snapshot) = crate::block_info::select(&self.client, Some(number)).await?
        else {
            return Ok(None);
        };
        snapshot.observer = self.observer.clone();
        crate::block_info::fetch(&snapshot).await
    }
}

/// Transport-independent primitive reads used by namespace methods.
pub trait View: Send + Sync {
    /// Optional descriptor observer, including reads performed by pinned clones.
    fn read_observer(&self) -> Option<&dyn ReadObserver> {
        None
    }
    /// Whether curated reads may use the borrowed metagraph decoder.
    ///
    /// Custom views permit it by default. A client and every snapshot retain
    /// the connection's `ClientConfig::metagraph_view` opt-out.
    fn metagraph_view_enabled(&self) -> bool {
        true
    }
    /// Return the pinned identity; an ordinary client leaves head choice to the node.
    fn pinned(&self) -> Option<&PinnedBlock>;
    /// Resolve an unpinned view once; a snapshot clones itself.
    fn pin(&self) -> impl Future<Output = Result<Snapshot, ClientError>> + Send;
    /// Resolve the runtime belonging to this view.
    fn runtime(&self) -> impl Future<Output = Result<RuntimeHandle, ClientError>> + Send;
    /// Read one already constructed storage key.
    fn storage_raw(
        &self,
        key: &[u8],
    ) -> impl Future<Output = Result<Option<Bytes>, ClientError>> + Send;
    /// Read storage keys in input order, preserving absent values.
    fn storage_raw_many(
        &self,
        keys: &[Vec<u8>],
    ) -> impl Future<Output = Result<Vec<Option<Bytes>>, ClientError>> + Send;
    /// Read a lexicographically ordered map page after an exclusive cursor.
    fn keys_paged(
        &self,
        prefix: &[u8],
        count: u32,
        start: Option<&[u8]>,
    ) -> impl Future<Output = Result<Vec<Vec<u8>>, ClientError>> + Send;
    /// Invoke an already encoded runtime method at this view's identity.
    fn state_call(
        &self,
        method: &str,
        args: &[u8],
    ) -> impl Future<Output = Result<Bytes, ClientError>> + Send;
}

impl<V: View + ?Sized> View for &V {
    fn read_observer(&self) -> Option<&dyn ReadObserver> {
        View::read_observer(*self)
    }
    fn metagraph_view_enabled(&self) -> bool {
        View::metagraph_view_enabled(*self)
    }
    fn pinned(&self) -> Option<&PinnedBlock> {
        View::pinned(*self)
    }
    async fn pin(&self) -> Result<Snapshot, ClientError> {
        View::pin(*self).await
    }
    async fn runtime(&self) -> Result<RuntimeHandle, ClientError> {
        View::runtime(*self).await
    }
    async fn storage_raw(&self, key: &[u8]) -> Result<Option<Bytes>, ClientError> {
        View::storage_raw(*self, key).await
    }
    async fn storage_raw_many(&self, keys: &[Vec<u8>]) -> Result<Vec<Option<Bytes>>, ClientError> {
        View::storage_raw_many(*self, keys).await
    }
    async fn keys_paged(
        &self,
        prefix: &[u8],
        count: u32,
        start: Option<&[u8]>,
    ) -> Result<Vec<Vec<u8>>, ClientError> {
        View::keys_paged(*self, prefix, count, start).await
    }
    async fn state_call(&self, method: &str, args: &[u8]) -> Result<Bytes, ClientError> {
        View::state_call(*self, method, args).await
    }
}

/// Object-safe twin of [`View`] for dispatchers and recording transports.
pub trait DynView: Send + Sync {
    /// Optional descriptor observer retained by the object-safe view.
    fn read_observer(&self) -> Option<&dyn ReadObserver> {
        None
    }
    /// Whether curated reads may use the borrowed metagraph decoder.
    fn metagraph_view_enabled(&self) -> bool {
        true
    }
    /// Return the pinned identity.
    fn pinned(&self) -> Option<&PinnedBlock>;
    /// Resolve this view to an immutable snapshot.
    fn pin(&self) -> BoxFuture<'_, Result<Snapshot, ClientError>>;
    /// Resolve this view's runtime.
    fn runtime(&self) -> BoxFuture<'_, Result<RuntimeHandle, ClientError>>;
    /// Read an encoded storage key.
    fn storage_raw<'a>(
        &'a self,
        key: &'a [u8],
    ) -> BoxFuture<'a, Result<Option<Bytes>, ClientError>>;
    /// Read encoded keys in input order.
    fn storage_raw_many<'a>(
        &'a self,
        keys: &'a [Vec<u8>],
    ) -> BoxFuture<'a, Result<Vec<Option<Bytes>>, ClientError>>;
    /// Read a map-key page.
    fn keys_paged<'a>(
        &'a self,
        prefix: &'a [u8],
        count: u32,
        start: Option<&'a [u8]>,
    ) -> BoxFuture<'a, Result<Vec<Vec<u8>>, ClientError>>;
    /// Invoke an encoded runtime method.
    fn state_call<'a>(
        &'a self,
        method: &'a str,
        args: &'a [u8],
    ) -> BoxFuture<'a, Result<Bytes, ClientError>>;
}

impl<T: View> DynView for T {
    fn read_observer(&self) -> Option<&dyn ReadObserver> {
        View::read_observer(self)
    }
    fn metagraph_view_enabled(&self) -> bool {
        View::metagraph_view_enabled(self)
    }
    fn pinned(&self) -> Option<&PinnedBlock> {
        View::pinned(self)
    }
    fn pin(&self) -> BoxFuture<'_, Result<Snapshot, ClientError>> {
        Box::pin(View::pin(self))
    }
    fn runtime(&self) -> BoxFuture<'_, Result<RuntimeHandle, ClientError>> {
        Box::pin(View::runtime(self))
    }
    fn storage_raw<'a>(
        &'a self,
        key: &'a [u8],
    ) -> BoxFuture<'a, Result<Option<Bytes>, ClientError>> {
        Box::pin(View::storage_raw(self, key))
    }
    fn storage_raw_many<'a>(
        &'a self,
        keys: &'a [Vec<u8>],
    ) -> BoxFuture<'a, Result<Vec<Option<Bytes>>, ClientError>> {
        Box::pin(View::storage_raw_many(self, keys))
    }
    fn keys_paged<'a>(
        &'a self,
        prefix: &'a [u8],
        count: u32,
        start: Option<&'a [u8]>,
    ) -> BoxFuture<'a, Result<Vec<Vec<u8>>, ClientError>> {
        Box::pin(View::keys_paged(self, prefix, count, start))
    }
    fn state_call<'a>(
        &'a self,
        method: &'a str,
        args: &'a [u8],
    ) -> BoxFuture<'a, Result<Bytes, ClientError>> {
        Box::pin(View::state_call(self, method, args))
    }
}

impl View for Client {
    fn metagraph_view_enabled(&self) -> bool {
        Client::metagraph_view_enabled(self)
    }
    fn pinned(&self) -> Option<&PinnedBlock> {
        None
    }
    async fn pin(&self) -> Result<Snapshot, ClientError> {
        self.at(self.head_policy().into()).await
    }
    async fn runtime(&self) -> Result<RuntimeHandle, ClientError> {
        self.runtime_for(None).await
    }
    async fn storage_raw(&self, key: &[u8]) -> Result<Option<Bytes>, ClientError> {
        storage_raw(self, key, None).await
    }
    async fn storage_raw_many(&self, keys: &[Vec<u8>]) -> Result<Vec<Option<Bytes>>, ClientError> {
        let snapshot = View::pin(self).await?;
        View::storage_raw_many(&snapshot, keys).await
    }
    async fn keys_paged(
        &self,
        prefix: &[u8],
        count: u32,
        start: Option<&[u8]>,
    ) -> Result<Vec<Vec<u8>>, ClientError> {
        keys_paged(self, prefix, count, start, None).await
    }
    async fn state_call(&self, method: &str, args: &[u8]) -> Result<Bytes, ClientError> {
        state_call(self, method, args, None).await
    }
}

impl View for Snapshot {
    fn read_observer(&self) -> Option<&dyn ReadObserver> {
        self.observer.as_deref()
    }
    fn metagraph_view_enabled(&self) -> bool {
        self.client.metagraph_view_enabled()
    }
    fn pinned(&self) -> Option<&PinnedBlock> {
        Some(&self.block)
    }
    async fn pin(&self) -> Result<Snapshot, ClientError> {
        Ok(self.clone())
    }
    async fn runtime(&self) -> Result<RuntimeHandle, ClientError> {
        Ok(self.block.runtime.clone())
    }
    async fn storage_raw(&self, key: &[u8]) -> Result<Option<Bytes>, ClientError> {
        self.observe_storage_key(key);
        storage_raw(&self.client, key, Some(self.block.hash)).await
    }
    async fn storage_raw_many(&self, keys: &[Vec<u8>]) -> Result<Vec<Option<Bytes>>, ClientError> {
        let mut unique = Vec::new();
        let mut seen = BTreeSet::new();
        for key in keys {
            if seen.insert(key.clone()) {
                self.observe_storage_key(key);
                unique.push(key.clone());
            }
        }
        let mut found = BTreeMap::new();
        for chunk in unique.chunks(STORAGE_PAGE) {
            let result = self
                .client
                .read_request(
                    ReadMethod::StateQueryStorageAt,
                    json!([
                        chunk.iter().map(|key| hex_bytes(key)).collect::<Vec<_>>(),
                        crate::hash_hex(self.block.hash)
                    ]),
                )
                .await?;
            for (key, value) in chunk
                .iter()
                .zip(decode_changes(&result, chunk, self.block.hash)?)
            {
                found.insert(key.clone(), value);
            }
        }
        let output = keys
            .iter()
            .map(|key| {
                found.get(key).cloned().ok_or_else(|| {
                    ClientError::Protocol("coalesced storage result was omitted".into())
                })
            })
            .collect::<Result<Vec<_>, _>>()?;
        Ok(output)
    }
    async fn keys_paged(
        &self,
        prefix: &[u8],
        count: u32,
        start: Option<&[u8]>,
    ) -> Result<Vec<Vec<u8>>, ClientError> {
        self.observe_storage_key(prefix);
        keys_paged(&self.client, prefix, count, start, Some(self.block.hash)).await
    }
    async fn state_call(&self, method: &str, args: &[u8]) -> Result<Bytes, ClientError> {
        if let Some((api, item)) = method.split_once('_') {
            observe(self, ReadKind::RuntimeApi, api, item);
        }
        state_call(&self.client, method, args, Some(self.block.hash)).await
    }
}

impl Snapshot {
    fn observe_storage_key(&self, key: &[u8]) {
        if self.observer.is_none() || key.len() < 32 {
            return;
        }
        // The core constructs each metadata prefix. Recording also covers raw
        // reads and empty map scans, which never reach a value decoder.
        for pallet in &self.block.runtime.core().pallets {
            for item in &pallet.storage {
                if self
                    .block
                    .runtime
                    .core()
                    .storage_key(item, &[])
                    .is_ok_and(|prefix| key.starts_with(&prefix))
                {
                    observe(self, ReadKind::Storage, &pallet.name, &item.name);
                    return;
                }
            }
        }
    }
}

fn observe<V: View + ?Sized>(view: &V, kind: ReadKind, pallet_or_api: &str, item: &str) {
    if let Some(observer) = View::read_observer(view) {
        observer.on_read(ReadTouch {
            kind,
            pallet_or_api: pallet_or_api.into(),
            item: item.into(),
            at: View::pinned(view).map(|block| block.hash),
        });
    }
}

fn hex_bytes(bytes: &[u8]) -> String {
    format!("0x{}", hex::encode(bytes))
}
fn decode_hex(value: &JsonValue) -> Result<Bytes, ClientError> {
    let text = value
        .as_str()
        .ok_or_else(|| ClientError::Protocol("RPC SCALE result is not a string".into()))?;
    let text = text
        .strip_prefix("0x")
        .ok_or_else(|| ClientError::Protocol("RPC SCALE result lacks 0x prefix".into()))?;
    hex::decode(text)
        .map(Bytes::from)
        .map_err(|error| ClientError::Protocol(error.to_string()))
}
async fn storage_raw(
    client: &Client,
    key: &[u8],
    hash: Option<H256>,
) -> Result<Option<Bytes>, ClientError> {
    let mut params = vec![json!(hex_bytes(key))];
    if let Some(hash) = hash {
        params.push(json!(crate::hash_hex(hash)));
    }
    let result = client
        .read_request(ReadMethod::StateGetStorage, json!(params))
        .await?;
    if result.is_null() {
        Ok(None)
    } else {
        decode_hex(&result).map(Some)
    }
}
async fn keys_paged(
    client: &Client,
    prefix: &[u8],
    count: u32,
    start: Option<&[u8]>,
    hash: Option<H256>,
) -> Result<Vec<Vec<u8>>, ClientError> {
    if count == 0 {
        return Err(ClientError::Protocol(
            "storage page count must be positive".into(),
        ));
    }
    let mut params = vec![
        json!(hex_bytes(prefix)),
        json!(count),
        start.map_or(JsonValue::Null, |start| json!(hex_bytes(start))),
    ];
    if let Some(hash) = hash {
        params.push(json!(crate::hash_hex(hash)));
    }
    let result = client
        .read_request(ReadMethod::StateGetKeysPaged, json!(params))
        .await?;
    let result = result
        .as_array()
        .ok_or_else(|| ClientError::Protocol("storage key page is not an array".into()))?;
    if result.len() > count as usize {
        return Err(ClientError::Protocol(
            "storage key page exceeds requested count".into(),
        ));
    }
    let mut keys = Vec::with_capacity(result.len());
    let mut previous = start.map(<[u8]>::to_vec);
    for value in result {
        let key = decode_hex(value)?.to_vec();
        if !key.starts_with(prefix)
            || previous
                .as_deref()
                .is_some_and(|previous| key.as_slice() <= previous)
        {
            return Err(ClientError::Protocol(
                "storage key page is outside the prefix or not strictly advancing".into(),
            ));
        }
        previous = Some(key.clone());
        keys.push(key);
    }
    Ok(keys)
}
async fn state_call(
    client: &Client,
    method: &str,
    args: &[u8],
    hash: Option<H256>,
) -> Result<Bytes, ClientError> {
    let mut params = vec![json!(method), json!(hex_bytes(args))];
    if let Some(hash) = hash {
        params.push(json!(crate::hash_hex(hash)));
    }
    decode_hex(
        &client
            .read_request(ReadMethod::StateCall, json!(params))
            .await?,
    )
}
fn decode_changes(
    result: &JsonValue,
    keys: &[Vec<u8>],
    expected_hash: H256,
) -> Result<Vec<Option<Bytes>>, ClientError> {
    let groups = result
        .as_array()
        .ok_or_else(|| ClientError::Protocol("storage change sets are not an array".into()))?;
    let requested: BTreeSet<_> = keys.iter().map(Vec::as_slice).collect();
    let mut found = BTreeMap::new();
    for group in groups {
        if crate::parse_hash(
            group
                .get("block")
                .ok_or_else(|| ClientError::Protocol("storage change set lacks block".into()))?,
        )? != expected_hash
        {
            return Err(ClientError::Protocol(
                "storage change set belongs to another block".into(),
            ));
        }
        let changes = group
            .get("changes")
            .and_then(JsonValue::as_array)
            .ok_or_else(|| ClientError::Protocol("storage change set lacks changes".into()))?;
        for change in changes {
            let pair = change
                .as_array()
                .filter(|pair| pair.len() == 2)
                .ok_or_else(|| {
                    ClientError::Protocol("storage change is not a key/value pair".into())
                })?;
            let key = decode_hex(&pair[0])?.to_vec();
            if !requested.contains(key.as_slice()) {
                return Err(ClientError::Protocol(
                    "storage change set returned an unrequested key".into(),
                ));
            }
            let value = if pair[1].is_null() {
                None
            } else {
                Some(decode_hex(&pair[1])?)
            };
            if found.insert(key, value).is_some() {
                return Err(ClientError::Protocol(
                    "storage change set repeated a key".into(),
                ));
            }
        }
    }
    keys.iter()
        .map(|key| {
            found.get(key).cloned().ok_or_else(|| {
                ClientError::Protocol("storage change set omitted a requested key".into())
            })
        })
        .collect()
}

/// Positional values supplied to storage keys and runtime API arguments.
pub trait IntoCoreValues: Send {
    /// Convert arguments without encoding chain bytes independently.
    fn into_core_values(self) -> Vec<Value>;
}
impl IntoCoreValues for () {
    fn into_core_values(self) -> Vec<Value> {
        Vec::new()
    }
}
impl IntoCoreValues for Vec<Value> {
    fn into_core_values(self) -> Vec<Value> {
        self
    }
}
macro_rules! argument_tuples {
    ($(($($name:ident:$index:tt),+)),+ $(,)?) => {$ (
        impl<$($name: IntoCoreValue + Send),+> IntoCoreValues for ($($name,)+) {
            fn into_core_values(self) -> Vec<Value> { vec![$(self.$index.into_core_value()),+] }
        }
    )+};
}
argument_tuples!((A:0),(A:0,B:1),(A:0,B:1,C:2),(A:0,B:1,C:2,D:3),(A:0,B:1,C:2,D:3,E:4),(A:0,B:1,C:2,D:3,E:4,F:5),(A:0,B:1,C:2,D:3,E:4,F:5,G:6),(A:0,B:1,C:2,D:3,E:4,F:5,G:6,H:7));

/// Typed map-key arguments and the free components returned during iteration.
pub trait KeyArgs: IntoCoreValues + Clone + Send + Sync {
    /// Leading key components fixed by a map scan.
    type Prefix: IntoCoreValues + Send;
    /// Free decoded components after the fixed prefix.
    type Decoded: Send;
    /// Convert live decoded key components to the iterator output.
    fn decode_free(values: Vec<Value>) -> Result<Self::Decoded, ClientError>;
}
impl<T: IntoCoreValues + Clone + Send + Sync> KeyArgs for T {
    type Prefix = Vec<Value>;
    type Decoded = Vec<DynValue>;
    fn decode_free(values: Vec<Value>) -> Result<Self::Decoded, ClientError> {
        Ok(values.into_iter().map(DynValue::new).collect())
    }
}

/// Decode storage values using the live entry's missing-value behavior.
pub trait FromStorage: Send + Sync + 'static {
    /// Result returned by an individual read or a coalesced query.
    type Out: Send + 'static;
    /// Present value type returned by map iteration.
    type Value: crate::read_decode::ReadValue;
    /// Decode present bytes or apply live Default/Optional semantics.
    fn from_storage(
        runtime: &RuntimeHandle,
        info: &rtao_core::runtime::StorageInfo,
        raw: Option<&[u8]>,
    ) -> Result<Self::Out, ClientError>;
}
impl<T: crate::read_decode::ReadValue> FromStorage for T {
    type Out = T;
    type Value = T;
    fn from_storage(
        runtime: &RuntimeHandle,
        info: &rtao_core::runtime::StorageInfo,
        raw: Option<&[u8]>,
    ) -> Result<T, ClientError> {
        match raw {
            Some(bytes) => T::decode(runtime, info.value_type, bytes),
            None if info.modifier == "Default" => {
                T::decode(runtime, info.value_type, &info.default_bytes)
            }
            None => Err(ClientError::SurfaceChanged {
                descriptor: "storage",
                detail: "typed nonoptional descriptor encountered an Optional miss".into(),
            }),
        }
    }
}
impl<T: crate::read_decode::ReadValue> FromStorage for rtao_runtime::OptionalStorage<T> {
    type Out = Option<T>;
    type Value = T;
    fn from_storage(
        runtime: &RuntimeHandle,
        info: &rtao_core::runtime::StorageInfo,
        raw: Option<&[u8]>,
    ) -> Result<Option<T>, ClientError> {
        match raw {
            Some(bytes) => T::decode(runtime, info.value_type, bytes).map(Some),
            None if info.modifier == "Default" => {
                T::decode(runtime, info.value_type, &info.default_bytes).map(Some)
            }
            None => Ok(None),
        }
    }
}

/// One bound storage query after its typed keys have been erased.
#[derive(Clone, Debug)]
pub struct ReadSpec {
    /// Names and structural shape checked against the selected runtime.
    pub surface: rtao_runtime::StorageSurface,
    /// Unhashed metadata key values.
    pub keys: Vec<Value>,
}

/// A collection of storage queries coalesced at one pinned block.
pub trait Fetch: Send + 'static {
    /// Decoded outputs in the same order as the supplied queries.
    type Output: Send + 'static;
    /// Return the metadata contracts and unhashed keys.
    fn queries(&self) -> Vec<ReadSpec>;
    /// Decode the corresponding raw values using each live entry.
    fn decode(
        self,
        runtime: &RuntimeHandle,
        values: &[Option<Bytes>],
    ) -> Result<Self::Output, ClientError>;
}
impl<K: KeyArgs + 'static, V: FromStorage> Fetch for rtao_runtime::StorageQuery<K, V> {
    type Output = V::Out;
    fn queries(&self) -> Vec<ReadSpec> {
        vec![ReadSpec {
            surface: self.descriptor.surface(),
            keys: self.keys.clone().into_core_values(),
        }]
    }
    fn decode(
        self,
        runtime: &RuntimeHandle,
        values: &[Option<Bytes>],
    ) -> Result<Self::Output, ClientError> {
        let info = storage_info(runtime, self.descriptor.surface())?;
        let raw = values
            .first()
            .ok_or_else(|| ClientError::Protocol("coalesced storage result was omitted".into()))?;
        V::from_storage(runtime, info, raw.as_deref())
    }
}
macro_rules! fetch_tuples {
    ($(($($name:ident:$index:tt),+)),+ $(,)?) => {$ (
        impl<$($name: Fetch),+> Fetch for ($($name,)+) {
            type Output = ($($name::Output,)+);
            fn queries(&self) -> Vec<ReadSpec> {
                let mut queries = Vec::new();
                $(queries.extend(self.$index.queries());)+
                queries
            }
            fn decode(self, runtime: &RuntimeHandle, values: &[Option<Bytes>]) -> Result<Self::Output, ClientError> {
                let mut offset = 0usize;
                $(let count = self.$index.queries().len();
                  let end = offset.checked_add(count).ok_or_else(|| ClientError::Protocol("storage plan length overflow".into()))?;
                  let $name = self.$index.decode(runtime, values.get(offset..end).ok_or_else(|| ClientError::Protocol("coalesced storage result length differs from its plan".into()))?)?;
                  offset = end;)+
                if offset != values.len() { return Err(ClientError::Protocol("coalesced storage result contains extra values".into())); }
                Ok(($($name,)+))
            }
        }
    )+};
}
#[allow(non_snake_case)]
mod tuples {
    use super::*;
    fetch_tuples!((A:0),(A:0,B:1),(A:0,B:1,C:2),(A:0,B:1,C:2,D:3),(A:0,B:1,C:2,D:3,E:4),(A:0,B:1,C:2,D:3,E:4,F:5),(A:0,B:1,C:2,D:3,E:4,F:5,G:6),(A:0,B:1,C:2,D:3,E:4,F:5,G:6,H:7),(A:0,B:1,C:2,D:3,E:4,F:5,G:6,H:7,I:8),(A:0,B:1,C:2,D:3,E:4,F:5,G:6,H:7,I:8,J:9),(A:0,B:1,C:2,D:3,E:4,F:5,G:6,H:7,I:8,J:9,K:10),(A:0,B:1,C:2,D:3,E:4,F:5,G:6,H:7,I:8,J:9,K:10,L:11),(A:0,B:1,C:2,D:3,E:4,F:5,G:6,H:7,I:8,J:9,K:10,L:11,M:12),(A:0,B:1,C:2,D:3,E:4,F:5,G:6,H:7,I:8,J:9,K:10,L:11,M:12,N:13),(A:0,B:1,C:2,D:3,E:4,F:5,G:6,H:7,I:8,J:9,K:10,L:11,M:12,N:13,O:14),(A:0,B:1,C:2,D:3,E:4,F:5,G:6,H:7,I:8,J:9,K:10,L:11,M:12,N:13,O:14,P:15));
}

/// A variable-length coalesced plan whose outputs retain Python's shapes.
#[derive(Clone, Debug, Default)]
pub struct ReadPlan {
    queries: Vec<ReadSpec>,
}
impl ReadPlan {
    /// Create an empty plan; fetching it performs no storage RPC.
    pub fn new() -> Self {
        Self::default()
    }
    /// Append a named storage query and its unhashed keys.
    pub fn push(&mut self, surface: rtao_runtime::StorageSurface, keys: Vec<Value>) {
        self.queries.push(ReadSpec { surface, keys });
    }
    /// Number of logical queries, including duplicates.
    pub fn len(&self) -> usize {
        self.queries.len()
    }
    /// Whether this plan has no queries.
    pub fn is_empty(&self) -> bool {
        self.queries.is_empty()
    }
}
impl Fetch for ReadPlan {
    type Output = Vec<DynValue>;
    fn queries(&self) -> Vec<ReadSpec> {
        self.queries.clone()
    }
    fn decode(
        self,
        runtime: &RuntimeHandle,
        values: &[Option<Bytes>],
    ) -> Result<Self::Output, ClientError> {
        if self.queries.len() != values.len() {
            return Err(ClientError::Protocol(
                "coalesced storage result length differs from its plan".into(),
            ));
        }
        self.queries
            .iter()
            .zip(values)
            .map(|(query, raw)| {
                let info = storage_info(runtime, query.surface)?;
                rtao_core::ops::storage::decode_storage_bytes(runtime.core(), info, raw.as_deref())
                    .map(DynValue::new)
                    .map_err(|error| decode_error("storage", error.into()))
            })
            .collect()
    }
}

fn decode_error(what: impl Into<std::borrow::Cow<'static, str>>, source: CoreError) -> ClientError {
    ClientError::Decode {
        what: what.into(),
        source,
    }
}
fn storage_info(
    runtime: &RuntimeHandle,
    surface: rtao_runtime::StorageSurface,
) -> Result<&rtao_core::runtime::StorageInfo, ClientError> {
    runtime
        .storage_entry(surface.pallet, surface.name)
        .ok_or_else(|| {
            CoreError::NotInRuntime(format!("storage {}.{}", surface.pallet, surface.name)).into()
        })
}
fn check_surface(
    runtime: &RuntimeHandle,
    descriptor: rtao_runtime::SurfaceDescriptor,
    id: String,
    name: &'static str,
) -> Result<(), ClientError> {
    let report = rtao_runtime::verify_surface_with(runtime, &[descriptor]);
    if report.shape_matches(&id) {
        Ok(())
    } else {
        Err(ClientError::SurfaceChanged {
            descriptor: name,
            detail: report
                .changes
                .iter()
                .filter(|change| change.descriptor == id)
                .map(|change| change.detail.as_str())
                .collect::<Vec<_>>()
                .join("; "),
        })
    }
}
fn storage_key(
    runtime: &RuntimeHandle,
    surface: rtao_runtime::StorageSurface,
    keys: &[Value],
) -> Result<Vec<u8>, ClientError> {
    let info = storage_info(runtime, surface)?;
    if keys.len() != info.key_types.len() {
        return Err(ClientError::Protocol(format!(
            "{}.{} expects {} key arguments, got {}",
            surface.pallet,
            surface.name,
            info.key_types.len(),
            keys.len()
        )));
    }
    runtime
        .core()
        .storage_key(info, keys)
        .map_err(|error| decode_error("storage key", error.into()))
}

async fn query_batch<V: View + ?Sized>(
    view: &V,
    pallet: &str,
    item: &str,
    param_sets: Vec<Vec<Value>>,
) -> Result<Vec<DynValue>, ClientError> {
    if param_sets.is_empty() {
        return Ok(Vec::new());
    }
    let snapshot = view.pin().await?;
    let runtime = snapshot.block().runtime.clone();
    let entry = runtime
        .storage_entry(pallet, item)
        .cloned()
        .ok_or_else(|| {
            ClientError::from(rtao_core::CoreError::NotInRuntime(format!(
                "storage {pallet}.{item}"
            )))
        })?;
    let keys = param_sets
        .iter()
        .map(|params| {
            if params.len() != entry.key_types.len() {
                return Err(ClientError::Protocol(format!(
                    "{pallet}.{item} expects {} key arguments, got {}",
                    entry.key_types.len(),
                    params.len()
                )));
            }
            runtime
                .core()
                .storage_key(&entry, params)
                .map_err(rtao_core::CoreError::from)
                .map_err(ClientError::from)
        })
        .collect::<Result<Vec<_>, _>>()?;
    let values = View::storage_raw_many(&snapshot, &keys).await?;
    let offload = values.iter().fold(0usize, |total, value| {
        total.saturating_add(value.as_ref().map_or(entry.default_bytes.len(), Bytes::len))
    }) > 64 * 1024;
    let decode = move || {
        values
            .into_iter()
            .map(|value| {
                rtao_core::ops::storage::decode_storage_bytes(
                    runtime.core(),
                    &entry,
                    value.as_deref(),
                )
                .map(DynValue::new)
                .map_err(rtao_core::CoreError::from)
                .map_err(ClientError::from)
            })
            .collect()
    };
    if offload {
        rtao_core::offload(decode).await.map_err(|error| {
            ClientError::Protocol(format!("storage batch decoder task failed: {error}"))
        })?
    } else {
        decode()
    }
}

/// Typed and Python-shaped reads built on a [`View`].
pub trait ViewExt: View {
    /// Read one named storage map for many keys at one pinned block.
    ///
    /// Results preserve key order and duplicates. An empty batch returns before
    /// metadata lookup or RPC, including when the connection is closed.
    fn query_batch_dyn(
        &self,
        pallet: &str,
        item: &str,
        param_sets: Vec<Vec<Value>>,
    ) -> impl Future<Output = Result<Vec<DynValue>, ClientError>> + Send {
        query_batch(self, pallet, item, param_sets)
    }

    /// Read one typed storage entry using its live key types and hashers.
    fn storage<K: KeyArgs + 'static, V: FromStorage>(
        &self,
        descriptor: &StorageDescriptor<K, V>,
        key: K,
    ) -> impl Future<Output = Result<V::Out, ClientError>> + Send {
        let query = descriptor.at(key);
        async move { self.fetch(query).await }
    }
    /// Coalesce all storage queries at one hash, with at most 1,000 keys per RPC.
    fn fetch<F: Fetch>(
        &self,
        plan: F,
    ) -> impl Future<Output = Result<F::Output, ClientError>> + Send {
        async move {
            let snapshot = View::pin(self).await?;
            let runtime = snapshot.block.runtime.clone();
            let queries = plan.queries();
            let mut keys = Vec::with_capacity(queries.len());
            for query in &queries {
                check_surface(
                    &runtime,
                    rtao_runtime::SurfaceDescriptor::Storage(query.surface),
                    format!("storage:{}.{}", query.surface.pallet, query.surface.name),
                    query.surface.name,
                )?;
                keys.push(storage_key(&runtime, query.surface, &query.keys)?);
            }
            let values = View::storage_raw_many(&snapshot, &keys).await?;
            if values.iter().flatten().map(Bytes::len).sum::<usize>() > 64 * 1024 {
                rtao_core::offload(move || plan.decode(&runtime, &values))
                    .await
                    .map_err(|error| {
                        ClientError::Protocol(format!("storage decoder task failed: {error}"))
                    })?
            } else {
                plan.decode(&runtime, &values)
            }
        }
    }
    /// Iterate a map at one pinned hash; the prefix contains leading unhashed keys.
    fn storage_iter<'a, K: KeyArgs + 'static, V: FromStorage>(
        &'a self,
        descriptor: &'a StorageDescriptor<K, V>,
        prefix: impl IntoCoreValues + 'a,
    ) -> impl Stream<Item = Result<(K::Decoded, V::Value), ClientError>> + Send + 'a {
        let surface = descriptor.surface();
        storage_stream(
            self,
            surface.pallet,
            surface.name,
            prefix.into_core_values(),
            Some(surface),
            |runtime, info, keys, raw| {
                let value = <V::Value as crate::read_decode::ReadValue>::decode(
                    runtime,
                    info.value_type,
                    &raw,
                )?;
                Ok((K::decode_free(keys)?, value))
            },
        )
    }
    /// Iterate present encoded values of a named map with live metadata.
    ///
    /// Each result includes free decoded keys, raw value, runtime, and live
    /// entry information for callers that need an alternate model decoder.
    fn storage_iter_dyn<'a>(
        &'a self,
        pallet: &'a str,
        item: &'a str,
        prefix: Vec<Value>,
    ) -> impl Stream<
        Item = Result<
            (
                Vec<Value>,
                Bytes,
                RuntimeHandle,
                rtao_core::runtime::StorageInfo,
            ),
            ClientError,
        >,
    > + Send
    + 'a {
        storage_stream(
            self,
            pallet,
            item,
            prefix,
            None,
            |runtime, info, keys, raw| Ok((keys, raw, runtime.clone(), info.clone())),
        )
    }
    /// Iterate dynamic map keys and values with Python-shaped decoding.
    ///
    /// Free key components remain a vector so a scan with a fixed prefix can
    /// distinguish a single free component from several components.
    /// Keys and values decode strictly. A deleted value is skipped before key
    /// decoding; an invalid present key or value fails the whole page.
    /// Python's private transport can optionally retain an undecodable key as
    /// `None`; this method follows the strict public SDK map contract instead.
    fn query_map_dyn<'a>(
        &'a self,
        pallet: &'a str,
        item: &'a str,
        prefix: Vec<Value>,
    ) -> impl Stream<Item = Result<(Vec<DynValue>, DynValue), ClientError>> + Send + 'a {
        storage_stream(
            self,
            pallet,
            item,
            prefix,
            None,
            |runtime, info, keys, raw| {
                let value =
                    rtao_core::ops::storage::decode_storage_bytes(runtime.core(), info, Some(&raw))
                        .map_err(CoreError::from)?;
                Ok((
                    keys.into_iter().map(DynValue::new).collect(),
                    DynValue::new(value),
                ))
            },
        )
    }
    /// Encode and decode a typed modern runtime API using live metadata.
    fn call<A: IntoCoreValues, R: crate::read_decode::ReadValue>(
        &self,
        descriptor: &RuntimeApiDescriptor<A, R>,
        args: A,
    ) -> impl Future<Output = Result<R, ClientError>> + Send {
        let surface = descriptor.surface();
        async move {
            let snapshot = View::pin(self).await?;
            let runtime = &snapshot.block.runtime;
            if crate::legacy_api::is_legacy(runtime, surface.api, surface.name) {
                return Err(ClientError::RuntimeApiUnavailable {
                    api: surface.api.into(),
                    method: surface.name.into(),
                });
            }
            let have = runtime.runtime_api_version(surface.api).unwrap_or(0);
            if have < surface.version {
                return Err(ClientError::ApiVersion {
                    api: surface.api.into(),
                    need: surface.version,
                    have,
                });
            }
            let info = runtime
                .runtime_api(surface.api, surface.name)
                .ok_or_else(|| ClientError::RuntimeApiUnavailable {
                    api: surface.api.into(),
                    method: surface.name.into(),
                })?;
            check_surface(
                runtime,
                rtao_runtime::SurfaceDescriptor::RuntimeApi(surface),
                format!("api:{}.{}", surface.api, surface.name),
                surface.name,
            )?;
            let encoded = rtao_core::ops::runtime_api::encode_params(
                runtime.core(),
                surface.api,
                info,
                &args.into_core_values(),
            )
            .map_err(CoreError::from)?;
            let bytes = View::state_call(
                &snapshot,
                &format!("{}_{}", surface.api, surface.name),
                &encoded,
            )
            .await?;
            if bytes.len() > 64 * 1024 {
                let runtime = runtime.clone();
                let output = info.output;
                rtao_core::offload(move || R::decode(&runtime, output, &bytes))
                    .await
                    .map_err(|error| {
                        ClientError::Protocol(format!("API decoder task failed: {error}"))
                    })?
            } else {
                R::decode(runtime, info.output, &bytes)
            }
        }
    }
    /// Read a constant from this view's own runtime without a storage RPC.
    fn constant<V: crate::read_decode::ReadValue>(
        &self,
        descriptor: &ConstantDescriptor<V>,
    ) -> impl Future<Output = Result<V, ClientError>> + Send {
        let surface = descriptor.surface();
        async move {
            let runtime = View::runtime(self).await?;
            check_surface(
                &runtime,
                rtao_runtime::SurfaceDescriptor::Constant(surface),
                format!("constant:{}.{}", surface.pallet, surface.name),
                surface.name,
            )?;
            let info = runtime
                .core()
                .constant(surface.pallet, surface.name)
                .ok_or_else(|| {
                    CoreError::NotInRuntime(format!("constant {}.{}", surface.pallet, surface.name))
                })?;
            observe(self, ReadKind::Constant, surface.pallet, surface.name);
            V::decode(&runtime, info.ty, &info.value)
        }
    }
    /// Decode both Python records and retained typed event fields at one block.
    fn events(&self) -> impl Future<Output = Result<crate::Events, ClientError>> + Send {
        async move {
            let snapshot = View::pin(self).await?;
            let runtime = snapshot.block.runtime.clone();
            let info = runtime
                .storage_entry("System", "Events")
                .ok_or_else(|| CoreError::NotInRuntime("storage System.Events".into()))?;
            let key = runtime
                .core()
                .storage_key(info, &[])
                .map_err(CoreError::from)?;
            let bytes = View::storage_raw(&snapshot, &key)
                .await?
                .unwrap_or_else(|| Bytes::copy_from_slice(&info.default_bytes));
            if bytes.len() > 64 * 1024 {
                rtao_core::offload(move || crate::Events::decode(runtime, bytes))
                    .await
                    .map_err(|error| {
                        ClientError::Protocol(format!("event decoder task failed: {error}"))
                    })?
            } else {
                crate::Events::decode(runtime, bytes)
            }
        }
    }
    /// Read the block timestamp as elapsed milliseconds since the Unix epoch.
    fn timestamp(&self) -> impl Future<Output = Result<SystemTime, ClientError>> + Send {
        async move {
            let value = self.storage_dyn("Timestamp", "Now", &[]).await?;
            let millis = value
                .as_u128()
                .and_then(|value| u64::try_from(value).ok())
                .ok_or_else(|| ClientError::Protocol("Timestamp.Now is not a u64".into()))?;
            SystemTime::UNIX_EPOCH
                .checked_add(Duration::from_millis(millis))
                .ok_or_else(|| ClientError::Protocol("timestamp exceeds SystemTime range".into()))
        }
    }
    /// Decode Aura's live slot-duration constant in milliseconds.
    fn block_time(&self) -> impl Future<Output = Result<Duration, ClientError>> + Send {
        async move {
            let runtime = View::runtime(self).await?;
            let info = runtime
                .core()
                .constant("Aura", "SlotDuration")
                .ok_or_else(|| CoreError::NotInRuntime("constant Aura.SlotDuration".into()))?;
            observe(self, ReadKind::Constant, "Aura", "SlotDuration");
            let value = runtime
                .core()
                .decode_spec(
                    &rtao_core::runtime::TypeSpec::Id(info.ty),
                    &info.value,
                    true,
                )
                .map_err(CoreError::from)?;
            let millis = rtao_core::value_ext::as_u128(&value)
                .and_then(|value| u64::try_from(value).ok())
                .ok_or_else(|| ClientError::Protocol("Aura.SlotDuration is not a u64".into()))?;
            Ok(Duration::from_millis(millis))
        }
    }
    /// Read a named entry with exact Python shapes and live miss semantics.
    fn storage_dyn(
        &self,
        pallet: &str,
        item: &str,
        keys: &[Value],
    ) -> impl Future<Output = Result<DynValue, ClientError>> + Send {
        async move {
            let snapshot = View::pin(self).await?;
            let runtime = snapshot.block.runtime.clone();
            let info = runtime
                .storage_entry(pallet, item)
                .ok_or_else(|| CoreError::NotInRuntime(format!("storage {pallet}.{item}")))?
                .clone();
            if keys.len() != info.key_types.len() {
                return Err(ClientError::Protocol(format!(
                    "{pallet}.{item} expects {} key arguments, got {}",
                    info.key_types.len(),
                    keys.len()
                )));
            }
            let key = runtime
                .core()
                .storage_key(&info, keys)
                .map_err(CoreError::from)?;
            let bytes = View::storage_raw(&snapshot, &key).await?;
            let offload = bytes.as_ref().is_some_and(|bytes| bytes.len() > 64 * 1024)
                || bytes.is_none() && info.default_bytes.len() > 64 * 1024;
            let decode = move || {
                rtao_core::ops::storage::decode_storage_bytes(
                    runtime.core(),
                    &info,
                    bytes.as_deref(),
                )
                .map(DynValue::new)
                .map_err(|error| decode_error("storage", error.into()))
            };
            if offload {
                rtao_core::offload(decode).await.map_err(|error| {
                    ClientError::Protocol(format!("storage decoder task failed: {error}"))
                })?
            } else {
                decode()
            }
        }
    }
    /// Call a named runtime API with exact Python output shapes.
    fn call_dyn(
        &self,
        api: &str,
        method: &str,
        args: &[Value],
    ) -> impl Future<Output = Result<DynValue, ClientError>> + Send {
        async move {
            let snapshot = View::pin(self).await?;
            let runtime = snapshot.block.runtime.clone();
            if crate::legacy_api::is_legacy(&runtime, api, method) {
                let definition = crate::legacy_api::method(api, method).ok_or_else(|| {
                    ClientError::RuntimeApiUnavailable {
                        api: api.into(),
                        method: method.into(),
                    }
                })?;
                let encoded = definition.encode(&runtime, args)?;
                let bytes =
                    View::state_call(&snapshot, &format!("{api}_{method}"), &encoded).await?;
                let offload = bytes.len() > 64 * 1024;
                let decode = move || definition.decode(&runtime, &bytes).map(DynValue::new);
                return if offload {
                    rtao_core::offload(decode).await.map_err(|error| {
                        ClientError::Protocol(format!("legacy API decoder task failed: {error}"))
                    })?
                } else {
                    decode()
                };
            }
            let info = runtime
                .runtime_api(api, method)
                .ok_or_else(|| ClientError::RuntimeApiUnavailable {
                    api: api.into(),
                    method: method.into(),
                })?
                .clone();
            let encoded =
                rtao_core::ops::runtime_api::encode_params(runtime.core(), api, &info, args)
                    .map_err(CoreError::from)?;
            let bytes = View::state_call(&snapshot, &format!("{api}_{method}"), &encoded).await?;
            let offload = bytes.len() > 64 * 1024;
            let decode = move || {
                rtao_core::ops::runtime_api::decode_bytes(runtime.core(), &info, &bytes)
                    .map(DynValue::new)
                    .map_err(|error| decode_error("runtime API", error.into()))
            };
            if offload {
                rtao_core::offload(decode).await.map_err(|error| {
                    ClientError::Protocol(format!("API decoder task failed: {error}"))
                })?
            } else {
                decode()
            }
        }
    }
}
impl<T: View + ?Sized> ViewExt for T {}

/// Keys per map page and per coalesced storage query. Public endpoints pace
/// requests, so pages use the node's `state_getKeysPaged` maximum; Python sends
/// every key of a read in one `state_queryStorageAt`, so 1,000 stays within
/// the request shapes public nodes already serve.
const STORAGE_PAGE: usize = 1000;

fn storage_stream<'a, V: View + ?Sized, T: Send + 'static, D>(
    view: &'a V,
    pallet: &'a str,
    item: &'a str,
    prefix: Vec<Value>,
    surface: Option<rtao_runtime::StorageSurface>,
    decode: D,
) -> impl Stream<Item = Result<T, ClientError>> + Send + 'a
where
    D: Fn(
            &RuntimeHandle,
            &rtao_core::runtime::StorageInfo,
            Vec<Value>,
            Bytes,
        ) -> Result<T, ClientError>
        + Clone
        + Send
        + Sync
        + 'static,
{
    use std::collections::VecDeque;
    futures::stream::try_unfold(
        (None::<Snapshot>, None::<Vec<u8>>, VecDeque::new(), false),
        move |(snapshot, cursor, mut queued, done)| {
            let prefix = prefix.clone();
            let decode = decode.clone();
            async move {
                let snapshot = match snapshot {
                    Some(snapshot) => snapshot,
                    None => {
                        let snapshot = View::pin(view).await?;
                        if let Some(surface) = surface {
                            check_surface(
                                &snapshot.block.runtime,
                                rtao_runtime::SurfaceDescriptor::Storage(surface),
                                format!("storage:{}.{}", surface.pallet, surface.name),
                                surface.name,
                            )?;
                        }
                        snapshot
                    }
                };
                let runtime = &snapshot.block.runtime;
                let info = runtime
                    .storage_entry(pallet, item)
                    .ok_or_else(|| CoreError::NotInRuntime(format!("storage {pallet}.{item}")))?
                    .clone();
                if prefix.len() >= info.key_types.len() {
                    return Err(ClientError::Protocol(
                        "map iteration requires at least one free key component".into(),
                    ));
                }
                let encoded_prefix = runtime
                    .core()
                    .storage_key(&info, &prefix)
                    .map_err(CoreError::from)?;
                let mut cursor = cursor;
                let mut done = done;
                loop {
                    if let Some(value) = queued.pop_front() {
                        return Ok(Some((value, (Some(snapshot), cursor, queued, done))));
                    }
                    if done {
                        return Ok(None);
                    }
                    let keys = View::keys_paged(
                        &snapshot,
                        &encoded_prefix,
                        STORAGE_PAGE as u32,
                        cursor.as_deref(),
                    )
                    .await?;
                    done = keys.len() < STORAGE_PAGE;
                    if keys.is_empty() {
                        return Ok(None);
                    }
                    cursor = keys.last().cloned();
                    let values = View::storage_raw_many(&snapshot, &keys).await?;
                    let offload = keys.len() > 64
                        || values.iter().flatten().any(|value| value.len() > 64 * 1024);
                    let decode_page = {
                        let runtime = runtime.clone();
                        let info = info.clone();
                        let fixed = prefix.len();
                        let decode = decode.clone();
                        move || {
                            keys.into_iter()
                                .zip(values)
                                .filter_map(|(key, value)| value.map(|value| (key, value)))
                                .map(|(key, value)| {
                                    let free = runtime
                                        .core()
                                        .decode_storage_key_params(&info, &key, fixed)
                                        .map_err(CoreError::from)?;
                                    decode(&runtime, &info, free, value)
                                })
                                .collect::<Result<VecDeque<T>, ClientError>>()
                        }
                    };
                    queued = if offload {
                        rtao_core::offload(decode_page).await.map_err(|error| {
                            ClientError::Protocol(format!("map page decoder task failed: {error}"))
                        })??
                    } else {
                        decode_page()?
                    };
                }
            }
        },
    )
}

impl View for dyn DynView + '_ {
    fn read_observer(&self) -> Option<&dyn ReadObserver> {
        DynView::read_observer(self)
    }
    fn metagraph_view_enabled(&self) -> bool {
        DynView::metagraph_view_enabled(self)
    }
    fn pinned(&self) -> Option<&PinnedBlock> {
        DynView::pinned(self)
    }
    async fn pin(&self) -> Result<Snapshot, ClientError> {
        DynView::pin(self).await
    }
    async fn runtime(&self) -> Result<RuntimeHandle, ClientError> {
        DynView::runtime(self).await
    }
    async fn storage_raw(&self, key: &[u8]) -> Result<Option<Bytes>, ClientError> {
        DynView::storage_raw(self, key).await
    }
    async fn storage_raw_many(&self, keys: &[Vec<u8>]) -> Result<Vec<Option<Bytes>>, ClientError> {
        DynView::storage_raw_many(self, keys).await
    }
    async fn keys_paged(
        &self,
        prefix: &[u8],
        count: u32,
        start: Option<&[u8]>,
    ) -> Result<Vec<Vec<u8>>, ClientError> {
        DynView::keys_paged(self, prefix, count, start).await
    }
    async fn state_call(&self, method: &str, args: &[u8]) -> Result<Bytes, ClientError> {
        DynView::state_call(self, method, args).await
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::panic)]
mod tests {
    use super::*;
    use futures::{StreamExt, TryStreamExt};
    use parity_scale_codec::{Decode, Encode};
    use rtao_primitives::{AccountId, NetUid};
    use rtao_runtime::{constants, runtime_apis, storage};
    use serde_json::value::RawValue;
    use std::sync::{Arc, Mutex};
    use subxt_rpcs::{
        RpcClient,
        client::{RawRpcFuture, RawRpcSubscription, RpcClientT},
    };

    const HASH: H256 = H256([0x42; 32]);
    const METADATA: &[u8] = include_bytes!("../../../vendor/metadata/finney/478.v15.scale");
    #[derive(Default)]
    struct Fake {
        requests: Mutex<Vec<(String, JsonValue)>>,
        values: Mutex<BTreeMap<String, JsonValue>>,
        calls: Mutex<BTreeMap<String, JsonValue>>,
    }
    impl RpcClientT for Fake {
        fn request_raw<'a>(
            &'a self,
            method: &'a str,
            params: Option<Box<RawValue>>,
        ) -> RawRpcFuture<'a, Box<RawValue>> {
            Box::pin(async move {
                let params: JsonValue = serde_json::from_str(params.unwrap().get()).unwrap();
                self.requests
                    .lock()
                    .unwrap()
                    .push((method.into(), params.clone()));
                let result = match method {
                    "chain_getBlockHash" | "chain_getFinalizedHead" => json!(crate::hash_hex(HASH)),
                    "system_properties" => json!({"ss58Format":42}),
                    "chain_getHeader" => {
                        json!({"number":"0x43","parentHash":crate::hash_hex(HASH)})
                    }
                    "state_getRuntimeVersion" => json!({"specVersion":478,"transactionVersion":1}),
                    "state_call" if params[0] == "Metadata_metadata_at_version" => {
                        json!(hex_bytes(&Some(METADATA.to_vec()).encode()))
                    }
                    "state_call" => self
                        .calls
                        .lock()
                        .unwrap()
                        .get(params[0].as_str().unwrap())
                        .cloned()
                        .unwrap_or_else(|| panic!("unscripted runtime call {params}")),
                    "state_getStorage" => self
                        .values
                        .lock()
                        .unwrap()
                        .get(params[0].as_str().unwrap())
                        .cloned()
                        .unwrap_or(JsonValue::Null),
                    "state_queryStorageAt" => {
                        let values = self.values.lock().unwrap();
                        let changes = params[0]
                            .as_array()
                            .unwrap()
                            .iter()
                            .rev()
                            .map(|key| {
                                json!([
                                    key,
                                    values
                                        .get(key.as_str().unwrap())
                                        .cloned()
                                        .unwrap_or(JsonValue::Null)
                                ])
                            })
                            .collect::<Vec<_>>();
                        json!([{"block":params[1],"changes":changes}])
                    }
                    "state_getKeysPaged" => {
                        let prefix = params[0].as_str().unwrap();
                        let count = params[1].as_u64().unwrap() as usize;
                        let start = params[2].as_str();
                        let keys = self
                            .values
                            .lock()
                            .unwrap()
                            .iter()
                            .filter(|(key, value)| {
                                !value.is_null()
                                    && key.starts_with(prefix)
                                    && start.is_none_or(|start| key.as_str() > start)
                            })
                            .take(count)
                            .map(|(key, _)| json!(key))
                            .collect::<Vec<_>>();
                        json!(keys)
                    }
                    _ => panic!("unscripted request {method}"),
                };
                Ok(serde_json::value::to_raw_value(&result).unwrap())
            })
        }
        fn subscribe_raw<'a>(
            &'a self,
            _: &'a str,
            _: Option<Box<RawValue>>,
            _: &'a str,
        ) -> RawRpcFuture<'a, RawRpcSubscription> {
            Box::pin(async {
                Err(subxt_rpcs::Error::Client(Box::new(
                    rtao_rpc::RpcError::Protocol("no subscriptions in view fixture".into()),
                )))
            })
        }
    }
    async fn fixture(runtime: RuntimeHandle) -> (Snapshot, Arc<Fake>) {
        fixture_with_metagraph(runtime, true).await
    }
    async fn fixture_with_metagraph(
        runtime: RuntimeHandle,
        metagraph_view: bool,
    ) -> (Snapshot, Arc<Fake>) {
        let fake = Arc::new(Fake::default());
        let network = crate::Network::resolve_with("ws://127.0.0.1:9944", |_| None).unwrap();
        let mut config = crate::ClientConfig::new(network).unwrap();
        config.runtime_cache_dir = None;
        config.config_store = None;
        config.track_heads = false;
        config.metagraph_view = metagraph_view;
        let client = Client::from_rpc(RpcClient::new(fake.clone()), config)
            .await
            .unwrap();
        fake.requests.lock().unwrap().clear();
        (
            Snapshot::new(
                client,
                PinnedBlock {
                    number: 67,
                    hash: HASH,
                    runtime,
                },
            ),
            fake,
        )
    }
    fn current_core() -> rtao_core::runtime::Runtime {
        rtao_core::runtime::Runtime::parse(METADATA, 478, 1, 42).unwrap()
    }
    fn current() -> RuntimeHandle {
        RuntimeHandle::parse(METADATA, 478, 1, 42).unwrap()
    }

    #[derive(Debug, Default)]
    struct Observer(Mutex<Vec<ReadTouch>>);
    impl ReadObserver for Observer {
        fn on_read(&self, touch: ReadTouch) {
            self.0.lock().unwrap().push(touch);
        }
    }

    #[tokio::test]
    async fn borrowed_views_forward_identity_options_and_all_reads_after_erasure() {
        let (snapshot, fake) = fixture_with_metagraph(current(), false).await;
        let observer = Arc::new(Observer::default());
        let snapshot = snapshot.with_read_observer(observer.clone());
        let runtime = &snapshot.block.runtime;
        let key = storage_key(
            runtime,
            storage::subtensor_module::TEMPO.surface(),
            &[Value::Int(1)],
        )
        .unwrap();
        let prefix = runtime
            .core()
            .storage_key(
                runtime.storage_entry("SubtensorModule", "Tempo").unwrap(),
                &[],
            )
            .unwrap();
        let bytes = Bytes::from(100u16.encode());
        fake.values
            .lock()
            .unwrap()
            .insert(hex_bytes(&key), json!(hex_bytes(&bytes)));
        fake.calls.lock().unwrap().insert(
            "AccountNonceApi_account_nonce".into(),
            json!(hex_bytes(&7u32.encode())),
        );
        let concrete = &snapshot;
        let dynamic: &dyn DynView = &snapshot;
        let views: [&dyn DynView; 2] = [&concrete, &dynamic];
        let keys = vec![key.clone(), vec![0], key.clone()];
        let args = [7; 32];
        let mut expected_requests: Vec<(String, JsonValue)> = Vec::new();
        for view in views {
            assert!(std::ptr::eq(
                DynView::pinned(view).unwrap(),
                snapshot.block()
            ));
            assert!(std::ptr::addr_eq(
                DynView::read_observer(view).unwrap(),
                observer.as_ref() as &dyn ReadObserver
            ));
            assert!(!DynView::metagraph_view_enabled(view));
            let resolved = DynView::runtime(view).await.unwrap();
            assert!(std::ptr::eq(resolved.entry(), runtime.entry()));
            let pinned = DynView::pin(view).await.unwrap();
            assert_eq!(pinned.block.hash, HASH);
            assert!(!View::metagraph_view_enabled(&pinned));
            assert!(std::ptr::addr_eq(
                View::read_observer(&pinned).unwrap(),
                observer.as_ref() as &dyn ReadObserver
            ));
            assert_eq!(
                DynView::storage_raw(view, &key).await.unwrap(),
                Some(bytes.clone())
            );
            assert_eq!(
                DynView::storage_raw_many(view, &keys).await.unwrap(),
                vec![Some(bytes.clone()), None, Some(bytes.clone())]
            );
            assert_eq!(
                DynView::keys_paged(view, &prefix, 1, None).await.unwrap(),
                vec![key.clone()]
            );
            assert!(
                DynView::keys_paged(view, &prefix, 2, Some(&key))
                    .await
                    .unwrap()
                    .is_empty()
            );
            assert_eq!(
                DynView::state_call(view, "AccountNonceApi_account_nonce", &args)
                    .await
                    .unwrap(),
                Bytes::from(7u32.encode())
            );
            let hash = crate::hash_hex(HASH);
            expected_requests.extend([
                ("state_getStorage".into(), json!([hex_bytes(&key), hash])),
                (
                    "state_queryStorageAt".into(),
                    json!([[hex_bytes(&key), "0x00"], hash]),
                ),
                (
                    "state_getKeysPaged".into(),
                    json!([hex_bytes(&prefix), 1, null, hash]),
                ),
                (
                    "state_getKeysPaged".into(),
                    json!([hex_bytes(&prefix), 2, hex_bytes(&key), hash]),
                ),
                (
                    "state_call".into(),
                    json!(["AccountNonceApi_account_nonce", hex_bytes(&args), hash]),
                ),
            ]);
        }
        assert_eq!(*fake.requests.lock().unwrap(), expected_requests);
        let touches = observer.0.lock().unwrap();
        assert_eq!(touches.len(), 10);
        assert!(touches.iter().all(|touch| touch.at == Some(HASH)));
        assert_eq!(
            touches
                .iter()
                .filter(|touch| touch.kind == ReadKind::RuntimeApi)
                .count(),
            2
        );
    }

    #[tokio::test]
    async fn observer_survives_pin_clone_and_dynamic_erasure() {
        let (snapshot, fake) = fixture(current()).await;
        let observer = Arc::new(Observer::default());
        let snapshot = snapshot.with_read_observer(observer.clone());
        let cloned = View::pin(&snapshot).await.unwrap();
        let erased: &dyn DynView = &cloned;
        assert_eq!(erased.block_time().await.unwrap(), Duration::from_secs(12));
        assert_eq!(
            erased
                .constant(&constants::system::SS58_PREFIX)
                .await
                .unwrap(),
            42
        );
        let _ = erased.timestamp().await.unwrap();
        let _ = erased
            .fetch((
                storage::subtensor_module::TEMPO.at((NetUid(1),)),
                storage::subtensor_module::TEMPO.at((NetUid(2),)),
            ))
            .await
            .unwrap();
        let empty = erased
            .storage_iter(&storage::subtensor_module::KEYS, (NetUid(99),))
            .try_collect::<Vec<_>>()
            .await
            .unwrap();
        assert!(empty.is_empty());
        fake.calls.lock().unwrap().insert(
            "AccountNonceApi_account_nonce".into(),
            json!(hex_bytes(&7u32.encode())),
        );
        assert_eq!(
            erased
                .call_dyn(
                    "AccountNonceApi",
                    "account_nonce",
                    &[Value::Str(AccountId([7; 32]).to_ss58(42))],
                )
                .await
                .unwrap()
                .to_json(),
            json!(7)
        );
        let touches = observer.0.lock().unwrap();
        assert!(touches.iter().all(|touch| touch.at == Some(HASH)));
        let identities = touches
            .iter()
            .map(|touch| {
                (
                    touch.kind,
                    touch.pallet_or_api.as_str(),
                    touch.item.as_str(),
                )
            })
            .collect::<BTreeSet<_>>();
        assert_eq!(
            identities,
            BTreeSet::from([
                (ReadKind::Constant, "Aura", "SlotDuration"),
                (ReadKind::Constant, "System", "SS58Prefix"),
                (ReadKind::Storage, "Timestamp", "Now"),
                (ReadKind::Storage, "SubtensorModule", "Tempo"),
                (ReadKind::Storage, "SubtensorModule", "Keys"),
                (ReadKind::RuntimeApi, "AccountNonceApi", "account_nonce"),
            ])
        );
        drop(touches);
        assert_state_pinned(&fake);
    }
    fn golden() -> JsonValue {
        serde_json::from_str(include_str!("../../../vendor/upstream/golden.json")).unwrap()
    }
    fn golden_runtime() -> RuntimeHandle {
        let golden = golden();
        let wrapped = hex::decode(
            golden["metadata"]["v15_hex"]
                .as_str()
                .unwrap()
                .trim_start_matches("0x"),
        )
        .unwrap();
        let bytes = Option::<Vec<u8>>::decode(&mut wrapped.as_slice())
            .unwrap()
            .unwrap();
        RuntimeHandle::parse(&bytes, 461, 1, 42).unwrap()
    }
    fn core(value: &JsonValue) -> Value {
        match value {
            JsonValue::Null => Value::Null,
            JsonValue::Bool(v) => Value::Bool(*v),
            JsonValue::String(v) => Value::Str(v.clone()),
            JsonValue::Number(v) => Value::Int(v.as_i128().unwrap()),
            JsonValue::Array(v) => Value::List(v.iter().map(core).collect()),
            _ => panic!("unsupported fixture argument"),
        }
    }
    fn assert_state_pinned(fake: &Fake) {
        for (method, args) in fake.requests.lock().unwrap().iter() {
            if matches!(
                method.as_str(),
                "state_getStorage" | "state_queryStorageAt" | "state_getKeysPaged" | "state_call"
            ) {
                assert_eq!(
                    args.as_array().unwrap().last(),
                    Some(&json!(crate::hash_hex(HASH))),
                    "{method}"
                );
            }
        }
    }
    #[tokio::test]
    async fn dynamic_storage_and_runtime_calls_reproduce_every_golden() {
        let (snapshot, fake) = fixture(golden_runtime()).await;
        let golden = golden();
        for item in golden["storage_values"].as_array().unwrap() {
            fake.values.lock().unwrap().insert(
                item["key_hex"].as_str().unwrap().into(),
                item["raw_hex"].clone(),
            );
        }
        for item in golden["storage_values"].as_array().unwrap() {
            let value = snapshot
                .storage_dyn(
                    item["pallet"].as_str().unwrap(),
                    item["storage_function"].as_str().unwrap(),
                    &item["params"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .map(core)
                        .collect::<Vec<_>>(),
                )
                .await
                .unwrap();
            assert_eq!(value.to_json(), item["decoded"]);
        }
        for item in golden["runtime_calls"].as_array().unwrap() {
            fake.calls.lock().unwrap().insert(
                format!(
                    "{}_{}",
                    item["api"].as_str().unwrap(),
                    item["method"].as_str().unwrap()
                ),
                item["raw_hex"].clone(),
            );
            let value = snapshot
                .call_dyn(
                    item["api"].as_str().unwrap(),
                    item["method"].as_str().unwrap(),
                    &item["params"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .map(core)
                        .collect::<Vec<_>>(),
                )
                .await
                .unwrap();
            assert_eq!(value.to_json(), item["decoded"]);
            let requests = fake.requests.lock().unwrap();
            let params = &requests.last().unwrap().1;
            assert_eq!(
                params[1].as_str().unwrap().trim_start_matches("0x"),
                item["encoded_params_hex"].as_str().unwrap()
            );
        }
        assert_state_pinned(&fake);
    }
    #[tokio::test]
    async fn typed_golden_constants_use_snapshot_runtime_without_rpc() {
        let (snapshot, fake) = fixture(golden_runtime()).await;
        assert_eq!(
            snapshot
                .constant(&constants::balances::EXISTENTIAL_DEPOSIT)
                .await
                .unwrap()
                .0,
            500
        );
        assert_eq!(
            snapshot
                .constant(&constants::aura::SLOT_DURATION)
                .await
                .unwrap(),
            250
        );
        assert_eq!(
            snapshot
                .constant(&constants::system::SS58_PREFIX)
                .await
                .unwrap(),
            42
        );
        assert!(fake.requests.lock().unwrap().is_empty());
        let mut core = current_core();
        core.pallets
            .iter_mut()
            .find(|p| p.name == "Aura")
            .unwrap()
            .constants
            .iter_mut()
            .find(|c| c.name == "SlotDuration")
            .unwrap()
            .value = 501u64.encode();
        let (snapshot, fake) = fixture(RuntimeHandle::new(core).unwrap()).await;
        assert_eq!(
            snapshot
                .constant(&constants::aura::SLOT_DURATION)
                .await
                .unwrap(),
            501
        );
        assert_eq!(
            snapshot.block_time().await.unwrap(),
            Duration::from_millis(501)
        );
        assert!(fake.requests.lock().unwrap().is_empty());
    }
    #[tokio::test]
    async fn typed_fetch_coalesces_mixed_defaults_optional_misses_and_duplicates() {
        let (snapshot, fake) = fixture(current()).await;
        let tempo = storage::subtensor_module::TEMPO;
        let key = snapshot
            .block
            .runtime
            .core()
            .storage_key(
                snapshot
                    .block
                    .runtime
                    .storage_entry("SubtensorModule", "Tempo")
                    .unwrap(),
                &[Value::Int(1)],
            )
            .unwrap();
        fake.values
            .lock()
            .unwrap()
            .insert(hex_bytes(&key), json!(hex_bytes(&100u16.encode())));
        let (actual, present, missing, uid, duplicate) = snapshot
            .fetch((
                tempo.at((NetUid(1),)),
                storage::subtensor_module::NETWORKS_ADDED.at((NetUid(1),)),
                storage::subtensor_module::NETWORKS_ADDED.at((NetUid(2),)),
                storage::subtensor_module::UIDS.at((NetUid(1), AccountId([7; 32]))),
                tempo.at((NetUid(1),)),
            ))
            .await
            .unwrap();
        assert_eq!(
            (actual, present, missing, uid, duplicate),
            (100, false, false, None, 100)
        );
        let requests = fake.requests.lock().unwrap();
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0].0, "state_queryStorageAt");
        assert_eq!(requests[0].1[0].as_array().unwrap().len(), 4);
        drop(requests);
        assert_state_pinned(&fake);
    }
    #[tokio::test]
    async fn fetch_chunks_eleven_hundred_keys_and_uses_live_hashers() {
        let mut runtime = current_core();
        runtime
            .pallets
            .iter_mut()
            .find(|p| p.name == "SubtensorModule")
            .unwrap()
            .storage
            .iter_mut()
            .find(|s| s.name == "Tempo")
            .unwrap()
            .hashers = vec!["Twox64Concat".into()];
        let (snapshot, fake) = fixture(RuntimeHandle::new(runtime).unwrap()).await;
        let info = snapshot
            .block
            .runtime
            .storage_entry("SubtensorModule", "Tempo")
            .unwrap();
        let mut plan = ReadPlan::new();
        for index in 0..1100u16 {
            let args = vec![Value::Int(i128::from(index))];
            let key = snapshot
                .block
                .runtime
                .core()
                .storage_key(info, &args)
                .unwrap();
            assert_eq!(key.len(), 42);
            fake.values
                .lock()
                .unwrap()
                .insert(hex_bytes(&key), json!(hex_bytes(&index.encode())));
            plan.push(storage::subtensor_module::TEMPO.surface(), args);
        }
        let values = snapshot.fetch(plan).await.unwrap();
        assert_eq!(values.len(), 1100);
        for (index, value) in values.into_iter().enumerate() {
            assert_eq!(value.as_u128(), Some(index as u128));
        }
        let requests = fake.requests.lock().unwrap();
        assert_eq!(requests.len(), 2);
        assert_eq!(requests[0].1[0].as_array().unwrap().len(), 1000);
        assert_eq!(requests[1].1[0].as_array().unwrap().len(), 100);
        drop(requests);
        assert_state_pinned(&fake);
    }
    #[tokio::test]
    async fn typed_map_pages_pin_once_and_decode_reversible_free_keys() {
        let (snapshot, fake) = fixture(current()).await;
        let info = snapshot
            .block
            .runtime
            .storage_entry("SubtensorModule", "Tempo")
            .unwrap();
        for index in 0..1100u16 {
            let key = snapshot
                .block
                .runtime
                .core()
                .storage_key(info, &[Value::Int(i128::from(index))])
                .unwrap();
            fake.values
                .lock()
                .unwrap()
                .insert(hex_bytes(&key), json!(hex_bytes(&index.encode())));
        }
        let stream = snapshot.storage_iter(&storage::subtensor_module::TEMPO, ());
        let values = stream.try_collect::<Vec<_>>().await.unwrap();
        assert_eq!(values.len(), 1100);
        for (keys, value) in values {
            assert_eq!(keys.len(), 1);
            assert_eq!(keys[0].as_u128(), Some(u128::from(value)));
        }
        assert_eq!(
            fake.requests
                .lock()
                .unwrap()
                .iter()
                .filter(|(method, _)| method == "state_getKeysPaged")
                .count(),
            2
        );
        assert_state_pinned(&fake);
    }
    #[tokio::test]
    async fn typed_map_accepts_tuple_prefix_and_skips_absent_values() {
        let (snapshot, fake) = fixture(current()).await;
        let info = snapshot
            .block
            .runtime
            .storage_entry("SubtensorModule", "Keys")
            .unwrap();
        let account = AccountId([7; 32]);
        for (netuid, uid) in [(1, 2), (1, 3), (2, 4)] {
            let key = snapshot
                .block
                .runtime
                .core()
                .storage_key(info, &[Value::Int(netuid), Value::Int(uid)])
                .unwrap();
            let value = if uid == 3 {
                JsonValue::Null
            } else {
                json!(hex_bytes(account.as_bytes()))
            };
            fake.values.lock().unwrap().insert(hex_bytes(&key), value);
        }
        let rows = snapshot
            .storage_iter(&storage::subtensor_module::KEYS, (NetUid(1),))
            .try_collect::<Vec<_>>()
            .await
            .unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].0[0].as_u128(), Some(2));
        assert_eq!(rows[0].1.raw(), &account);
        assert_state_pinned(&fake);
    }
    #[tokio::test]
    async fn all_query_map_goldens_follow_live_prefix_and_free_keys() {
        let (snapshot, fake) = fixture(golden_runtime()).await;
        for item in golden()["query_maps"].as_array().unwrap() {
            fake.values.lock().unwrap().clear();
            for change in item["raw_changes"].as_array().unwrap() {
                fake.values
                    .lock()
                    .unwrap()
                    .insert(change[0].as_str().unwrap().into(), change[1].clone());
            }
            let stream = snapshot.storage_iter_dyn(
                item["pallet"].as_str().unwrap(),
                item["storage_function"].as_str().unwrap(),
                item["params"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(core)
                    .collect(),
            );
            let rows = stream
                .map(|row| {
                    let (keys, bytes, runtime, info) = row.unwrap();
                    let key = if keys.len() == 1 {
                        keys.into_iter().next().unwrap()
                    } else {
                        Value::Tuple(keys)
                    };
                    let value = rtao_core::ops::storage::decode_storage_bytes(
                        runtime.core(),
                        &info,
                        Some(&bytes),
                    )
                    .unwrap();
                    DynValue::new(Value::Tuple(vec![key, value])).to_json()
                })
                .collect::<Vec<_>>()
                .await;
            assert_eq!(json!(rows), item["pairs"]);
        }
        assert_state_pinned(&fake);
    }
    #[tokio::test]
    async fn api_versions_refuse_before_rpc_and_allow_verified_newer_versions() {
        let mut versions = BTreeMap::new();
        versions.insert("AccountNonceApi".into(), 0);
        let (snapshot, fake) = fixture(
            RuntimeHandle::new_with_api_versions(current_core(), versions.clone()).unwrap(),
        )
        .await;
        let error = snapshot
            .call(
                &runtime_apis::account_nonce_api::ACCOUNT_NONCE,
                (AccountId([7; 32]),),
            )
            .await
            .unwrap_err();
        assert!(matches!(
            error,
            ClientError::ApiVersion {
                need: 1,
                have: 0,
                ..
            }
        ));
        assert!(fake.requests.lock().unwrap().is_empty());
        versions.insert("AccountNonceApi".into(), 2);
        let (snapshot, fake) =
            fixture(RuntimeHandle::new_with_api_versions(current_core(), versions).unwrap()).await;
        fake.calls.lock().unwrap().insert(
            "AccountNonceApi_account_nonce".into(),
            json!(hex_bytes(&7u32.encode())),
        );
        assert_eq!(
            snapshot
                .call(
                    &runtime_apis::account_nonce_api::ACCOUNT_NONCE,
                    (AccountId([7; 32]),)
                )
                .await
                .unwrap(),
            7
        );
        assert_state_pinned(&fake);
    }
    #[test]
    fn malformed_change_sets_fail_without_reordering_or_inventing_misses() {
        let keys = vec![vec![1], vec![2]];
        let valid =
            json!([{"block":crate::hash_hex(HASH),"changes":[["0x02",null],["0x01","0x0a"]]}]);
        assert_eq!(
            decode_changes(&valid, &keys, HASH).unwrap(),
            vec![Some(Bytes::from_static(&[10])), None]
        );
        for malformed in [
            json!([{"block":crate::hash_hex(H256([0;32])),"changes":[]}]),
            json!([{"block":crate::hash_hex(HASH),"changes":[["0x01",null]]}]),
            json!([{"block":crate::hash_hex(HASH),"changes":[["0x01",null],["0x01",null],["0x02",null]]}]),
            json!([{"block":crate::hash_hex(HASH),"changes":[["0x03",null]]}]),
        ] {
            assert!(decode_changes(&malformed, &keys, HASH).is_err());
        }
    }
    #[tokio::test]
    async fn dyn_view_dispatch_preserves_snapshot_identity() {
        let (snapshot, fake) = fixture(current()).await;
        let dynamic: &dyn DynView = &snapshot;
        assert_eq!(DynView::pinned(dynamic).unwrap().hash, HASH);
        let pinned = DynView::pin(dynamic).await.unwrap();
        assert_eq!(pinned.block.hash, HASH);
        assert_eq!(
            DynView::storage_raw_many(dynamic, &[]).await.unwrap(),
            Vec::<Option<Bytes>>::new()
        );
        assert!(fake.requests.lock().unwrap().is_empty());
    }
    fn v15_byte_output_runtime() -> RuntimeHandle {
        // Rewrite and reparse actual V15 bytes, retaining their portable registry.
        let byte_output = current_core().type_id_of("Vec<u8>").unwrap();
        let mut metadata =
            frame_metadata::RuntimeMetadataPrefixed::decode(&mut &METADATA[..]).unwrap();
        let frame_metadata::RuntimeMetadata::V15(v15) = &mut metadata.1 else {
            panic!("fixture must be V15")
        };
        let api = v15
            .apis
            .iter_mut()
            .find(|api| api.name == "StakeInfoRuntimeApi")
            .unwrap();
        let method = api
            .methods
            .iter_mut()
            .find(|method| method.name == "get_stake_info_for_coldkey")
            .unwrap();
        // This changes only the output. Its modern AccountId input must no longer
        // control encoding once a byte-vector output selects the frozen contract.
        method.output.id = byte_output;
        let mut unknown = method.clone();
        unknown.name = "unknown_archive_bytes".into();
        api.methods.push(unknown);
        RuntimeHandle::parse(&metadata.encode(), 478, 1, 42).unwrap()
    }

    #[tokio::test]
    async fn v15_byte_output_uses_frozen_parameters_and_double_encoded_stake_rows() {
        use parity_scale_codec::Compact;
        let runtime = v15_byte_output_runtime();
        assert_eq!(runtime.metadata_level(), rtao_runtime::MetadataLevel::V15);
        let info = runtime
            .runtime_api("StakeInfoRuntimeApi", "get_stake_info_for_coldkey")
            .unwrap();
        assert_eq!(runtime.core().type_ident(info.output), "Vec<u8>");
        let (snapshot, fake) = fixture(runtime).await;
        let inner = (
            Compact(2u32),
            [7u8; 32],
            [8u8; 32],
            Compact(u64::MAX),
            [9u8; 32],
            [10u8; 32],
            Compact(64u64),
        )
            .encode();
        let outer = inner.encode();
        assert_ne!(inner, outer);
        fake.calls.lock().unwrap().insert(
            "StakeInfoRuntimeApi_get_stake_info_for_coldkey".into(),
            json!(hex_bytes(&outer)),
        );
        let output = snapshot
            .call_dyn(
                "StakeInfoRuntimeApi",
                "get_stake_info_for_coldkey",
                &[Value::Str(AccountId([8; 32]).to_ss58(42))],
            )
            .await
            .unwrap();
        assert_eq!(
            output.to_json(),
            json!([
                {"netuid":0,"hotkey":AccountId([7;32]).to_ss58(42),"coldkey":AccountId([8;32]).to_ss58(42),"stake":u64::MAX,"locked":0,"emission":0,"drain":0,"is_registered":false},
                {"netuid":0,"hotkey":AccountId([9;32]).to_ss58(42),"coldkey":AccountId([10;32]).to_ss58(42),"stake":64,"locked":0,"emission":0,"drain":0,"is_registered":false}
            ])
        );
        assert_eq!(
            *fake.requests.lock().unwrap(),
            vec![(
                "state_call".into(),
                json!([
                    "StakeInfoRuntimeApi_get_stake_info_for_coldkey",
                    hex_bytes(&vec![8u8; 32].encode()),
                    crate::hash_hex(HASH)
                ])
            )]
        );
    }

    #[tokio::test]
    async fn v15_unknown_byte_output_is_refused_before_rpc() {
        let runtime = v15_byte_output_runtime();
        let info = runtime
            .runtime_api("StakeInfoRuntimeApi", "unknown_archive_bytes")
            .unwrap();
        assert_eq!(runtime.core().type_ident(info.output), "Vec<u8>");
        let (snapshot, fake) = fixture(runtime).await;
        assert!(
            matches!(snapshot.call_dyn("StakeInfoRuntimeApi", "unknown_archive_bytes", &[Value::Str(AccountId([8;32]).to_ss58(42))]).await,
            Err(ClientError::RuntimeApiUnavailable { api, method }) if api == "StakeInfoRuntimeApi" && method == "unknown_archive_bytes")
        );
        assert!(fake.requests.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn v14_archive_calls_use_only_frozen_double_encoded_api_definitions() {
        let golden = golden();
        let wrapped = hex::decode(
            golden["metadata"]["v14_hex"]
                .as_str()
                .unwrap()
                .trim_start_matches("0x"),
        )
        .unwrap();
        // state_getMetadata supplies bare V14, while the fixture may retain its Option wrapper.
        let bytes = if wrapped.starts_with(b"meta") {
            wrapped
        } else {
            Option::<Vec<u8>>::decode(&mut wrapped.as_slice())
                .unwrap()
                .unwrap()
        };
        let runtime = RuntimeHandle::parse(&bytes, 461, 1, 42).unwrap();
        assert_eq!(
            runtime.metadata_level(),
            rtao_runtime::MetadataLevel::V14Degraded
        );
        let (snapshot, fake) = fixture(runtime).await;
        let method = "StakeInfoRuntimeApi_get_stake_info_for_coldkey";
        fake.calls
            .lock()
            .unwrap()
            .insert(method.into(), json!(hex_bytes(&vec![0u8].encode())));
        let output = snapshot
            .call_dyn(
                "StakeInfoRuntimeApi",
                "get_stake_info_for_coldkey",
                &[Value::Str(AccountId([7; 32]).to_ss58(42))],
            )
            .await
            .unwrap();
        assert_eq!(output.to_json(), json!([]));
        assert_eq!(
            fake.requests.lock().unwrap().last().unwrap().1[1],
            json!(hex_bytes(&vec![7u8; 32].encode()))
        );
        let before = fake.requests.lock().unwrap().len();
        assert!(matches!(
            snapshot
                .call_dyn(
                    "AccountNonceApi",
                    "account_nonce",
                    &[Value::Str(AccountId([7; 32]).to_ss58(42))]
                )
                .await,
            Err(ClientError::RuntimeApiUnavailable { .. })
        ));
        assert_eq!(fake.requests.lock().unwrap().len(), before);
        assert_state_pinned(&fake);
    }
}
