//! Shared neuron state and durable, identity-checked JSON checkpoints.

use std::{
    collections::BTreeMap,
    fs::{self, File, OpenOptions},
    io::{self, Write},
    marker::PhantomData,
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex, RwLock, RwLockReadGuard, RwLockWriteGuard,
        atomic::{AtomicU64, Ordering},
    },
    time::{SystemTime, UNIX_EPOCH},
};

use rtao_metagraph::MetagraphDiff;
use rtao_primitives::{AccountId, NetUid};
use serde::{Deserialize, Serialize, de::DeserializeOwned};

/// Maximum serialized checkpoint or rotated generation size (64 MiB).
pub const MAX_CHECKPOINT_BYTES: u64 = 64 * 1024 * 1024;

/// Shared state whose standard-library guards cannot cross an await in a Send future.
///
/// Locks recover their contents after poisoning. Callbacks should release guards
/// before any await so another callback can use the state.
///
/// ```compile_fail
/// use rtao_neuron::StateCell;
/// fn require_send(_: impl std::future::Future<Output = ()> + Send) {}
/// let state = StateCell::new(0u64);
/// require_send(async move {
///     let mut guard = state.write();
///     std::future::ready(()).await;
///     *guard += 1;
/// });
/// ```
#[derive(Debug)]
pub struct StateCell<T>(Arc<RwLock<T>>);

impl<T> Clone for StateCell<T> {
    fn clone(&self) -> Self {
        Self(Arc::clone(&self.0))
    }
}
impl<T: Default> Default for StateCell<T> {
    fn default() -> Self {
        Self::new(T::default())
    }
}
impl<T> StateCell<T> {
    /// Put one state value behind a shared lock.
    pub fn new(state: T) -> Self {
        Self(Arc::new(RwLock::new(state)))
    }
    /// Borrow state, recovering a poisoned lock's contents.
    pub fn read(&self) -> RwLockReadGuard<'_, T> {
        self.0
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
    /// Mutably borrow state, recovering a poisoned lock's contents.
    pub fn write(&self) -> RwLockWriteGuard<'_, T> {
        self.0
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

/// Exponential moving averages indexed by stable hotkey identity.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct EmaScores {
    scores: BTreeMap<AccountId, f64>,
}
impl EmaScores {
    /// Read a hotkey's last score.
    pub fn get(&self, hotkey: &AccountId) -> Option<f64> {
        self.scores.get(hotkey).copied()
    }
    /// Iterate stable hotkey/score pairs in account order.
    pub fn iter(&self) -> impl Iterator<Item = (&AccountId, f64)> {
        self.scores.iter().map(|(account, score)| (account, *score))
    }
    /// Update one finite observation with an alpha in the inclusive range 0..=1.
    ///
    /// The first observation initializes the score. Invalid input leaves all
    /// scores unchanged, including when arithmetic would overflow.
    pub fn update(
        &mut self,
        hotkey: AccountId,
        observation: f64,
        alpha: f64,
    ) -> Result<f64, ScoreError> {
        if !observation.is_finite() || !alpha.is_finite() || !(0.0..=1.0).contains(&alpha) {
            return Err(ScoreError);
        }
        let score = self.scores.get(&hotkey).map_or(observation, |previous| {
            (1.0 - alpha) * previous + alpha * observation
        });
        if !score.is_finite() {
            return Err(ScoreError);
        }
        self.scores.insert(hotkey, score);
        Ok(score)
    }
    /// Drop identities replaced or deregistered in a new metagraph snapshot.
    pub fn apply(&mut self, diff: &MetagraphDiff) {
        for (_, previous, _) in &diff.replaced {
            self.scores.remove(previous);
        }
        for (_, previous) in &diff.deregistered {
            self.scores.remove(previous);
        }
    }
}

/// An invalid moving-average input or non-finite result.
#[derive(Clone, Copy, Debug, thiserror::Error, PartialEq, Eq)]
#[error("EMA observation must be finite and alpha must be between zero and one")]
pub struct ScoreError;

#[derive(Serialize, Deserialize)]
struct Envelope<T> {
    schema_version: u32,
    rtao_version: String,
    netuid: NetUid,
    hotkey: AccountId,
    block: u64,
    saved_at: u64,
    state: T,
}

/// A checkpoint loaded from the current file or one of its two predecessors.
#[derive(Debug)]
pub struct LoadedState<T> {
    /// Application state from the checkpoint.
    pub state: T,
    /// Finalized block accompanying the saved state.
    pub block: u64,
    /// Unix seconds at which the state was saved.
    pub saved_at: u64,
    /// Package version recorded by the saving process.
    pub rtao_version: String,
    /// Zero for the current checkpoint, one or two for a recovery generation.
    pub generation: u8,
}

/// A checkpoint read, write, identity, or schema failure.
#[derive(Debug, thiserror::Error)]
pub enum StateError {
    /// A checkpoint belongs to another subnet or signing identity.
    #[error("checkpoint identity differs from configured netuid {netuid} and hotkey {hotkey}")]
    Identity {
        /// Configured subnet.
        netuid: NetUid,
        /// Configured hotkey.
        hotkey: AccountId,
    },
    /// Application schema migration is required before loading.
    #[error("checkpoint schema {found} differs from expected schema {expected}")]
    Schema {
        /// Configured schema version.
        expected: u32,
        /// Checkpoint schema version.
        found: u32,
    },
    /// A filesystem operation failed.
    #[error("checkpoint filesystem operation failed at {path}: {source}")]
    Io {
        /// File or directory at which the operation failed.
        path: PathBuf,
        /// Underlying filesystem failure.
        source: io::Error,
    },
    /// Every available generation was invalid JSON or invalid application state.
    #[error("checkpoint JSON failed at {path}: {source}")]
    Json {
        /// Invalid checkpoint file.
        path: PathBuf,
        /// Underlying JSON failure.
        source: serde_json::Error,
    },
    /// The blocking worker panicked or the runtime stopped.
    #[error("checkpoint worker failed: {0}")]
    Worker(#[from] tokio::task::JoinError),
    /// The system clock predates the Unix epoch.
    #[error("checkpoint clock predates the Unix epoch")]
    Clock,
}

struct StoreInner {
    directory: PathBuf,
    netuid: NetUid,
    hotkey: AccountId,
    schema_version: u32,
    // The worker retains this lock even if its async caller is cancelled.
    gate: Mutex<()>,
}

/// Atomic JSON checkpoints with three generations and checked chain identity.
///
/// Serialization, deserialization and filesystem calls run on Tokio's blocking
/// pool. Cancelled save futures leave their worker running to complete the
/// atomic replacement, and subsequent operations wait for that worker.
pub struct StateStore<T> {
    inner: Arc<StoreInner>,
    marker: PhantomData<fn() -> T>,
}
impl<T> Clone for StateStore<T> {
    fn clone(&self) -> Self {
        Self {
            inner: Arc::clone(&self.inner),
            marker: PhantomData,
        }
    }
}
impl<T> StateStore<T> {
    /// Bind an existing or future directory to one identity and application schema.
    ///
    /// This constructor performs no filesystem operations.
    pub fn new(
        directory: impl Into<PathBuf>,
        netuid: NetUid,
        hotkey: AccountId,
        schema_version: u32,
    ) -> Self {
        Self {
            inner: Arc::new(StoreInner {
                directory: directory.into(),
                netuid,
                hotkey,
                schema_version,
                gate: Mutex::new(()),
            }),
            marker: PhantomData,
        }
    }
    /// Directory holding `state.json`, `state.1.json`, and `state.2.json`.
    pub fn directory(&self) -> &Path {
        &self.inner.directory
    }
}
impl<T: Serialize + Send + Sync + 'static> StateStore<T> {
    /// Serialize a consistent state view and atomically install a checkpoint.
    /// The complete JSON envelope must fit the 64 MiB checkpoint limit.
    pub async fn save(&self, state: &StateCell<T>, block: u64) -> Result<(), StateError> {
        let inner = Arc::clone(&self.inner);
        let state = state.clone();
        tokio::task::spawn_blocking(move || {
            let _gate = inner
                .gate
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let saved_at = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map_err(|_| StateError::Clock)?
                .as_secs();
            let state = state.read();
            let envelope = Envelope {
                schema_version: inner.schema_version,
                rtao_version: env!("CARGO_PKG_VERSION").into(),
                netuid: inner.netuid,
                hotkey: inner.hotkey,
                block,
                saved_at,
                state: &*state,
            };
            let bytes = serde_json::to_vec(&envelope).map_err(|source| StateError::Json {
                path: inner.directory.join("state.json"),
                source,
            })?;
            drop(state);
            write_checkpoint(&inner.directory, &bytes)
        })
        .await?
    }
}
impl<T: DeserializeOwned + Send + 'static> StateStore<T> {
    /// Load the newest valid generation, refusing identity or schema mismatches.
    ///
    /// Missing files yield `None`. Damaged JSON falls back to a previous
    /// generation. Identity/schema failures fail immediately, since recovery
    /// must never silently adopt a different application or neuron identity.
    /// Every generation must be a regular file no larger than 64 MiB.
    pub async fn load(&self) -> Result<Option<LoadedState<T>>, StateError> {
        let inner = Arc::clone(&self.inner);
        tokio::task::spawn_blocking(move || {
            let _gate = inner
                .gate
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let mut invalid = None;
            for generation in 0..3 {
                let path = generation_path(&inner.directory, generation);
                let bytes = match rtao_client::bounded_file::read(&path, MAX_CHECKPOINT_BYTES) {
                    Ok(bytes) => bytes,
                    Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
                    Err(source) => return Err(StateError::Io { path, source }),
                };
                // Check metadata before deserializing application state: a wrong
                // identity must not be concealed by an invalid state payload.
                let envelope: Envelope<serde_json::Value> =
                    match rtao_primitives::json::from_slice(&bytes) {
                        Ok(envelope) => envelope,
                        Err(source) => {
                            invalid = Some(StateError::Json { path, source });
                            continue;
                        }
                    };
                if envelope.netuid != inner.netuid || envelope.hotkey != inner.hotkey {
                    return Err(StateError::Identity {
                        netuid: inner.netuid,
                        hotkey: inner.hotkey,
                    });
                }
                if envelope.schema_version != inner.schema_version {
                    return Err(StateError::Schema {
                        expected: inner.schema_version,
                        found: envelope.schema_version,
                    });
                }
                let envelope: Envelope<T> = match rtao_primitives::json::from_slice(&bytes) {
                    Ok(envelope) => envelope,
                    Err(source) => {
                        invalid = Some(StateError::Json { path, source });
                        continue;
                    }
                };
                return Ok(Some(LoadedState {
                    state: envelope.state,
                    block: envelope.block,
                    saved_at: envelope.saved_at,
                    rtao_version: envelope.rtao_version,
                    generation,
                }));
            }
            invalid.map_or(Ok(None), Err)
        })
        .await?
    }
}

fn generation_path(directory: &Path, generation: u8) -> PathBuf {
    directory.join(if generation == 0 {
        "state.json".into()
    } else {
        format!("state.{generation}.json")
    })
}
static NEXT_TEMP: AtomicU64 = AtomicU64::new(0);

fn write_checkpoint(directory: &Path, bytes: &[u8]) -> Result<(), StateError> {
    let temporary = directory.join(format!(
        ".state.{}-{}.tmp",
        std::process::id(),
        NEXT_TEMP.fetch_add(1, Ordering::Relaxed)
    ));
    write_checkpoint_with_temporary(directory, bytes, temporary)
}

// The chosen path is explicit so collision fixtures exercise the publication
// path without racing the process-wide temporary sequence.
fn write_checkpoint_with_temporary(
    directory: &Path,
    bytes: &[u8],
    temporary: PathBuf,
) -> Result<(), StateError> {
    if bytes.len() as u64 > MAX_CHECKPOINT_BYTES {
        return Err(StateError::Io {
            path: generation_path(directory, 0),
            source: io::Error::new(
                io::ErrorKind::InvalidData,
                "checkpoint exceeds 64 MiB limit",
            ),
        });
    }
    fs::create_dir_all(directory).map_err(|source| StateError::Io {
        path: directory.into(),
        source,
    })?;
    // Declare the guard first so an I/O error drops the open file before the
    // guard removes its path, including on platforms that forbid open unlink.
    let (temporary, mut file) = TemporaryCheckpoint::create(temporary)?;
    file.write_all(bytes)
        .and_then(|()| file.sync_all())
        .map_err(|source| StateError::Io {
            path: temporary.path.clone(),
            source,
        })?;
    drop(file);
    // Copy through separate synced temporary files. The current checkpoint
    // stays present until the final rename, even if the process is killed
    // while its previous generations are being updated.
    rotate(directory, 1, 2)?;
    rotate(directory, 0, 1)?;
    temporary.publish(&generation_path(directory, 0))?;
    sync_directory(directory)
}

fn rotate(directory: &Path, from: u8, to: u8) -> Result<(), StateError> {
    let temporary = directory.join(format!(
        ".state.{to}.{}-{}.tmp",
        std::process::id(),
        NEXT_TEMP.fetch_add(1, Ordering::Relaxed)
    ));
    rotate_with_temporary(directory, from, to, temporary)
}

fn rotate_with_temporary(
    directory: &Path,
    from: u8,
    to: u8,
    temporary: PathBuf,
) -> Result<(), StateError> {
    let source_path = generation_path(directory, from);
    match fs::symlink_metadata(&source_path) {
        Ok(_) => {}
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(source) => {
            return Err(StateError::Io {
                path: source_path,
                source,
            });
        }
    }
    // Acquisition still validates the opened descriptor and bounds copying;
    // the existence check above only identifies an absent older generation.
    let (temporary, mut destination) = TemporaryCheckpoint::create(temporary)?;
    rtao_client::bounded_file::copy(&source_path, &mut destination, MAX_CHECKPOINT_BYTES).map_err(
        |source| StateError::Io {
            path: source_path.clone(),
            source,
        },
    )?;
    destination.sync_all().map_err(|source| StateError::Io {
        path: temporary.path.clone(),
        source,
    })?;
    drop(destination);
    temporary.publish(&generation_path(directory, to))?;
    sync_directory(directory)
}

struct TemporaryCheckpoint {
    path: PathBuf,
    remove_on_drop: bool,
}

impl TemporaryCheckpoint {
    fn create(path: PathBuf) -> Result<(Self, File), StateError> {
        let file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
            .map_err(|source| StateError::Io {
                path: path.clone(),
                source,
            })?;
        // Own cleanup only after create_new succeeds. An existing candidate
        // belongs to somebody else and must survive an acquisition failure.
        Ok((
            Self {
                path,
                remove_on_drop: true,
            },
            file,
        ))
    }

    fn publish(mut self, destination: &Path) -> Result<(), StateError> {
        fs::rename(&self.path, destination).map_err(|source| StateError::Io {
            path: self.path.clone(),
            source,
        })?;
        // The owned file now has its final name. Do not remove a later file
        // that might appear at the retired temporary path.
        self.remove_on_drop = false;
        Ok(())
    }
}

impl Drop for TemporaryCheckpoint {
    fn drop(&mut self) {
        if self.remove_on_drop {
            let _ = fs::remove_file(&self.path);
        }
    }
}
fn sync_directory(directory: &Path) -> Result<(), StateError> {
    File::open(directory)
        .and_then(|file| file.sync_all())
        .map_err(|source| StateError::Io {
            path: directory.into(),
            source,
        })
}

#[cfg(test)]
mod tests {
    #![allow(clippy::expect_used)]
    use super::*;
    use rtao_primitives::Uid;

    struct Fixture(PathBuf);
    impl Fixture {
        fn new() -> Self {
            let root = std::env::var_os("RTAO_TEMP_ROOT")
                .map(PathBuf::from)
                .unwrap_or_else(|| {
                    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/neuron-tests")
                });
            let root = root.join("tmp").join(format!(
                "neuron-state-{}-{}",
                std::process::id(),
                NEXT_TEMP.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir_all(&root).expect("create isolated state fixture");
            Self(root)
        }
        fn store<T>(&self) -> StateStore<T> {
            StateStore::new(&self.0, NetUid(1), AccountId([1; 32]), 1)
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            fs::remove_dir_all(&self.0)
                .expect("remove isolated fixture after all child processes and workers finish");
        }
    }

    #[test]
    #[allow(clippy::panic)] // A deliberate panic verifies poisoned-lock recovery.
    fn cell_clone_shares_state_without_requiring_clone_and_poison_recovers() {
        struct NonClone(u64);
        let state = StateCell::new(NonClone(2));
        state.clone().write().0 += 1;
        assert_eq!(state.read().0, 3);
        let poisoned = state.clone();
        assert!(
            std::thread::spawn(move || {
                let mut guard = poisoned.write();
                guard.0 = 4;
                panic!("intentional poison");
            })
            .join()
            .is_err()
        );
        assert_eq!(state.read().0, 4);
    }

    #[test]
    fn ema_identity_replacement_drops_old_hotkey_and_preserves_other_scores() {
        let mut state = EmaScores::default();
        state
            .update(AccountId([1; 32]), 10.0, 0.2)
            .expect("initialize");
        state
            .update(AccountId([2; 32]), 20.0, 0.2)
            .expect("initialize");
        state
            .update(AccountId([3; 32]), 30.0, 0.2)
            .expect("initialize");
        assert_eq!(
            state.update(AccountId([1; 32]), 20.0, 0.2).expect("EMA"),
            12.0
        );
        let previous = state.clone();
        assert!(state.update(AccountId([1; 32]), f64::NAN, 0.2).is_err());
        assert!(state.update(AccountId([1; 32]), 20.0, 2.0).is_err());
        assert_eq!(state, previous);
        state.apply(&MetagraphDiff {
            replaced: vec![(Uid(0), AccountId([1; 32]), AccountId([4; 32]))],
            deregistered: vec![(Uid(1), AccountId([2; 32]))],
            ..Default::default()
        });
        assert_eq!(state.get(&AccountId([1; 32])), None);
        assert_eq!(state.get(&AccountId([4; 32])), None);
        assert_eq!(state.get(&AccountId([2; 32])), None);
        assert_eq!(state.get(&AccountId([3; 32])), Some(30.0));
    }

    #[tokio::test]
    async fn checkpoints_keep_three_generations_and_recover_truncated_latest() {
        let fixture = Fixture::new();
        let store = fixture.store::<u64>();
        assert!(store.load().await.expect("missing checkpoint").is_none());
        for block in 1..=4 {
            store
                .save(&StateCell::new(block), block)
                .await
                .expect("checkpoint");
        }
        assert_eq!(fs::read_dir(&fixture.0).expect("entries").count(), 3);
        assert_eq!(
            store.load().await.expect("current").expect("present").state,
            4
        );
        fs::write(fixture.0.join("state.json"), b"{\"truncated")
            .expect("simulate interrupted external write");
        let recovered = store
            .load()
            .await
            .expect("recover")
            .expect("previous exists");
        assert_eq!(
            (recovered.state, recovered.block, recovered.generation),
            (3, 3, 1)
        );
    }

    #[tokio::test]
    async fn identity_and_schema_are_checked_before_application_state_and_recovery() {
        let fixture = Fixture::new();
        let store = fixture.store::<u64>();
        store
            .save(&StateCell::new(42), 3)
            .await
            .expect("checkpoint");
        let other = StateStore::<String>::new(&fixture.0, NetUid(2), AccountId([1; 32]), 1);
        assert!(matches!(
            other.load().await,
            Err(StateError::Identity { .. })
        ));
        let other = StateStore::<u64>::new(&fixture.0, NetUid(1), AccountId([2; 32]), 1);
        assert!(matches!(
            other.load().await,
            Err(StateError::Identity { .. })
        ));
        let other = StateStore::<String>::new(&fixture.0, NetUid(1), AccountId([1; 32]), 2);
        assert!(matches!(
            other.load().await,
            Err(StateError::Schema {
                expected: 2,
                found: 1
            })
        ));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn serialization_runs_off_the_reactor_and_never_requires_clone() {
        struct NonClone(std::thread::ThreadId);
        impl Serialize for NonClone {
            fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
                assert_ne!(self.0, std::thread::current().id());
                serializer.serialize_u64(73)
            }
        }
        let fixture = Fixture::new();
        fixture
            .store::<NonClone>()
            .save(&StateCell::new(NonClone(std::thread::current().id())), 9)
            .await
            .expect("save without cloning on the blocking worker");
        let loaded = fixture
            .store::<u64>()
            .load()
            .await
            .expect("load worker")
            .expect("present");
        assert_eq!((loaded.state, loaded.block), (73, 9));
    }

    #[tokio::test]
    async fn failed_replace_preserves_directory_and_removes_only_its_temporary_files() {
        let fixture = Fixture::new();
        let destination = fixture.0.join("state.json");
        fs::create_dir(&destination).expect("directory destination");
        fs::write(destination.join("retain"), b"unrelated").expect("unrelated file");
        assert!(
            fixture
                .store::<u64>()
                .save(&StateCell::new(1), 1)
                .await
                .is_err()
        );
        assert_eq!(
            fs::read(destination.join("retain")).expect("preserved file"),
            b"unrelated"
        );
        assert_eq!(fs::read_dir(&fixture.0).expect("parent entries").count(), 1);
    }

    #[test]
    fn checkpoint_temporary_collision_preserves_candidate_and_existing_generations() {
        let fixture = Fixture::new();
        let originals: Vec<_> = (0..3)
            .map(|generation| {
                let path = generation_path(&fixture.0, generation);
                let bytes = format!("existing generation {generation}").into_bytes();
                fs::write(&path, &bytes).expect("existing checkpoint");
                (path, bytes)
            })
            .collect();
        let collision = fixture.0.join("chosen-checkpoint-temporary.tmp");
        fs::write(&collision, b"unrelated existing candidate").expect("collision candidate");

        assert!(matches!(
            write_checkpoint_with_temporary(&fixture.0, b"new checkpoint", collision.clone()),
            Err(StateError::Io { path, source })
                if path == collision && source.kind() == io::ErrorKind::AlreadyExists
        ));

        assert_eq!(
            fs::read(&collision).expect("collision retained"),
            b"unrelated existing candidate"
        );
        for (path, bytes) in originals {
            assert_eq!(fs::read(path).expect("generation retained"), bytes);
        }
        assert_eq!(fs::read_dir(&fixture.0).expect("entries").count(), 4);
    }

    #[test]
    fn rotation_temporary_collision_preserves_candidate_and_existing_generations() {
        for (from, to) in [(0, 1), (1, 2)] {
            let fixture = Fixture::new();
            let originals: Vec<_> = (0..3)
                .map(|generation| {
                    let path = generation_path(&fixture.0, generation);
                    let bytes = format!("existing generation {generation}").into_bytes();
                    fs::write(&path, &bytes).expect("existing checkpoint");
                    (path, bytes)
                })
                .collect();
            let collision = fixture.0.join("chosen-rotation-temporary.tmp");
            fs::write(&collision, b"unrelated existing candidate").expect("collision candidate");

            assert!(matches!(
                rotate_with_temporary(&fixture.0, from, to, collision.clone()),
                Err(StateError::Io { path, source })
                    if path == collision && source.kind() == io::ErrorKind::AlreadyExists
            ));

            assert_eq!(
                fs::read(&collision).expect("collision retained"),
                b"unrelated existing candidate"
            );
            for (path, bytes) in originals {
                assert_eq!(fs::read(path).expect("generation retained"), bytes);
            }
            assert_eq!(fs::read_dir(&fixture.0).expect("entries").count(), 4);
        }
    }

    #[tokio::test]
    async fn oversized_load_refuses_the_current_file_without_adopting_an_older_generation() {
        let fixture = Fixture::new();
        let store = fixture.store::<u64>();
        for block in 1..=2 {
            store
                .save(&StateCell::new(block), block)
                .await
                .expect("checkpoint");
        }
        let path = generation_path(&fixture.0, 0);
        let old_path = generation_path(&fixture.0, 1);
        let old = fs::read(&old_path).expect("older generation");
        OpenOptions::new()
            .write(true)
            .open(&path)
            .expect("current file")
            .set_len(MAX_CHECKPOINT_BYTES + 1)
            .expect("sparse oversized checkpoint");
        assert!(matches!(
            store.load().await,
            Err(StateError::Io { path: failed_path, source })
                if failed_path == path && source.kind() == io::ErrorKind::InvalidData
        ));
        assert_eq!(
            fs::metadata(&path).expect("current preserved").len(),
            MAX_CHECKPOINT_BYTES + 1
        );
        assert_eq!(fs::read(old_path).expect("older preserved"), old);
        assert_eq!(fs::read_dir(&fixture.0).expect("entries").count(), 2);
    }

    #[tokio::test]
    async fn oversized_rotation_sources_preserve_current_and_remove_all_owned_temporaries() {
        for generation in 0..=1 {
            let fixture = Fixture::new();
            let store = fixture.store::<u64>();
            store
                .save(&StateCell::new(1), 1)
                .await
                .expect("initial checkpoint");
            let current_path = generation_path(&fixture.0, 0);
            let current = fs::read(&current_path).expect("initial contents");
            let source_path = generation_path(&fixture.0, generation);
            let source = OpenOptions::new()
                .write(true)
                .create(true)
                .truncate(false)
                .open(&source_path)
                .expect("rotation source");
            source
                .set_len(MAX_CHECKPOINT_BYTES + 1)
                .expect("sparse oversized rotation source");
            drop(source);
            assert!(matches!(
                store.save(&StateCell::new(2), 2).await,
                Err(StateError::Io { path, source })
                    if path == source_path && source.kind() == io::ErrorKind::InvalidData
            ));
            assert_eq!(
                fs::metadata(&source_path).expect("source retained").len(),
                MAX_CHECKPOINT_BYTES + 1
            );
            if generation == 0 {
                let mut prefix = vec![0; current.len()];
                std::io::Read::read_exact(
                    &mut File::open(&current_path).expect("current retained"),
                    &mut prefix,
                )
                .expect("initial prefix");
                assert_eq!(prefix, current);
            } else {
                assert_eq!(fs::read(&current_path).expect("current retained"), current);
            }
            assert_eq!(
                fs::read_dir(&fixture.0)
                    .expect("no temporary files")
                    .count(),
                usize::from(generation) + 1
            );
        }
    }

    #[test]
    fn oversized_checkpoint_write_preserves_existing_generations_without_temporary_output() {
        let fixture = Fixture::new();
        for bytes in [b"oldest".as_slice(), b"previous", b"current"] {
            write_checkpoint(&fixture.0, bytes).expect("initial checkpoint");
        }
        let originals: Vec<_> = (0..3)
            .map(|generation| {
                let path = generation_path(&fixture.0, generation);
                let bytes = fs::read(&path).expect("initial generation");
                (path, bytes)
            })
            .collect();
        let bytes = vec![0; (MAX_CHECKPOINT_BYTES + 1) as usize];
        let path = generation_path(&fixture.0, 0);
        assert!(matches!(
            write_checkpoint(&fixture.0, &bytes),
            Err(StateError::Io { path: failed_path, source })
                if failed_path == path && source.kind() == io::ErrorKind::InvalidData
        ));
        for (path, expected) in originals {
            assert_eq!(fs::read(path).expect("preserved checkpoint"), expected);
        }
        assert_eq!(
            fs::read_dir(&fixture.0)
                .expect("no temporary files")
                .count(),
            3
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn broken_rotation_symlinks_are_errors_instead_of_missing_generations() {
        let fixture = Fixture::new();
        let store = fixture.store::<u64>();
        store
            .save(&StateCell::new(1), 1)
            .await
            .expect("initial checkpoint");
        let current_path = generation_path(&fixture.0, 0);
        let current = fs::read(&current_path).expect("initial contents");
        let broken = generation_path(&fixture.0, 1);
        std::os::unix::fs::symlink(fixture.0.join("absent"), &broken)
            .expect("broken generation symlink");
        assert!(matches!(
            store.save(&StateCell::new(2), 2).await,
            Err(StateError::Io { path, source })
                if path == broken && source.kind() == io::ErrorKind::NotFound
        ));
        assert_eq!(fs::read(current_path).expect("current retained"), current);
        assert!(
            fs::symlink_metadata(broken)
                .expect("broken link retained")
                .file_type()
                .is_symlink()
        );
        assert_eq!(fs::read_dir(&fixture.0).expect("no temporaries").count(), 2);
    }

    #[tokio::test]
    async fn application_flattened_floats_use_exact_json_input_adapter() {
        #[derive(Debug, Serialize, Deserialize, PartialEq)]
        struct FloatingState {
            #[serde(flatten)]
            values: BTreeMap<String, f64>,
        }
        let fixture = Fixture::new();
        let state = FloatingState {
            values: BTreeMap::from([("score".into(), 1e-5)]),
        };
        let store = fixture.store::<FloatingState>();
        store
            .save(&StateCell::new(state), 1)
            .await
            .expect("flattened float save");
        let path = fixture.0.join("state.json");
        let bytes = fs::read_to_string(&path)
            .expect("saved JSON")
            .replace("0.00001", "1e-05")
            .replace("1e-5", "1e-05");
        assert!(bytes.contains("1e-05"));
        fs::write(path, bytes).expect("noncanonical decimal spelling");
        assert_eq!(
            store
                .load()
                .await
                .expect("adapted float load")
                .expect("present")
                .state
                .values["score"],
            1e-5
        );
    }

    #[tokio::test]
    async fn cancelling_save_keeps_worker_serialized_until_atomic_install_finishes() {
        type Gate = Arc<(Mutex<bool>, std::sync::Condvar)>;
        struct Release(Gate);
        impl Drop for Release {
            fn drop(&mut self) {
                let (released, wake) = &*self.0;
                *released
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner) = true;
                wake.notify_all();
            }
        }
        struct BlockingState {
            value: u64,
            gate: Gate,
            entered: Arc<tokio::sync::Notify>,
        }
        impl Serialize for BlockingState {
            fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
                self.entered.notify_one();
                let (released, wake) = &*self.gate;
                let mut released = released
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                while !*released {
                    released = wake
                        .wait(released)
                        .unwrap_or_else(std::sync::PoisonError::into_inner);
                }
                serializer.serialize_u64(self.value)
            }
        }
        impl<'de> Deserialize<'de> for BlockingState {
            fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
                Ok(Self {
                    value: u64::deserialize(deserializer)?,
                    gate: Arc::new((Mutex::new(true), std::sync::Condvar::new())),
                    entered: Arc::new(tokio::sync::Notify::new()),
                })
            }
        }
        let fixture = Fixture::new();
        let store = fixture.store::<BlockingState>();
        let gate = Arc::new((Mutex::new(false), std::sync::Condvar::new()));
        let release = Release(gate.clone());
        let entered = Arc::new(tokio::sync::Notify::new());
        let state = StateCell::new(BlockingState {
            value: 81,
            gate,
            entered: entered.clone(),
        });
        let save_store = store.clone();
        let save = tokio::spawn(async move { save_store.save(&state, 81).await });
        entered.notified().await;
        save.abort();
        assert!(save.await.expect_err("cancelled async save").is_cancelled());
        let load = tokio::spawn(async move { store.load().await });
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        let waited_for_worker = !load.is_finished();
        drop(release);
        // Await the worker before the fixture is eligible for cleanup, even if
        // the assertion about cancellation behavior fails.
        let loaded = load
            .await
            .expect("load task")
            .expect("serialized load")
            .expect("installed checkpoint");
        assert!(waited_for_worker);
        assert_eq!((loaded.state.value, loaded.block), (81, 81));
    }

    // Run in a child process so the parent can kill the validator between saves.
    #[test]
    fn checkpoint_child_process() {
        let Some(directory) = std::env::var_os("RTAO_CHECKPOINT_CHILD_DIR") else {
            return;
        };
        let runtime = tokio::runtime::Runtime::new().expect("child runtime");
        runtime.block_on(async {
            let store = StateStore::<EmaScores>::new(
                PathBuf::from(&directory),
                NetUid(1),
                AccountId([1; 32]),
                1,
            );
            let mut state = store
                .load()
                .await
                .expect("child load")
                .map_or_else(EmaScores::default, |loaded| loaded.state);
            let score = state.get(&AccountId([5; 32])).unwrap_or(0.0) + 1.0;
            state
                .update(AccountId([5; 32]), score, 1.0)
                .expect("child update");
            store
                .save(&StateCell::new(state), 100 + score as u64)
                .await
                .expect("child save");
            fs::write(PathBuf::from(directory).join("ready"), b"saved").expect("child ready");
            loop {
                tokio::time::sleep(std::time::Duration::from_secs(1)).await;
            }
        });
    }

    #[test]
    fn validator_restart_resumes_state() {
        let fixture = Fixture::new();
        for _ in 0..2 {
            let ready = fixture.0.join("ready");
            if ready.exists() {
                fs::remove_file(&ready).expect("clear ready");
            }
            let mut child =
                std::process::Command::new(std::env::current_exe().expect("test executable"))
                    .args([
                        "--exact",
                        "state::tests::checkpoint_child_process",
                        "--nocapture",
                    ])
                    .env("RTAO_CHECKPOINT_CHILD_DIR", &fixture.0)
                    .stdout(std::process::Stdio::null())
                    .stderr(std::process::Stdio::null())
                    .spawn()
                    .expect("spawn checkpoint validator");
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(15);
            while !ready.exists()
                && std::time::Instant::now() < deadline
                && child.try_wait().expect("poll child").is_none()
            {
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
            let persisted = ready.exists();
            let _ = child.kill();
            child.wait().expect("reap child before removing fixture");
            assert!(persisted, "child saved before kill");
        }
        let loaded = tokio::runtime::Runtime::new()
            .expect("restart runtime")
            .block_on(fixture.store::<EmaScores>().load())
            .expect("restart load")
            .expect("checkpoint exists");
        assert_eq!(loaded.state.get(&AccountId([5; 32])), Some(2.0));
        assert_eq!(loaded.block, 102);
    }
}
