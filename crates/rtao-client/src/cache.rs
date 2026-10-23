// Derived from RaoFoundation/subtensor sdk/python/bittensor/_transport/runtime.py @ 850f2755bb05a3851f948489f0ab27b0645cebce (Apache-2.0); modified by rtao.
//! Parent-runtime resolution and Python-compatible metadata caches.

use std::{
    collections::{BTreeMap, VecDeque},
    fs::{self, OpenOptions},
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    time::{Duration, SystemTime},
};

use blake2::{Blake2b, Digest, digest::consts::U8};
use futures::{
    FutureExt,
    future::{BoxFuture, Shared},
};
use parity_scale_codec::Decode;
use rtao_catalog::runtime_py::{
    DISK_CACHE_MAX_FILES, DISK_FORMAT_VERSION, DISK_MAGIC, V15_MISSING_NEEDLE, V15_VERSION_HEX,
};
use rtao_core::runtime::Runtime;
use rtao_primitives::H256;
use rtao_runtime::RuntimeHandle;
use serde_json::{Value, json};
use subxt_rpcs::RpcClient;
use tokio::{sync::Mutex, time::Instant};

use crate::{ClientError, rpc_request};

const HEAD_TTL: Duration = Duration::from_secs(12);
const RUNTIME_CACHE_SIZE: usize = 4;
const BLOCK_CACHE_SIZE: usize = 512;
const MAX_OFFLINE_METADATA_BYTES: u64 = 64 * 1024 * 1024;
static NEXT_TEMP: AtomicU64 = AtomicU64::new(0);

type RuntimeKey = (H256, u32, u32);
type RuntimeFetch = Shared<BoxFuture<'static, Result<RuntimeHandle, ClientError>>>;
type VersionFetch = Shared<BoxFuture<'static, Result<RuntimeInfo, ClientError>>>;

#[derive(Debug)]
struct Lru<K, V> {
    entries: VecDeque<(K, V)>,
    capacity: usize,
}

impl<K: Eq, V: Clone> Lru<K, V> {
    fn new(capacity: usize) -> Self {
        Self {
            entries: VecDeque::new(),
            capacity,
        }
    }

    fn get(&mut self, key: &K) -> Option<V> {
        let position = self
            .entries
            .iter()
            .position(|(candidate, _)| candidate == key)?;
        let item = self.entries.remove(position)?;
        let value = item.1.clone();
        self.entries.push_back(item);
        Some(value)
    }

    fn insert(&mut self, key: K, value: V) {
        if let Some(position) = self
            .entries
            .iter()
            .position(|(candidate, _)| candidate == &key)
        {
            self.entries.remove(position);
        }
        self.entries.push_back((key, value));
        while self.entries.len() > self.capacity {
            self.entries.pop_front();
        }
    }
}

struct CacheState {
    runtimes: Lru<RuntimeKey, RuntimeHandle>,
    blocks: Lru<H256, RuntimeKey>,
    inflight: BTreeMap<RuntimeKey, RuntimeFetch>,
    /// Concurrent lookups of one state's runtime version share a request.
    versions: BTreeMap<H256, VersionFetch>,
    head: Option<(RuntimeHandle, Instant)>,
    head_generation: u64,
    /// A live runtime-version subscription keeps the head current, so the TTL
    /// recheck is unnecessary while it runs.
    head_followed: bool,
}

struct FetchContext {
    rpc: RpcClient,
    genesis: H256,
    ss58_format: u16,
    disk_dir: Option<PathBuf>,
}

struct Inner {
    context: Arc<FetchContext>,
    state: Mutex<CacheState>,
}

/// Metadata cached per chain, runtime spec and transaction version.
///
/// Clones share four parsed runtimes, 512 parent-selected block-to-runtime
/// entries and in-flight fetches. Same-spec metadata is reused across transaction
/// versions. Files and parsing run on blocking workers. A disabled disk cache
/// performs no filesystem work.
#[derive(Clone)]
pub struct RuntimeCache(Arc<Inner>);

impl std::fmt::Debug for RuntimeCache {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("RuntimeCache")
            .field("genesis", &self.0.context.genesis)
            .field("ss58_format", &self.0.context.ss58_format)
            .field("disk_dir", &self.0.context.disk_dir)
            .finish_non_exhaustive()
    }
}

impl RuntimeCache {
    /// Create a cache for one validated chain identity.
    ///
    /// `disk_dir = None` disables disk caching. To use Python's directory,
    /// explicitly pass [`Self::default_disk_dir`].
    pub fn new(rpc: RpcClient, genesis: H256, ss58_format: u16, disk_dir: Option<PathBuf>) -> Self {
        Self(Arc::new(Inner {
            context: Arc::new(FetchContext {
                rpc,
                genesis,
                ss58_format,
                disk_dir,
            }),
            state: Mutex::new(CacheState {
                runtimes: Lru::new(RUNTIME_CACHE_SIZE),
                blocks: Lru::new(BLOCK_CACHE_SIZE),
                inflight: BTreeMap::new(),
                versions: BTreeMap::new(),
                head: None,
                head_generation: 0,
                head_followed: false,
            }),
        }))
    }

    /// Immutable chain identity selecting this cache's metadata files.
    pub fn genesis(&self) -> H256 {
        self.0.context.genesis
    }

    /// Resolve an already cached spec for offline decoding, without any RPC.
    ///
    /// The most recently used matching in-memory handle wins. A disk-only handle uses its
    /// stored transaction version and has no verified runtime API versions; it
    /// is returned for decoding without replacing live or historical handles.
    /// Missing or corrupt files fail without rewriting or deleting them.
    pub async fn cached_runtime(&self, spec_version: u32) -> Result<RuntimeHandle, ClientError> {
        {
            let state = self.0.state.lock().await;
            if let Some((_, runtime)) = state
                .runtimes
                .entries
                .iter()
                .rev()
                .find(|(key, _)| key.0 == self.genesis() && key.1 == spec_version)
            {
                return Ok(runtime.clone());
            }
        }
        let context = self.0.context.clone();
        tokio::task::spawn_blocking(move || {
            let missing = || {
                ClientError::Protocol(format!(
                    "runtime metadata for spec {spec_version} is not cached"
                ))
            };
            let directory = context.disk_dir.as_ref().ok_or_else(missing)?;
            let path = disk_path(directory, context.genesis, spec_version);
            let file = open_offline_metadata(&path).map_err(|error| {
                if error.kind() == std::io::ErrorKind::InvalidData {
                    ClientError::Protocol(error.to_string())
                } else {
                    missing()
                }
            })?;
            let mut data = Vec::new();
            file.take(MAX_OFFLINE_METADATA_BYTES + 1)
                .read_to_end(&mut data)
                .map_err(|_| missing())?;
            if data.len() as u64 > MAX_OFFLINE_METADATA_BYTES
                || data.get(8).is_some_and(|flag| *flag > 1)
            {
                return Err(missing());
            }
            let (metadata, transaction_version) = decode_disk(&data).ok_or_else(missing)?;
            parse_runtime_blocking(
                &metadata,
                RuntimeInfo {
                    spec_version,
                    transaction_version,
                    api_versions: BTreeMap::new(),
                },
                context.ss58_format,
            )
        })
        .await
        .map_err(worker_error)?
    }

    /// Resolve Python's cache directory from its override or the home directory.
    pub fn default_disk_dir() -> Option<PathBuf> {
        if let Some(directory) =
            std::env::var_os("BITTENSOR_RUNTIME_CACHE_DIR").filter(|value| !value.is_empty())
        {
            return Some(directory.into());
        }
        std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".bittensor/runtime-cache"))
    }

    /// The runtime that executed a block, or the current head runtime for `None`.
    ///
    /// Historical blocks resolve through their parent's runtime version.
    /// Genesis resolves through itself because its parent hash is zero.
    pub async fn runtime_at(&self, block: Option<H256>) -> Result<RuntimeHandle, ClientError> {
        let Some(block) = block else {
            let state = self.0.state.lock().await;
            if let Some((runtime, checked_at)) = &state.head
                && (state.head_followed || checked_at.elapsed() < HEAD_TTL)
            {
                return Ok(runtime.clone());
            }
            // Do not hold the state lock across a network request.
            drop(state);
            return self.refresh_head().await;
        };
        {
            let mut state = self.0.state.lock().await;
            if let Some(key) = state.blocks.get(&block)
                && let Some(runtime) = state.runtimes.get(&key)
            {
                return Ok(runtime);
            }
        }
        let header = rpc_request(
            &self.0.context.rpc,
            "chain_getHeader",
            json!([hash_hex(block)]),
        )
        .await?;
        if header.is_null() {
            return Err(ClientError::BlockNotFound(block));
        }
        let parent = parse_hash(
            header.get("parentHash").unwrap_or(&Value::Null),
            "parentHash",
        )?;
        self.runtime_at_parent(block, parent).await
    }

    /// [`Self::runtime_at`] for a block whose parent is already known, such as
    /// a tracked head, without re-reading its header.
    pub async fn runtime_at_parent(
        &self,
        block: H256,
        parent: H256,
    ) -> Result<RuntimeHandle, ClientError> {
        {
            let mut state = self.0.state.lock().await;
            if let Some(key) = state.blocks.get(&block)
                && let Some(runtime) = state.runtimes.get(&key)
            {
                return Ok(runtime);
            }
        }
        let at = if parent.0 == [0; 32] { block } else { parent };
        let lookup = {
            let mut state = self.0.state.lock().await;
            if let Some(lookup) = state.versions.get(&at) {
                lookup.clone()
            } else {
                let context = self.0.context.clone();
                let inner = Arc::downgrade(&self.0);
                let lookup = async move {
                    let result = async {
                        let value = rpc_request(
                            &context.rpc,
                            "state_getRuntimeVersion",
                            json!([hash_hex(at)]),
                        )
                        .await?;
                        if value.is_null() {
                            return Err(ClientError::Protocol(format!(
                                "No runtime information for block '{}'",
                                hash_hex(block)
                            )));
                        }
                        RuntimeInfo::parse(&value)
                    }
                    .await;
                    if let Some(inner) = inner.upgrade() {
                        inner.state.lock().await.versions.remove(&at);
                    }
                    result
                }
                .boxed()
                .shared();
                state.versions.insert(at, lookup.clone());
                lookup
            }
        };
        let info = lookup.await?;
        self.0
            .state
            .lock()
            .await
            .blocks
            .insert(block, info.key(self.0.context.genesis));
        self.fetch(info, Some(at)).await
    }

    /// Resolve the runtime at a state hash without parent-block selection.
    ///
    /// This is useful for the runtime advertised by a head subscription; block
    /// contents and events must instead use [`Self::runtime_at`].
    pub async fn runtime_at_state(&self, block: H256) -> Result<RuntimeHandle, ClientError> {
        let info = rpc_request(
            &self.0.context.rpc,
            "state_getRuntimeVersion",
            json!([hash_hex(block)]),
        )
        .await?;
        self.fetch(RuntimeInfo::parse(&info)?, Some(block)).await
    }

    pub(crate) async fn fresh_signing_metadata(&self, at: H256) -> Result<Vec<u8>, ClientError> {
        self.0
            .context
            .download(Some(at))
            .await
            .map(|(bytes, _)| bytes)
    }

    /// Recheck the current runtime version, bypassing the twelve-second head TTL.
    pub async fn refresh_head(&self) -> Result<RuntimeHandle, ClientError> {
        let checked_at = Instant::now();
        let generation = self.next_head_generation().await;
        let info = rpc_request(
            &self.0.context.rpc,
            "state_getRuntimeVersion",
            json!([null]),
        )
        .await?;
        self.install_head(&info, checked_at, generation).await
    }

    /// Install the head runtime advertised by a runtime-version notification.
    ///
    /// The notification carries the same object `state_getRuntimeVersion`
    /// returns, so no request is made unless that runtime's metadata is new.
    pub async fn follow_head(&self, version: &Value) -> Result<RuntimeHandle, ClientError> {
        let checked_at = Instant::now();
        let generation = self.next_head_generation().await;
        self.install_head(version, checked_at, generation).await
    }

    /// Mark whether a live runtime-version subscription is keeping the head current.
    pub async fn set_head_followed(&self, followed: bool) {
        self.0.state.lock().await.head_followed = followed;
    }

    /// The installed head runtime, without fetching.
    pub(crate) async fn peek_head(&self) -> Option<RuntimeHandle> {
        self.0
            .state
            .lock()
            .await
            .head
            .as_ref()
            .map(|(runtime, _)| runtime.clone())
    }

    async fn next_head_generation(&self) -> u64 {
        let mut state = self.0.state.lock().await;
        state.head_generation = state.head_generation.wrapping_add(1);
        state.head_generation
    }

    async fn install_head(
        &self,
        version: &Value,
        checked_at: Instant,
        generation: u64,
    ) -> Result<RuntimeHandle, ClientError> {
        let runtime = self.fetch(RuntimeInfo::parse(version)?, None).await?;
        let mut state = self.0.state.lock().await;
        if state.head_generation == generation {
            state.head = Some((runtime.clone(), checked_at));
        }
        Ok(runtime)
    }

    /// Expire the head runtime after an upgrade notification.
    ///
    /// In-progress refreshes cannot revalidate a head invalidated after they
    /// began. Existing historical handles remain immutable and usable.
    pub async fn invalidate_head(&self) {
        let mut state = self.0.state.lock().await;
        state.head = None;
        state.head_generation = state.head_generation.wrapping_add(1);
    }

    async fn fetch(
        &self,
        info: RuntimeInfo,
        at: Option<H256>,
    ) -> Result<RuntimeHandle, ClientError> {
        let key = info.key(self.0.context.genesis);
        let future = {
            let mut state = self.0.state.lock().await;
            if let Some(runtime) = state.runtimes.get(&key) {
                return Ok(runtime);
            }
            if let Some(future) = state.inflight.get(&key) {
                future.clone()
            } else {
                let cached_metadata = state
                    .runtimes
                    .entries
                    .iter()
                    .rev()
                    .find(|((genesis, spec, _), _)| *genesis == key.0 && *spec == key.1)
                    .map(|(_, runtime)| runtime.clone());
                let context = self.0.context.clone();
                let inner = Arc::downgrade(&self.0);
                let future = async move {
                    let result = context.build_runtime(info, at, cached_metadata).await;
                    if let Some(inner) = inner.upgrade() {
                        let mut state = inner.state.lock().await;
                        if let Ok(runtime) = &result {
                            state.runtimes.insert(key, runtime.clone());
                        }
                        state.inflight.remove(&key);
                    }
                    result
                }
                .boxed()
                .shared();
                state.inflight.insert(key, future.clone());
                future
            }
        };
        future.await
    }
}

#[derive(Clone, Debug)]
struct RuntimeInfo {
    spec_version: u32,
    transaction_version: u32,
    api_versions: BTreeMap<[u8; 8], u32>,
}

impl RuntimeInfo {
    fn key(&self, genesis: H256) -> RuntimeKey {
        (genesis, self.spec_version, self.transaction_version)
    }

    fn parse(value: &Value) -> Result<Self, ClientError> {
        let spec_version = parse_u32(value.get("specVersion"), "specVersion")?;
        let transaction_version = match value.get("transactionVersion").filter(|value| !value.is_null()) {
            Some(value) => parse_u32(Some(value), "transactionVersion")?,
            None => return Err(ClientError::Protocol("state_getRuntimeVersion returned no transactionVersion; cannot build a codec that signs correctly".into())),
        };
        let mut api_versions = BTreeMap::new();
        if let Some(apis) = value.get("apis") {
            let apis = apis.as_array().ok_or_else(|| {
                ClientError::Protocol("runtime API versions must be an array".into())
            })?;
            for api in apis {
                let pair = api
                    .as_array()
                    .filter(|pair| pair.len() == 2)
                    .ok_or_else(|| {
                        ClientError::Protocol(
                            "runtime API version must be an ID/version pair".into(),
                        )
                    })?;
                let id: [u8; 8] =
                    decode_hex(&pair[0], "runtime API ID")?
                        .try_into()
                        .map_err(|_| {
                            ClientError::Protocol("runtime API ID must contain eight bytes".into())
                        })?;
                let version = parse_u32(Some(&pair[1]), "runtime API version")?;
                if let Some(previous) = api_versions.insert(id, version)
                    && previous != version
                {
                    return Err(ClientError::Protocol(
                        "conflicting versions for one runtime API ID".into(),
                    ));
                }
            }
        }
        Ok(Self {
            spec_version,
            transaction_version,
            api_versions,
        })
    }
}

impl FetchContext {
    async fn build_runtime(
        &self,
        info: RuntimeInfo,
        at: Option<H256>,
        cached_metadata: Option<RuntimeHandle>,
    ) -> Result<RuntimeHandle, ClientError> {
        if let Some(runtime) = cached_metadata {
            let ss58_format = self.ss58_format;
            // Reparse on a worker with the live stamp, leaving the historical
            // handle immutable and avoiding another metadata download.
            return tokio::task::spawn_blocking(move || {
                parse_runtime_blocking(&runtime.core().metadata_bytes, info, ss58_format)
            })
            .await
            .map_err(worker_error)?;
        }
        let path = self
            .disk_dir
            .as_ref()
            .map(|directory| disk_path(directory, self.genesis, info.spec_version));
        if let Some(cache_path) = &path {
            let load_path = cache_path.clone();
            let cached = tokio::task::spawn_blocking(move || load_disk(&load_path))
                .await
                .map_err(worker_error)?;
            if let Some((metadata, _saved_transaction_version)) = cached {
                // Python's filename is spec-only. Its header records the
                // version at write time; live state supplies the signing stamp.
                match parse_runtime(metadata, info.clone(), self.ss58_format).await {
                    Ok(runtime) => return Ok(runtime),
                    Err(error) => {
                        tracing::warn!(spec_version = info.spec_version, %error, "cached metadata failed to decode; fetching fresh metadata");
                        let remove_path = cache_path.clone();
                        let _ =
                            tokio::task::spawn_blocking(move || fs::remove_file(remove_path)).await;
                    }
                }
            }
        }
        let (metadata, is_v15) = self.download(at).await?;
        let runtime = parse_runtime(metadata.clone(), info.clone(), self.ss58_format).await?;
        if let Some(path) = path {
            let transaction_version = info.transaction_version;
            let result = tokio::task::spawn_blocking(move || {
                save_disk(&path, &metadata, transaction_version, is_v15)
            })
            .await;
            match result {
                Ok(Ok(())) => {}
                Ok(Err(error)) => tracing::debug!(%error, "could not write runtime metadata cache"),
                Err(error) => tracing::debug!(%error, "runtime metadata cache worker failed"),
            }
        }
        Ok(runtime)
    }

    async fn download(&self, at: Option<H256>) -> Result<(Vec<u8>, bool), ClientError> {
        let at = at.map(hash_hex);
        let result = rpc_request(
            &self.rpc,
            "state_call",
            json!(["Metadata_metadata_at_version", V15_VERSION_HEX, at]),
        )
        .await;
        let result = match result {
            Ok(value) => value,
            Err(ClientError::Rpc(ref error)) if error.to_string().contains(V15_MISSING_NEEDLE) => {
                Value::Null
            }
            Err(error) => return Err(error),
        };
        if !result.is_null() {
            let wrapped = decode_hex(&result, "V15 metadata response")?;
            let mut input = wrapped.as_slice();
            let metadata = if wrapped.is_empty() {
                None
            } else {
                Option::<Vec<u8>>::decode(&mut input).map_err(|error| {
                    ClientError::Protocol(format!("invalid Option<OpaqueMetadata>: {error}"))
                })?
            };
            if !input.is_empty() {
                return Err(ClientError::Protocol(
                    "trailing bytes after Option<OpaqueMetadata>".into(),
                ));
            }
            if let Some(metadata) = metadata {
                if metadata.get(4) != Some(&15) {
                    return Err(ClientError::Protocol(
                        "Metadata_metadata_at_version(15) returned non-V15 metadata".into(),
                    ));
                }
                return Ok((metadata, true));
            }
        }
        let legacy = rpc_request(&self.rpc, "state_getMetadata", json!([at])).await?;
        if legacy.is_null() || legacy.as_str() == Some("") {
            return Err(ClientError::Protocol(format!(
                "No metadata for block '{}'",
                at.as_deref().unwrap_or("None")
            )));
        }
        Ok((decode_hex(&legacy, "legacy metadata response")?, false))
    }
}

async fn parse_runtime(
    metadata: Vec<u8>,
    info: RuntimeInfo,
    ss58_format: u16,
) -> Result<RuntimeHandle, ClientError> {
    tokio::task::spawn_blocking(move || parse_runtime_blocking(&metadata, info, ss58_format))
        .await
        .map_err(worker_error)?
}

fn parse_runtime_blocking(
    metadata: &[u8],
    info: RuntimeInfo,
    ss58_format: u16,
) -> Result<RuntimeHandle, ClientError> {
    let core = Runtime::parse(
        metadata,
        info.spec_version,
        info.transaction_version,
        ss58_format,
    )
    .map_err(rtao_core::CoreError::from)
    .map_err(ClientError::from)?;
    let versions = core
        .apis
        .iter()
        .filter_map(|api| {
            let id: [u8; 8] = Blake2b::<U8>::digest(api.name.as_bytes()).into();
            info.api_versions
                .get(&id)
                .map(|version| (api.name.clone(), *version))
        })
        .collect();
    let runtime =
        RuntimeHandle::new_with_api_versions(core, versions).map_err(ClientError::from)?;
    // Both the extension guard and complete generated surface verification
    // run before a newly fetched handle becomes visible to readers.
    let _ = runtime.signing_status();
    let _ = runtime.surface_report();
    Ok(runtime)
}

fn worker_error(error: tokio::task::JoinError) -> ClientError {
    ClientError::Protocol(format!("runtime metadata worker failed: {error}"))
}

fn parse_u32(value: Option<&Value>, field: &str) -> Result<u32, ClientError> {
    let number = value.and_then(|value| value.as_u64().or_else(|| value.as_str()?.parse().ok()));
    number
        .and_then(|number| u32::try_from(number).ok())
        .ok_or_else(|| {
            ClientError::Protocol(format!("state_getRuntimeVersion returned invalid {field}"))
        })
}

fn hash_hex(hash: H256) -> String {
    format!("0x{}", hex::encode(hash.0))
}

fn decode_hex(value: &Value, field: &str) -> Result<Vec<u8>, ClientError> {
    let value = value
        .as_str()
        .ok_or_else(|| ClientError::Protocol(format!("{field} must be a hexadecimal string")))?;
    hex::decode(value.strip_prefix("0x").unwrap_or(value))
        .map_err(|error| ClientError::Protocol(format!("invalid {field}: {error}")))
}

fn parse_hash(value: &Value, field: &str) -> Result<H256, ClientError> {
    decode_hex(value, field)?
        .try_into()
        .map(H256)
        .map_err(|_| ClientError::Protocol(format!("{field} must contain 32 bytes")))
}

fn disk_path(directory: &Path, genesis: H256, spec_version: u32) -> PathBuf {
    let chain = hex::encode(&genesis.0[..8]);
    directory.join(format!(
        "md{DISK_FORMAT_VERSION}-{chain}-{spec_version}.bin"
    ))
}

fn open_offline_metadata(path: &Path) -> std::io::Result<fs::File> {
    let invalid = || {
        std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "offline metadata cache entry must be a bounded regular file",
        )
    };
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        // Refuse a final symlink, and never wait for a FIFO writer before the
        // opened descriptor's type can be checked.
        options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
    }
    #[cfg(not(unix))]
    if !fs::symlink_metadata(path)?.file_type().is_file() {
        return Err(invalid());
    }
    let file = options.open(path)?;
    let metadata = file.metadata()?;
    if !metadata.file_type().is_file() || metadata.len() > MAX_OFFLINE_METADATA_BYTES {
        return Err(invalid());
    }
    Ok(file)
}

fn load_disk(path: &Path) -> Option<(Vec<u8>, u32)> {
    let data = crate::bounded_file::read(path, MAX_OFFLINE_METADATA_BYTES).ok()?;
    let decoded = decode_disk(&data);
    if decoded.is_none() {
        let _ = fs::remove_file(path);
    }
    decoded
}

fn decode_disk(data: &[u8]) -> Option<(Vec<u8>, u32)> {
    if data.len() < 9 || data.get(..4) != Some(DISK_MAGIC) {
        return None;
    }
    // Python writes <IB>; as in Python the body, rather than the flag, decides
    // which metadata decoder is valid.
    let transaction_version = u32::from_le_bytes(data[4..8].try_into().ok()?);
    Some((data[9..].to_vec(), transaction_version))
}

struct TempFile(PathBuf);

impl Drop for TempFile {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
    }
}

fn save_disk(
    path: &Path,
    metadata: &[u8],
    transaction_version: u32,
    is_v15: bool,
) -> std::io::Result<()> {
    if metadata.len() as u64 > MAX_OFFLINE_METADATA_BYTES.saturating_sub(9) {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "metadata cache exceeds 64 MiB limit",
        ));
    }
    let directory = path
        .parent()
        .ok_or_else(|| std::io::Error::other("metadata cache has no parent directory"))?;
    fs::create_dir_all(directory)?;
    let (temporary, mut file) = loop {
        let serial = NEXT_TEMP.fetch_add(1, Ordering::Relaxed);
        let temporary = directory.join(format!(".rtao-md-{}-{serial}.tmp", std::process::id()));
        match OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)
        {
            Ok(file) => break (TempFile(temporary), file),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error),
        }
    };
    file.write_all(DISK_MAGIC)?;
    file.write_all(&transaction_version.to_le_bytes())?;
    file.write_all(&[u8::from(is_v15)])?;
    file.write_all(metadata)?;
    file.sync_all()?;
    drop(file);
    fs::rename(&temporary.0, path)?;
    prune_disk(directory);
    Ok(())
}

fn prune_disk(directory: &Path) {
    let Ok(entries) = fs::read_dir(directory) else {
        return;
    };
    let mut entries: Vec<_> = entries
        .filter_map(Result::ok)
        .filter_map(|entry| {
            let name = entry.file_name();
            let name = name.to_str()?;
            if !name.starts_with("md") || !name.ends_with(".bin") {
                return None;
            }
            let metadata = entry.metadata().ok()?;
            if !metadata.is_file() {
                return None;
            }
            Some((
                metadata.modified().unwrap_or(SystemTime::UNIX_EPOCH),
                entry.path(),
            ))
        })
        .collect();
    entries.sort_by(|left, right| right.0.cmp(&left.0).then_with(|| left.1.cmp(&right.1)));
    for (_, stale) in entries.into_iter().skip(DISK_CACHE_MAX_FILES) {
        let _ = fs::remove_file(stale);
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
    use super::*;
    use parity_scale_codec::Encode;
    use rtao_rpc::RpcError;
    use rtao_runtime::MetadataLevel;
    use serde_json::value::RawValue;
    use std::sync::{Mutex as StdMutex, atomic::AtomicU32};
    use subxt_rpcs::client::{RawRpcFuture, RawRpcSubscription, RpcClientT};
    use tokio::sync::Notify;

    const METADATA: &[u8] = include_bytes!("../../../vendor/metadata/finney/478.v15.scale");
    const GENESIS: H256 = H256([0x17; 32]);

    type Responder = dyn Fn(&str, &Value) -> Result<Value, RpcError> + Send + Sync;

    #[derive(Default)]
    struct Gate {
        entered: Notify,
        release: Notify,
    }

    struct FakeRpc {
        requests: StdMutex<Vec<(String, Value)>>,
        responder: Box<Responder>,
        gate: Option<Arc<Gate>>,
    }

    impl FakeRpc {
        fn new(
            responder: impl Fn(&str, &Value) -> Result<Value, RpcError> + Send + Sync + 'static,
        ) -> Arc<Self> {
            Arc::new(Self {
                requests: StdMutex::new(Vec::new()),
                responder: Box::new(responder),
                gate: None,
            })
        }

        fn count(&self, method: &str) -> usize {
            self.requests
                .lock()
                .unwrap()
                .iter()
                .filter(|(candidate, _)| candidate == method)
                .count()
        }
    }

    impl RpcClientT for FakeRpc {
        fn request_raw<'a>(
            &'a self,
            method: &'a str,
            params: Option<Box<RawValue>>,
        ) -> RawRpcFuture<'a, Box<RawValue>> {
            Box::pin(async move {
                let params: Value = params
                    .map(|value| serde_json::from_str(value.get()).unwrap())
                    .unwrap_or_else(|| json!([]));
                self.requests
                    .lock()
                    .unwrap()
                    .push((method.into(), params.clone()));
                if method == "state_call" {
                    if let Some(gate) = &self.gate {
                        gate.entered.notify_one();
                        gate.release.notified().await;
                    }
                    // Let concurrent clients find the same in-flight fetch.
                    tokio::task::yield_now().await;
                }
                let result = (self.responder)(method, &params)
                    .map_err(|error| subxt_rpcs::Error::Client(Box::new(error)))?;
                serde_json::value::to_raw_value(&result).map_err(subxt_rpcs::Error::Serialization)
            })
        }

        fn subscribe_raw<'a>(
            &'a self,
            _: &'a str,
            _: Option<Box<RawValue>>,
            _: &'a str,
        ) -> RawRpcFuture<'a, RawRpcSubscription> {
            Box::pin(async {
                Err(subxt_rpcs::Error::Client(Box::new(RpcError::Protocol(
                    "cache never subscribes directly".into(),
                ))))
            })
        }
    }

    fn wrapped_metadata() -> Value {
        json!(format!(
            "0x{}",
            hex::encode(Some(METADATA.to_vec()).encode())
        ))
    }

    fn usual_response(method: &str, _: &Value) -> Result<Value, RpcError> {
        match method {
            "state_getRuntimeVersion" => Ok(json!({"specVersion":478, "transactionVersion":1})),
            "state_call" => Ok(wrapped_metadata()),
            _ => panic!("unexpected method {method}"),
        }
    }

    fn cache(rpc: Arc<FakeRpc>) -> RuntimeCache {
        RuntimeCache::new(RpcClient::new(rpc), GENESIS, 42, None)
    }

    struct TestDirectory(PathBuf);

    impl TestDirectory {
        fn new() -> Self {
            let root = std::env::var_os("RTAO_TEMP_ROOT")
                .map(|root| PathBuf::from(root).join("tmp"))
                .unwrap_or_else(std::env::temp_dir);
            fs::create_dir_all(&root).unwrap();
            loop {
                let serial = NEXT_TEMP.fetch_add(1, Ordering::Relaxed);
                let path = root.join(format!(
                    "runtime-cache-test-{}-{serial}",
                    std::process::id()
                ));
                match fs::create_dir(&path) {
                    Ok(()) => return Self(path),
                    Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
                    Err(error) => panic!("cannot create cache fixture directory: {error}"),
                }
            }
        }
    }

    impl Drop for TestDirectory {
        fn drop(&mut self) {
            // All fixture RPC and blocking workers are joined before this owner
            // is dropped. Only this exclusively created directory is removed.
            fs::remove_dir_all(&self.0).unwrap();
        }
    }

    #[test]
    fn lru_refreshes_reads_and_evicts_only_the_oldest() {
        let mut cache = Lru::new(4);
        for value in 0..4 {
            cache.insert(value, value);
        }
        assert_eq!(cache.get(&0), Some(0));
        cache.insert(4, 4);
        assert_eq!(cache.get(&1), None);
        assert_eq!(cache.get(&0), Some(0));
        cache.insert(2, 22);
        assert_eq!(cache.entries.len(), 4);
        assert_eq!(cache.get(&2), Some(22));
    }

    #[test]
    fn exact_disk_header_and_python_names() {
        let directory = TestDirectory::new();
        let path = disk_path(&directory.0, GENESIS, 478);
        assert_eq!(path.file_name().unwrap(), "md1-1717171717171717-478.bin");
        save_disk(&path, b"metadata", 0x12345678, true).unwrap();
        assert_eq!(
            fs::read(&path).unwrap(),
            b"STMD\x78\x56\x34\x12\x01metadata"
        );
        assert_eq!(load_disk(&path), Some((b"metadata".to_vec(), 0x12345678)));
        assert_eq!(fs::read_dir(&directory.0).unwrap().count(), 1);
    }

    #[test]
    fn unreadable_disk_headers_are_removed() {
        let directory = TestDirectory::new();
        for malformed in [&b"STMD"[..], &b"WRNG\0\0\0\0\x01metadata"[..]] {
            let path = disk_path(&directory.0, GENESIS, 478);
            fs::write(&path, malformed).unwrap();
            assert!(load_disk(&path).is_none());
            assert!(!path.exists());
        }
    }

    #[test]
    fn oversized_save_counts_header_and_preserves_existing_entry() {
        let directory = TestDirectory::new();
        let path = disk_path(&directory.0, GENESIS, 478);
        save_disk(&path, b"previous metadata", 7, true).unwrap();
        let original = fs::read(&path).unwrap();
        // The body is individually below the file cap; the nine-byte header
        // puts the complete representation exactly one byte over it.
        let metadata = vec![0; (MAX_OFFLINE_METADATA_BYTES - 8) as usize];
        assert_eq!(
            save_disk(&path, &metadata, 8, true).unwrap_err().kind(),
            std::io::ErrorKind::InvalidData
        );
        assert_eq!(fs::read(path).unwrap(), original);
        assert_eq!(fs::read_dir(&directory.0).unwrap().count(), 1);
    }

    #[tokio::test]
    async fn live_cache_treats_oversized_files_as_misses_and_fetches_bounded_replacements() {
        let directory = TestDirectory::new();
        let path = disk_path(&directory.0, GENESIS, 478);
        let file = fs::File::create(&path).unwrap();
        file.set_len(MAX_OFFLINE_METADATA_BYTES + 1).unwrap();
        drop(file);
        // Failed acquisition does not classify or remove a file's contents.
        assert!(load_disk(&path).is_none());
        assert_eq!(
            fs::metadata(&path).unwrap().len(),
            MAX_OFFLINE_METADATA_BYTES + 1
        );
        let rpc = FakeRpc::new(usual_response);
        let cache = RuntimeCache::new(
            RpcClient::new(rpc.clone()),
            GENESIS,
            42,
            Some(directory.0.clone()),
        );
        let runtime = cache.runtime_at(None).await.unwrap();
        assert_eq!(runtime.spec_version(), 478);
        assert!(
            rpc.requests
                .lock()
                .unwrap()
                .iter()
                .any(|(method, _)| method == "state_call")
        );
        assert_eq!(load_disk(&path).unwrap().0, METADATA);
        assert!(fs::metadata(&path).unwrap().len() <= MAX_OFFLINE_METADATA_BYTES);
        assert!(fs::read_dir(&directory.0).unwrap().all(|entry| {
            !entry
                .unwrap()
                .file_name()
                .to_string_lossy()
                .starts_with(".rtao-md-")
        }));
    }

    #[tokio::test]
    async fn offline_lookup_reuses_latest_spec_without_rpc() {
        let rpc = FakeRpc::new(usual_response);
        let cache = cache(rpc.clone());
        let live = cache.runtime_at(None).await.unwrap();
        let requests = rpc.requests.lock().unwrap().len();
        assert_eq!(cache.genesis(), GENESIS);
        let restored = cache.cached_runtime(478).await.unwrap();
        assert_eq!(restored.core().spec_version, live.core().spec_version);
        assert_eq!(
            restored.core().transaction_version,
            live.core().transaction_version
        );
        assert!(cache.cached_runtime(479).await.is_err());
        assert_eq!(rpc.requests.lock().unwrap().len(), requests);
    }

    #[tokio::test]
    async fn offline_disk_lookup_retains_version_without_poisoning_live_cache() {
        let directory = TestDirectory::new();
        let path = disk_path(&directory.0, GENESIS, 478);
        save_disk(&path, METADATA, 17, true).unwrap();
        let original = fs::read(&path).unwrap();
        let rpc = FakeRpc::new(usual_response);
        let cache = RuntimeCache::new(
            RpcClient::new(rpc.clone()),
            GENESIS,
            42,
            Some(directory.0.clone()),
        );
        let restored = cache.cached_runtime(478).await.unwrap();
        assert_eq!(restored.core().spec_version, 478);
        assert_eq!(restored.core().transaction_version, 17);
        assert!(
            restored
                .core()
                .apis
                .iter()
                .all(|api| restored.runtime_api_version(&api.name).is_none())
        );
        assert!(rpc.requests.lock().unwrap().is_empty());
        assert!(cache.0.state.lock().await.runtimes.entries.is_empty());
        assert_eq!(fs::read(&path).unwrap(), original);
    }

    #[tokio::test]
    async fn offline_malformed_entries_remain_untouched_and_do_not_call_rpc() {
        let directory = TestDirectory::new();
        let rpc = FakeRpc::new(usual_response);
        let cache = RuntimeCache::new(
            RpcClient::new(rpc.clone()),
            GENESIS,
            42,
            Some(directory.0.clone()),
        );
        let path = disk_path(&directory.0, GENESIS, 478);
        for data in [
            &b"STMD"[..],
            &b"STMD\x01\x00\x00\x00\x02bad"[..],
            &b"STMD\x01\x00\x00\x00\x01bad"[..],
        ] {
            fs::write(&path, data).unwrap();
            assert!(cache.cached_runtime(478).await.is_err());
            assert_eq!(fs::read(&path).unwrap(), data);
        }
        assert!(cache.cached_runtime(479).await.is_err());
        assert!(rpc.requests.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn offline_rejects_directories_and_oversized_files_without_mutation() {
        let directory = TestDirectory::new();
        let rpc = FakeRpc::new(|_, _| Err(RpcError::Transport("RPC must not be called".into())));
        let cache = RuntimeCache::new(
            RpcClient::new(rpc.clone()),
            GENESIS,
            42,
            Some(directory.0.clone()),
        );
        let directory_entry = disk_path(&directory.0, GENESIS, 478);
        fs::create_dir(&directory_entry).unwrap();
        assert!(cache.cached_runtime(478).await.is_err());
        assert!(directory_entry.is_dir());
        assert_eq!(fs::read_dir(&directory_entry).unwrap().count(), 0);

        let oversized_entry = disk_path(&directory.0, GENESIS, 479);
        let file = fs::File::create(&oversized_entry).unwrap();
        file.set_len(MAX_OFFLINE_METADATA_BYTES + 1).unwrap();
        drop(file);
        assert!(cache.cached_runtime(479).await.is_err());
        assert_eq!(
            fs::metadata(&oversized_entry).unwrap().len(),
            MAX_OFFLINE_METADATA_BYTES + 1
        );
        assert!(rpc.requests.lock().unwrap().is_empty());
        let state = cache.0.state.lock().await;
        assert!(state.runtimes.entries.is_empty());
        assert!(state.head.is_none());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn offline_rejects_symlinks_and_fifos_without_rpc_or_hanging() {
        use std::os::unix::fs::{FileTypeExt, symlink};

        let directory = TestDirectory::new();
        let rpc = FakeRpc::new(|_, _| Err(RpcError::Transport("RPC must not be called".into())));
        let cache = RuntimeCache::new(
            RpcClient::new(rpc.clone()),
            GENESIS,
            42,
            Some(directory.0.clone()),
        );
        let target = directory.0.join("retained-metadata.bin");
        save_disk(&target, METADATA, 17, true).unwrap();
        let original = fs::read(&target).unwrap();
        let linked_entry = disk_path(&directory.0, GENESIS, 478);
        symlink(&target, &linked_entry).unwrap();
        assert!(cache.cached_runtime(478).await.is_err());
        assert!(
            fs::symlink_metadata(&linked_entry)
                .unwrap()
                .file_type()
                .is_symlink()
        );
        assert_eq!(fs::read(&target).unwrap(), original);

        let fifo_entry = disk_path(&directory.0, GENESIS, 479);
        assert!(
            std::process::Command::new("mkfifo")
                .arg(&fifo_entry)
                .status()
                .unwrap()
                .success()
        );
        let result = tokio::time::timeout(Duration::from_secs(1), cache.cached_runtime(479))
            .await
            .expect("offline metadata must not wait for a FIFO writer");
        assert!(result.is_err());
        assert!(
            fs::symlink_metadata(&fifo_entry)
                .unwrap()
                .file_type()
                .is_fifo()
        );
        assert!(rpc.requests.lock().unwrap().is_empty());
        let state = cache.0.state.lock().await;
        assert!(state.runtimes.entries.is_empty());
        assert!(state.head.is_none());
    }

    #[test]
    fn prune_keeps_eight_python_entries_and_other_names() {
        let directory = TestDirectory::new();
        let tooling = directory.0.join("rtao-md16-1717171717171717-478.scale");
        let unrelated = directory.0.join("notes.bin");
        fs::write(&tooling, b"tooling").unwrap();
        fs::write(&unrelated, b"notes").unwrap();
        for spec in 0..12 {
            save_disk(
                &disk_path(&directory.0, GENESIS, spec),
                b"metadata",
                1,
                false,
            )
            .unwrap();
        }
        let count = fs::read_dir(&directory.0)
            .unwrap()
            .filter_map(Result::ok)
            .filter(|entry| {
                entry
                    .file_name()
                    .to_str()
                    .is_some_and(|name| name.starts_with("md") && name.ends_with(".bin"))
            })
            .count();
        assert_eq!(count, 8);
        assert_eq!(fs::read(tooling).unwrap(), b"tooling");
        assert_eq!(fs::read(unrelated).unwrap(), b"notes");
    }

    #[test]
    fn transaction_version_is_required_and_checked() {
        assert!(
            RuntimeInfo::parse(&json!({"specVersion":478}))
                .unwrap_err()
                .to_string()
                .contains("no transactionVersion")
        );
        assert!(RuntimeInfo::parse(&json!({"specVersion":478,"transactionVersion":-1})).is_err());
        assert!(
            RuntimeInfo::parse(&json!({"specVersion":478,"transactionVersion":4294967296_u64}))
                .is_err()
        );
        assert_eq!(
            RuntimeInfo::parse(&json!({"specVersion":"478","transactionVersion":"1"}))
                .unwrap()
                .transaction_version,
            1
        );
        assert!(RuntimeInfo::parse(&json!({"specVersion":478,"transactionVersion":1,"apis":[["0x0000000000000000",1],["0x0000000000000000",2]]})).is_err());
    }

    #[tokio::test]
    async fn parent_runtime_is_selected_and_genesis_resolves_itself() {
        let block = H256([1; 32]);
        let parent = H256([2; 32]);
        let rpc = FakeRpc::new(move |method, params| match method {
            "chain_getHeader" => Ok(
                json!({"parentHash":hash_hex(if params[0] == hash_hex(GENESIS) { H256([0;32]) } else { parent })}),
            ),
            "state_getRuntimeVersion" => {
                assert!(params[0] == hash_hex(parent) || params[0] == hash_hex(GENESIS));
                Ok(json!({"specVersion":478,"transactionVersion":1}))
            }
            "state_call" => {
                assert_eq!(params[2], hash_hex(parent));
                Ok(wrapped_metadata())
            }
            _ => panic!("unexpected method {method}"),
        });
        let cache = cache(rpc.clone());
        let historical = cache.runtime_at(Some(block)).await.unwrap();
        assert_eq!(historical.spec_version(), 478);
        let repeat = cache.runtime_at(Some(block)).await.unwrap();
        assert!(std::ptr::eq(historical.entry(), repeat.entry()));
        assert_eq!(rpc.count("chain_getHeader"), 1);
        cache.runtime_at(Some(GENESIS)).await.unwrap();
        assert_eq!(rpc.count("chain_getHeader"), 2);
        assert_eq!(rpc.count("state_call"), 1);
    }

    #[tokio::test]
    async fn missing_block_has_typed_error() {
        let rpc = FakeRpc::new(|method, _| {
            assert_eq!(method, "chain_getHeader");
            Ok(Value::Null)
        });
        let block = H256([1; 32]);
        assert!(
            matches!(cache(rpc).runtime_at(Some(block)).await, Err(ClientError::BlockNotFound(hash)) if hash == block)
        );
    }

    #[tokio::test]
    async fn concurrent_spec_fetches_share_one_handle() {
        let rpc = FakeRpc::new(usual_response);
        let cache = cache(rpc.clone());
        let results = futures::future::join_all((0..16).map(|_| cache.refresh_head())).await;
        let first = results[0].as_ref().unwrap();
        for result in &results {
            assert!(std::ptr::eq(
                first.entry(),
                result.as_ref().unwrap().entry()
            ));
        }
        assert_eq!(rpc.count("state_call"), 1);
        assert_eq!(cache.0.state.lock().await.inflight.len(), 0);
    }

    #[tokio::test]
    async fn cancelled_first_waiter_does_not_wedge_the_shared_fetch() {
        let gate = Arc::new(Gate::default());
        let rpc = Arc::new(FakeRpc {
            requests: StdMutex::new(Vec::new()),
            responder: Box::new(usual_response),
            gate: Some(gate.clone()),
        });
        let cache = cache(rpc.clone());
        let owner = tokio::spawn({
            let cache = cache.clone();
            async move { cache.refresh_head().await }
        });
        gate.entered.notified().await;
        owner.abort();
        assert!(owner.await.unwrap_err().is_cancelled());
        gate.release.notify_one();
        let result = cache.refresh_head().await.unwrap();
        assert_eq!(result.spec_version(), 478);
        assert_eq!(rpc.count("state_call"), 1);
    }

    #[tokio::test]
    async fn failures_are_coalesced_then_retryable() {
        let attempts = Arc::new(AtomicU32::new(0));
        let rpc = FakeRpc::new({
            let attempts = attempts.clone();
            move |method, params| {
                if method == "state_call" && attempts.fetch_add(1, Ordering::SeqCst) == 0 {
                    return Err(RpcError::Transport("offline".into()));
                }
                usual_response(method, params)
            }
        });
        let cache = cache(rpc.clone());
        let (left, right) = tokio::join!(cache.refresh_head(), cache.refresh_head());
        assert_eq!(left.unwrap_err().to_string(), "offline");
        assert_eq!(right.unwrap_err().to_string(), "offline");
        assert_eq!(rpc.count("state_call"), 1);
        assert_eq!(cache.refresh_head().await.unwrap().spec_version(), 478);
        assert_eq!(rpc.count("state_call"), 2);
    }

    #[tokio::test]
    async fn head_invalidation_rechecks_and_replaces_upgraded_spec() {
        let spec = Arc::new(AtomicU32::new(478));
        let rpc = FakeRpc::new({
            let spec = spec.clone();
            move |method, params| {
                if method == "state_getRuntimeVersion" {
                    return Ok(
                        json!({"specVersion":spec.load(Ordering::SeqCst),"transactionVersion":1}),
                    );
                }
                usual_response(method, params)
            }
        });
        let cache = cache(rpc.clone());
        let old = cache.runtime_at(None).await.unwrap();
        assert_eq!(cache.runtime_at(None).await.unwrap().spec_version(), 478);
        assert_eq!(rpc.count("state_getRuntimeVersion"), 1);
        spec.store(479, Ordering::SeqCst);
        cache.invalidate_head().await;
        assert_eq!(cache.runtime_at(None).await.unwrap().spec_version(), 479);
        assert_eq!(old.spec_version(), 478);
        assert_eq!(rpc.count("state_call"), 2);
    }

    #[tokio::test]
    async fn cached_runtime_uses_memory_without_rpc_or_head_mutation() {
        let rpc = FakeRpc::new(|_, _| Err(RpcError::Transport("RPC must not be called".into())));
        let cache = cache(rpc.clone());
        let old = RuntimeHandle::parse(METADATA, 478, 1, 42).unwrap();
        let recent = RuntimeHandle::parse(METADATA, 478, 2, 42).unwrap();
        let head = RuntimeHandle::parse(METADATA, 479, 1, 42).unwrap();
        {
            let mut state = cache.0.state.lock().await;
            state.runtimes.insert((GENESIS, 478, 1), old.clone());
            state.runtimes.insert((GENESIS, 478, 2), recent.clone());
            state.runtimes.insert((GENESIS, 479, 1), head.clone());
            state.head = Some((head.clone(), Instant::now()));
        }
        assert_eq!(cache.genesis(), GENESIS);
        let restored = cache.cached_runtime(478).await.unwrap();
        assert!(std::ptr::eq(restored.entry(), recent.entry()));
        assert_eq!(restored.transaction_version(), 2);
        assert_eq!(old.transaction_version(), 1);
        assert!(cache.cached_runtime(999).await.is_err());
        let state = cache.0.state.lock().await;
        assert!(std::ptr::eq(
            state.head.as_ref().unwrap().0.entry(),
            head.entry()
        ));
        assert_eq!(state.runtimes.entries.len(), 3);
        assert!(rpc.requests.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn cached_runtime_disk_lookup_preserves_all_files_and_live_cache() {
        let directory = TestDirectory::new();
        let rpc = FakeRpc::new(|_, _| Err(RpcError::Transport("RPC must not be called".into())));
        let cache = RuntimeCache::new(
            RpcClient::new(rpc.clone()),
            GENESIS,
            42,
            Some(directory.0.clone()),
        );
        let path = disk_path(&directory.0, GENESIS, 478);
        save_disk(&path, METADATA, 7, true).unwrap();
        let before = fs::read(&path).unwrap();
        let restored = cache.cached_runtime(478).await.unwrap();
        assert_eq!(restored.spec_version(), 478);
        assert_eq!(restored.transaction_version(), 7);
        assert_eq!(fs::read(&path).unwrap(), before);
        assert!(cache.cached_runtime(999).await.is_err());
        assert!(!disk_path(&directory.0, GENESIS, 999).exists());
        let corrupt_header = disk_path(&directory.0, GENESIS, 479);
        fs::write(&corrupt_header, b"corrupt metadata cache").unwrap();
        assert!(cache.cached_runtime(479).await.is_err());
        assert_eq!(fs::read(corrupt_header).unwrap(), b"corrupt metadata cache");
        let corrupt_body = disk_path(&directory.0, GENESIS, 480);
        save_disk(&corrupt_body, b"bad metadata", 1, true).unwrap();
        let body_before = fs::read(&corrupt_body).unwrap();
        assert!(cache.cached_runtime(480).await.is_err());
        assert_eq!(fs::read(corrupt_body).unwrap(), body_before);
        let state = cache.0.state.lock().await;
        assert!(state.runtimes.entries.is_empty());
        assert!(state.head.is_none());
        assert!(rpc.requests.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn transaction_version_upgrade_reuses_metadata_and_preserves_history() {
        let old_block = H256([1; 32]);
        let new_block = H256([2; 32]);
        let old_parent = H256([3; 32]);
        let new_parent = H256([4; 32]);
        let transaction_version = Arc::new(AtomicU32::new(1));
        let rpc = FakeRpc::new({
            let transaction_version = transaction_version.clone();
            move |method, params| match method {
                "chain_getHeader" => {
                    let parent = if params[0] == hash_hex(old_block) {
                        old_parent
                    } else {
                        assert_eq!(params[0], hash_hex(new_block));
                        new_parent
                    };
                    Ok(json!({"parentHash":hash_hex(parent)}))
                }
                "state_getRuntimeVersion" => {
                    let tx = if params[0].is_null() {
                        transaction_version.load(Ordering::SeqCst)
                    } else if params[0] == hash_hex(old_parent) {
                        1
                    } else {
                        assert_eq!(params[0], hash_hex(new_parent));
                        2
                    };
                    Ok(json!({"specVersion":478,"transactionVersion":tx}))
                }
                _ => usual_response(method, params),
            }
        });
        let cache = cache(rpc.clone());
        let historical = cache.runtime_at(Some(old_block)).await.unwrap();
        let old_head = cache.runtime_at(None).await.unwrap();
        let old_call = rtao_runtime::calls::system::remark(vec![1])
            .compose(&old_head)
            .unwrap();

        transaction_version.store(2, Ordering::SeqCst);
        cache.invalidate_head().await;
        let new_head = cache.runtime_at(None).await.unwrap();
        assert_eq!(new_head.spec_version(), 478);
        assert_eq!(new_head.transaction_version(), 2);
        assert!(new_head.signing_status().is_ok());
        assert!(old_call.ensure_runtime(&new_head).is_err());
        let new_call = rtao_runtime::calls::system::remark(vec![2])
            .compose(&new_head)
            .unwrap();
        assert_eq!(new_call.transaction_version(), 2);
        assert!(new_call.ensure_runtime(&new_head).is_ok());

        let old_again = cache.runtime_at(Some(old_block)).await.unwrap();
        let new_historical = cache.runtime_at(Some(new_block)).await.unwrap();
        let new_again = cache.runtime_at(Some(new_block)).await.unwrap();
        assert_eq!(historical.transaction_version(), 1);
        assert_eq!(old_head.transaction_version(), 1);
        assert!(old_call.ensure_runtime(&historical).is_ok());
        assert!(std::ptr::eq(historical.entry(), old_again.entry()));
        assert!(std::ptr::eq(new_head.entry(), new_historical.entry()));
        assert!(std::ptr::eq(new_head.entry(), new_again.entry()));
        assert_eq!(rpc.count("chain_getHeader"), 2);
        assert_eq!(rpc.count("state_call"), 1);
    }

    #[tokio::test]
    async fn concurrent_transaction_versions_do_not_share_an_inflight_handle() {
        let gate = Arc::new(Gate::default());
        let rpc = Arc::new(FakeRpc {
            requests: StdMutex::new(Vec::new()),
            responder: Box::new(|method, params| {
                if method == "state_getRuntimeVersion" {
                    let hash = parse_hash(&params[0], "state hash").unwrap();
                    return Ok(json!({"specVersion":478,"transactionVersion":hash.0[0]}));
                }
                usual_response(method, params)
            }),
            gate: Some(gate.clone()),
        });
        let cache = cache(rpc.clone());
        let first = tokio::spawn({
            let cache = cache.clone();
            async move { cache.runtime_at_state(H256([1; 32])).await }
        });
        gate.entered.notified().await;
        let second = tokio::spawn({
            let cache = cache.clone();
            async move { cache.runtime_at_state(H256([2; 32])).await }
        });
        tokio::time::timeout(Duration::from_secs(1), gate.entered.notified())
            .await
            .expect("a different transaction version must start its own fetch");
        assert_eq!(cache.0.state.lock().await.inflight.len(), 2);
        gate.release.notify_waiters();
        let first = first.await.unwrap().unwrap();
        let second = second.await.unwrap().unwrap();
        assert_eq!(first.transaction_version(), 1);
        assert_eq!(second.transaction_version(), 2);
        assert_eq!(cache.0.state.lock().await.inflight.len(), 0);
        let first_again = cache.runtime_at_state(H256([1; 32])).await.unwrap();
        let second_again = cache.runtime_at_state(H256([2; 32])).await.unwrap();
        assert!(std::ptr::eq(first.entry(), first_again.entry()));
        assert!(std::ptr::eq(second.entry(), second_again.entry()));
        assert_eq!(rpc.count("state_call"), 2);
    }

    #[tokio::test]
    async fn stale_disk_transaction_header_uses_live_version_without_redownload() {
        let directory = TestDirectory::new();
        let path = disk_path(&directory.0, GENESIS, 478);
        save_disk(&path, METADATA, 7, true).unwrap();
        let rpc = FakeRpc::new(|method, _| {
            assert_eq!(method, "state_getRuntimeVersion");
            Ok(json!({"specVersion":478,"transactionVersion":1}))
        });
        let cache = RuntimeCache::new(
            RpcClient::new(rpc.clone()),
            GENESIS,
            42,
            Some(directory.0.clone()),
        );
        let runtime = cache.refresh_head().await.unwrap();
        assert_eq!(runtime.transaction_version(), 1);
        assert_eq!(rpc.count("state_call"), 0);
        // Reusing the body preserves Python's spec-only file and byte format.
        assert_eq!(load_disk(&path).unwrap(), (METADATA.to_vec(), 7));
    }

    #[tokio::test]
    async fn corrupt_cached_body_is_redownloaded_and_replaced() {
        let directory = TestDirectory::new();
        let path = disk_path(&directory.0, GENESIS, 478);
        save_disk(&path, b"not metadata", 1, true).unwrap();
        let rpc = FakeRpc::new(usual_response);
        let cache = RuntimeCache::new(
            RpcClient::new(rpc.clone()),
            GENESIS,
            42,
            Some(directory.0.clone()),
        );
        cache.refresh_head().await.unwrap();
        assert_eq!(rpc.count("state_call"), 1);
        assert_eq!(load_disk(&path).unwrap().0, METADATA);
        assert_eq!(fs::read(&path).unwrap()[..9], *b"STMD\x01\x00\x00\x00\x01");
    }

    #[tokio::test]
    async fn disk_flag_records_the_python_fetch_path() {
        let directory = TestDirectory::new();
        let path = disk_path(&directory.0, GENESIS, 478);
        let rpc = FakeRpc::new(|method, params| match method {
            "state_call" => Ok(Value::Null),
            // The shared file's flag records the legacy fetch path even if a
            // newer node returns a V15 body from its legacy metadata RPC.
            "state_getMetadata" => Ok(json!(format!("0x{}", hex::encode(METADATA)))),
            _ => usual_response(method, params),
        });
        let cache = RuntimeCache::new(RpcClient::new(rpc), GENESIS, 42, Some(directory.0.clone()));
        let runtime = cache.refresh_head().await.unwrap();
        assert_eq!(runtime.metadata_level(), MetadataLevel::V15);
        let data = fs::read(path).unwrap();
        assert_eq!(data[8], 0);
        assert_eq!(&data[9..], METADATA);
    }

    #[tokio::test]
    async fn other_rpc_errors_never_fall_back_to_v14() {
        let rpc = FakeRpc::new(|method, params| {
            if method == "state_call" {
                return Err(RpcError::JsonRpc {
                    code: -32603,
                    message: "temporary overload".into(),
                    data: None,
                    payload: Box::new(json!({"code":-32603,"message":"temporary overload"})),
                });
            }
            usual_response(method, params)
        });
        assert!(
            cache(rpc.clone())
                .refresh_head()
                .await
                .unwrap_err()
                .to_string()
                .contains("temporary overload")
        );
        assert_eq!(rpc.count("state_getMetadata"), 0);
    }

    #[tokio::test]
    async fn malformed_some_wrapper_is_an_error_without_fallback() {
        let rpc = FakeRpc::new(|method, params| {
            if method == "state_call" {
                return Ok(json!("0x010c6d65"));
            }
            usual_response(method, params)
        });
        assert!(cache(rpc.clone()).refresh_head().await.is_err());
        assert_eq!(rpc.count("state_getMetadata"), 0);
    }

    fn v14_metadata() -> Vec<u8> {
        use frame_metadata::{RuntimeMetadata, RuntimeMetadataPrefixed};
        let prefixed = RuntimeMetadataPrefixed::decode(&mut &METADATA[..]).unwrap();
        let RuntimeMetadata::V15(v15) = prefixed.1 else {
            unreachable!()
        };
        let extrinsic_ty = v15
            .types
            .types
            .iter()
            .find(|ty| {
                ty.ty
                    .path
                    .segments
                    .last()
                    .is_some_and(|name| name == "UncheckedExtrinsic")
            })
            .map(|ty| ty.id)
            .unwrap_or(v15.ty.id);
        let v14 = frame_metadata::v14::RuntimeMetadataV14 {
            types: v15.types,
            pallets: v15
                .pallets
                .into_iter()
                .map(|pallet| frame_metadata::v14::PalletMetadata {
                    name: pallet.name,
                    storage: pallet.storage,
                    calls: pallet.calls,
                    event: pallet.event,
                    constants: pallet.constants,
                    error: pallet.error,
                    index: pallet.index,
                })
                .collect(),
            extrinsic: frame_metadata::v14::ExtrinsicMetadata {
                ty: extrinsic_ty.into(),
                version: v15.extrinsic.version,
                signed_extensions: v15
                    .extrinsic
                    .signed_extensions
                    .into_iter()
                    .map(|extension| frame_metadata::v14::SignedExtensionMetadata {
                        identifier: extension.identifier,
                        ty: extension.ty,
                        additional_signed: extension.additional_signed,
                    })
                    .collect(),
            },
            ty: v15.ty,
        };
        RuntimeMetadataPrefixed(frame_metadata::META_RESERVED, RuntimeMetadata::V14(v14)).encode()
    }

    #[tokio::test]
    async fn missing_method_and_scale_none_are_the_only_legacy_paths() {
        let legacy = Arc::new(v14_metadata());
        for mode in 0..4 {
            let rpc = FakeRpc::new({
                let legacy = legacy.clone();
                move |method, params| match method {
                    "state_call" => match mode {
                        0 => Err(RpcError::JsonRpc {
                            code: -32603,
                            message: V15_MISSING_NEEDLE.into(),
                            data: None,
                            payload: Box::new(json!({"code":-32603,"message":V15_MISSING_NEEDLE})),
                        }),
                        1 => Ok(Value::Null),
                        2 => Ok(json!("0x00")),
                        _ => Ok(json!("0x")),
                    },
                    "state_getMetadata" => {
                        Ok(json!(format!("0x{}", hex::encode(legacy.as_slice()))))
                    }
                    _ => usual_response(method, params),
                }
            });
            let runtime = cache(rpc.clone()).refresh_head().await.unwrap();
            assert_eq!(runtime.metadata_level(), MetadataLevel::V14Degraded);
            assert!(runtime.signing_status().is_err());
            assert!(runtime.storage_entry("System", "Account").is_some());
            assert_eq!(rpc.count("state_getMetadata"), 1);
        }
    }

    #[tokio::test]
    async fn runtime_api_versions_are_resolved_by_blake2_trait_id() {
        let id = hex::encode(Blake2b::<U8>::digest(b"AccountNonceApi"));
        let rpc = FakeRpc::new(move |method, params| {
            if method == "state_getRuntimeVersion" {
                return Ok(
                    json!({"specVersion":478,"transactionVersion":1,"apis":[[format!("0x{id}"),1]]}),
                );
            }
            usual_response(method, params)
        });
        let runtime = cache(rpc).refresh_head().await.unwrap();
        assert_eq!(runtime.runtime_api_version("AccountNonceApi"), Some(1));
        assert_eq!(runtime.runtime_api_version("Metadata"), None);
    }

    #[tokio::test]
    async fn parsed_runtime_lru_retains_four_specs() {
        let rpc = FakeRpc::new(|method, params| {
            if method == "state_getRuntimeVersion" {
                let block = parse_hash(&params[0], "state hash").unwrap();
                return Ok(json!({"specVersion":u32::from(block.0[0]),"transactionVersion":1}));
            }
            usual_response(method, params)
        });
        let cache = cache(rpc.clone());
        let oldest = cache.runtime_at_state(H256([1; 32])).await.unwrap();
        for spec in 2..=5 {
            cache.runtime_at_state(H256([spec; 32])).await.unwrap();
        }
        assert_eq!(rpc.count("state_call"), 5);
        let recent = cache.runtime_at_state(H256([5; 32])).await.unwrap();
        assert_eq!(recent.spec_version(), 5);
        assert_eq!(rpc.count("state_call"), 5);
        let refetched = cache.runtime_at_state(H256([1; 32])).await.unwrap();
        assert_eq!(rpc.count("state_call"), 6);
        assert!(!std::ptr::eq(oldest.entry(), refetched.entry()));
        assert_eq!(oldest.spec_version(), 1);
    }

    #[tokio::test]
    async fn block_cache_retains_512_recent_resolutions() {
        let rpc = FakeRpc::new(|method, params| {
            if method == "chain_getHeader" {
                return Ok(json!({"parentHash":hash_hex(GENESIS)}));
            }
            usual_response(method, params)
        });
        let cache = cache(rpc.clone());
        let block = |number: u64| {
            let mut bytes = [0; 32];
            bytes[..8].copy_from_slice(&number.to_le_bytes());
            H256(bytes)
        };
        for number in 0..513 {
            cache.runtime_at(Some(block(number))).await.unwrap();
        }
        assert_eq!(rpc.count("chain_getHeader"), 513);
        cache.runtime_at(Some(block(512))).await.unwrap();
        assert_eq!(rpc.count("chain_getHeader"), 513);
        cache.runtime_at(Some(block(0))).await.unwrap();
        assert_eq!(rpc.count("chain_getHeader"), 514);
        assert_eq!(rpc.count("state_call"), 1);
    }

    #[tokio::test]
    async fn cancelled_fetch_cannot_keep_a_dropped_cache_alive() {
        let gate = Arc::new(Gate::default());
        let rpc = Arc::new(FakeRpc {
            requests: StdMutex::new(Vec::new()),
            responder: Box::new(usual_response),
            gate: Some(gate.clone()),
        });
        let cache = cache(rpc);
        let weak = Arc::downgrade(&cache.0);
        let owner = tokio::spawn({
            let cache = cache.clone();
            async move { cache.refresh_head().await }
        });
        gate.entered.notified().await;
        owner.abort();
        assert!(owner.await.unwrap_err().is_cancelled());
        drop(cache);
        assert!(weak.upgrade().is_none());
    }

    #[tokio::test]
    async fn an_invalidated_inflight_refresh_cannot_revalidate_the_head() {
        let gate = Arc::new(Gate::default());
        let rpc = Arc::new(FakeRpc {
            requests: StdMutex::new(Vec::new()),
            responder: Box::new(usual_response),
            gate: Some(gate.clone()),
        });
        let cache = cache(rpc.clone());
        let owner = tokio::spawn({
            let cache = cache.clone();
            async move { cache.refresh_head().await }
        });
        gate.entered.notified().await;
        cache.invalidate_head().await;
        gate.release.notify_one();
        owner.await.unwrap().unwrap();
        assert!(cache.0.state.lock().await.head.is_none());
        cache.runtime_at(None).await.unwrap();
        assert_eq!(rpc.count("state_getRuntimeVersion"), 2);
        assert_eq!(rpc.count("state_call"), 1);
    }
}
