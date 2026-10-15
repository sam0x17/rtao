//! Atomic metagraph files retaining the original runtime-API response.
//!
//! Enable the `persist` feature to save fetched graphs and load them without
//! network access. Loading requires the stored spec's metadata in the supplied
//! runtime cache. The separately fetched commitment maps are not persisted.

use std::{
    fmt,
    fs::{self, File, OpenOptions},
    io::{self, Write},
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};

use bytes::Bytes;
use parity_scale_codec::{Decode, Encode};
use rtao_client::{ClientError, RuntimeCache};
use rtao_primitives::{H256, MechId, NetUid};
use rtao_runtime::SelectiveMetagraphIndex;

use crate::{Metagraph, MetagraphError, MetagraphOptions, SnapshotInfo};

const MAGIC: &[u8; 4] = b"RTMG";
const VERSION: u8 = 1;
static NEXT_TEMP: AtomicU64 = AtomicU64::new(0);

// A source is also the persisted preset: full API method or the exact selective
// field indices. Enum indices are part of the version-1 file contract.
#[derive(Clone, Debug, PartialEq, Eq, Encode, Decode)]
pub(crate) enum Source {
    #[codec(index = 0)]
    FullSubnet,
    #[codec(index = 1)]
    FullMechanism,
    #[codec(index = 2)]
    Selective(Vec<u8>),
}

#[derive(Clone, Debug)]
pub(crate) struct Origin {
    pub(crate) genesis: H256,
    pub(crate) source: Source,
    pub(crate) at: SnapshotInfo,
    pub(crate) netuid: NetUid,
    pub(crate) mechid: MechId,
}

#[derive(Debug, Encode, Decode)]
struct Header {
    genesis: [u8; 32],
    block_hash: [u8; 32],
    block_number: u64,
    spec_version: u32,
    netuid: u16,
    mechid: u8,
    preset: Source,
}

/// Failure to save or restore a metagraph file.
#[derive(Debug)]
#[non_exhaustive]
pub enum PersistError {
    /// Filesystem work failed.
    Io(io::Error),
    /// The magic, SCALE header, identities or raw response are invalid.
    InvalidFormat(String),
    /// The file uses a format version this implementation does not support.
    UnsupportedVersion(u8),
    /// The supplied runtime cache belongs to another chain.
    GenesisMismatch {
        /// Genesis of the supplied cache.
        expected: H256,
        /// Genesis stored in the file.
        found: H256,
    },
    /// The graph lacks an original fetched response and chain provenance.
    MissingProvenance,
    /// The runtime was unavailable or could not be parsed from the offline cache.
    Runtime(ClientError),
    /// Runtime metadata rejected the persisted response.
    Decode(MetagraphError),
    /// A blocking filesystem or decoder worker failed to complete.
    Worker(String),
}

impl fmt::Display for PersistError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(error) => error.fmt(formatter),
            Self::InvalidFormat(detail) => write!(formatter, "invalid metagraph file: {detail}"),
            Self::UnsupportedVersion(version) => {
                write!(formatter, "unsupported metagraph file version {version}")
            }
            Self::GenesisMismatch { expected, found } => write!(
                formatter,
                "metagraph genesis 0x{} differs from runtime cache genesis 0x{}",
                hex::encode(found.0),
                hex::encode(expected.0)
            ),
            Self::MissingProvenance => {
                formatter.write_str("metagraph has no fetched response and chain provenance")
            }
            Self::Runtime(error) => error.fmt(formatter),
            Self::Decode(error) => error.fmt(formatter),
            Self::Worker(detail) => {
                write!(formatter, "metagraph persistence worker failed: {detail}")
            }
        }
    }
}

impl std::error::Error for PersistError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(error) => Some(error),
            Self::Runtime(error) => Some(error),
            Self::Decode(error) => Some(error),
            _ => None,
        }
    }
}

impl From<io::Error> for PersistError {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}

/// Atomically replace a file with a fetched metagraph's raw runtime response.
///
/// The sibling temporary file is flushed and synced before rename, then the
/// parent directory is synced. Failed writes remove their temporary file.
/// Synthetic graphs made with [`Metagraph::from_dynamic`] cannot be saved.
/// The separately fetched commitment maps are not included; fetch them again
/// when needed. Any commitment fields in the runtime response remain intact.
pub async fn save(graph: &Metagraph, path: impl AsRef<Path>) -> Result<(), PersistError> {
    let origin = graph
        .persistence_origin
        .as_ref()
        .ok_or(PersistError::MissingProvenance)?;
    if graph.raw().is_empty() {
        return Err(PersistError::MissingProvenance);
    }
    if graph.at != origin.at
        || graph.header.netuid != origin.netuid
        || graph.header.mechid != origin.mechid
    {
        return Err(PersistError::InvalidFormat(
            "graph identity was changed after fetching".into(),
        ));
    }
    if graph.source_netuid().map_err(PersistError::Decode)? != origin.netuid {
        return Err(PersistError::InvalidFormat(
            "runtime response subnet differs from fetched subnet".into(),
        ));
    }
    let header = Header {
        genesis: origin.genesis.0,
        block_hash: origin.at.hash.0,
        block_number: origin.at.number,
        spec_version: origin.at.spec_version,
        netuid: origin.netuid.0,
        mechid: origin.mechid.0,
        preset: origin.source.clone(),
    };
    let raw = graph.raw().clone();
    let path = path.as_ref().to_owned();
    tokio::task::spawn_blocking(move || write_atomic(&path, &header, &raw))
        .await
        .map_err(|error| PersistError::Worker(error.to_string()))?
}

/// Restore a raw metagraph with the stored spec's metadata, without any RPC.
///
/// Metadata must already exist in `cache`'s memory or shared disk directory.
/// The stored genesis must match the cache. The original file is never changed,
/// including when its header or response is malformed. v10 `.pt` pickles are
/// not supported. Restored graphs have no separately fetched commitment maps.
pub async fn load(path: impl AsRef<Path>, cache: &RuntimeCache) -> Result<Metagraph, PersistError> {
    let path = path.as_ref().to_owned();
    let (header, raw) = tokio::task::spawn_blocking(move || read_file(&path))
        .await
        .map_err(|error| PersistError::Worker(error.to_string()))??;
    if H256(header.genesis) != cache.genesis() {
        return Err(PersistError::GenesisMismatch {
            expected: cache.genesis(),
            found: H256(header.genesis),
        });
    }
    let netuid = NetUid::new(header.netuid)
        .map_err(|error| PersistError::InvalidFormat(error.to_string()))?;
    let mechid = MechId::new(header.mechid)
        .map_err(|error| PersistError::InvalidFormat(error.to_string()))?;
    if let Source::Selective(fields) = &header.preset
        && fields
            .iter()
            .any(|index| SelectiveMetagraphIndex::from_index(*index).is_none())
    {
        return Err(PersistError::InvalidFormat(
            "unknown selective metagraph field".into(),
        ));
    }
    let runtime = cache
        .cached_runtime(header.spec_version)
        .await
        .map_err(PersistError::Runtime)?;
    let at = SnapshotInfo {
        number: header.block_number,
        hash: H256(header.block_hash),
        spec_version: header.spec_version,
    };
    tokio::task::spawn_blocking(move || {
        let options = MetagraphOptions {
            commitments: false,
            ..MetagraphOptions::default()
        };
        let graph = match &header.preset {
            Source::FullSubnet => Metagraph::decode(raw, &runtime, netuid, at, options),
            Source::FullMechanism => {
                Metagraph::decode_mechanism(raw, &runtime, netuid, at, options)
            }
            Source::Selective(_) => {
                Metagraph::decode_selective(raw, &runtime, netuid, mechid, at, options)
            }
        };
        let mut graph = graph.map_err(PersistError::Decode)?.ok_or_else(|| {
            PersistError::InvalidFormat("runtime response contains no metagraph".into())
        })?;
        let source_netuid = graph.source_netuid().map_err(PersistError::Decode)?;
        if source_netuid != netuid || graph.header.mechid != mechid {
            return Err(PersistError::InvalidFormat(format!(
                "runtime response subnet {source_netuid}, mechanism {} differs from file subnet {netuid}, mechanism {mechid}",
                graph.header.mechid
            )));
        }
        graph.set_persistence_origin(H256(header.genesis), header.preset);
        Ok(graph)
    })
    .await
    .map_err(|error| PersistError::Worker(error.to_string()))?
}

fn read_file(path: &Path) -> Result<(Header, Bytes), PersistError> {
    let bytes = Bytes::from(fs::read(path)?);
    if bytes.get(..MAGIC.len()) != Some(MAGIC.as_slice()) {
        return Err(PersistError::InvalidFormat("expected RTMG magic".into()));
    }
    let version = bytes
        .get(MAGIC.len())
        .ok_or_else(|| PersistError::InvalidFormat("missing format version".into()))?;
    if *version != VERSION {
        return Err(PersistError::UnsupportedVersion(*version));
    }
    let mut input = &bytes[MAGIC.len() + 1..];
    let header = Header::decode(&mut input)
        .map_err(|error| PersistError::InvalidFormat(format!("SCALE header: {error}")))?;
    if input.is_empty() {
        return Err(PersistError::InvalidFormat(
            "missing runtime response".into(),
        ));
    }
    let raw = bytes.slice(bytes.len() - input.len()..);
    Ok((header, raw))
}

struct TemporaryFile(PathBuf);

impl Drop for TemporaryFile {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
    }
}

fn write_atomic(path: &Path, header: &Header, raw: &[u8]) -> Result<(), PersistError> {
    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    // create_new makes the cleanup guard own precisely the file we created.
    let (mut file, temporary) = loop {
        let sequence = NEXT_TEMP.fetch_add(1, Ordering::Relaxed);
        let candidate = parent.join(format!(
            ".rtao-metagraph-{}-{sequence}.tmp",
            std::process::id()
        ));
        match OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&candidate)
        {
            Ok(file) => break (file, TemporaryFile(candidate)),
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error.into()),
        }
    };
    file.write_all(MAGIC)?;
    file.write_all(&[VERSION])?;
    file.write_all(&header.encode())?;
    file.write_all(raw)?;
    file.sync_all()?;
    drop(file);
    fs::rename(&temporary.0, path)?;
    File::open(parent)?.sync_all()?;
    Ok(())
}
