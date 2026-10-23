//! Exclusive atomic native storage. Lock files are never unlinked.
use super::{JOURNAL_SCHEMA, JournalError, JournalImage, JournalLease, JournalStore};
use std::{
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::{
        Mutex,
        atomic::{AtomicU64, Ordering},
    },
};
/// Maximum encoded image. Capacity exhaustion refuses work without deleting evidence.
pub const MAX_JOURNAL_BYTES: u64 = 64 * 1024 * 1024;
/// Practical native store: exclusive OS lock, bounded JSON, synced atomic replacement.
///
/// Use a local filesystem with working file locks, atomic rename and directory
/// fsync. Remote/network filesystems need a separately qualified JournalStore.
pub struct NativeJournalStore {
    path: PathBuf,
    _lock: File,
    serial: Mutex<()>,
    owners: std::sync::Arc<Mutex<std::collections::BTreeSet<String>>>,
}
fn error(e: impl std::fmt::Display) -> JournalError {
    JournalError::Storage(e.to_string())
}
fn regular(path: &Path) -> Result<(), JournalError> {
    match fs::symlink_metadata(path) {
        Ok(m) if !m.is_file() || m.file_type().is_symlink() => {
            Err(error("journal path must be a regular nonsymlink file"))
        }
        Ok(_) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(error(e)),
    }
}
fn options() -> OpenOptions {
    let mut o = OpenOptions::new();
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        o.mode(0o600)
            .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
    }
    o
}
impl NativeJournalStore {
    /// Acquire an exclusive lifetime lock; preserve data on every refusal.
    pub fn open(path: impl AsRef<Path>) -> Result<Self, JournalError> {
        Self::open_with_directory_sync(path, |directory| File::open(directory)?.sync_all())
    }
    fn open_with_directory_sync(
        path: impl AsRef<Path>,
        mut sync_directory: impl FnMut(&Path) -> std::io::Result<()>,
    ) -> Result<Self, JournalError> {
        let path = path.as_ref();
        let parent = path
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or_else(|| Path::new("."));
        fs::create_dir_all(parent).map_err(error)?;
        let parent = fs::canonicalize(parent).map_err(error)?;
        // File publication cannot rescue an ancestor name lost after mkdir.
        // Sync the whole resolved ancestry before any journal publication,
        // including directories left behind by an interrupted earlier open.
        for directory in parent.ancestors() {
            sync_directory(directory).map_err(error)?;
        }
        let path = parent.join(
            path.file_name()
                .ok_or_else(|| error("missing journal filename"))?,
        );
        regular(&path)?;
        let lock_path = path.with_file_name(format!(
            "{}.lock",
            path.file_name()
                .ok_or_else(|| error("missing filename"))?
                .to_string_lossy()
        ));
        regular(&lock_path)?;
        let lock = options()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(lock_path)
            .map_err(error)?;
        lock.try_lock().map_err(|_| JournalError::Owned)?;
        let store = Self {
            path,
            _lock: lock,
            serial: Mutex::new(()),
            owners: Default::default(),
        };
        if !store.path.exists() {
            store.write(&JournalImage::default(), false)?;
        } else {
            let mut image = store.read()?;
            match image.schema_version {
                JOURNAL_SCHEMA => {}
                1 => {
                    image.schema_version = JOURNAL_SCHEMA;
                    image.revision = image
                        .revision
                        .checked_add(1)
                        .ok_or_else(|| error("generation exhausted"))?;
                    store.write(&image, false)?;
                }
                v => return Err(JournalError::Schema(v)),
            }
        }
        Ok(store)
    }
    fn read(&self) -> Result<JournalImage, JournalError> {
        regular(&self.path)?;
        let file = options().read(true).open(&self.path).map_err(error)?;
        let metadata = file.metadata().map_err(error)?;
        if !metadata.is_file() || metadata.len() > MAX_JOURNAL_BYTES {
            return Err(error("journal must be a bounded regular file"));
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            if metadata.permissions().mode() & 0o077 != 0 {
                return Err(error(
                    "journal permissions must exclude group and other access",
                ));
            }
        }
        let mut bytes = Vec::new();
        file.take(MAX_JOURNAL_BYTES + 1)
            .read_to_end(&mut bytes)
            .map_err(error)?;
        if bytes.len() as u64 > MAX_JOURNAL_BYTES {
            return Err(error("journal exceeds its byte limit"));
        }
        serde_json::from_slice(&bytes)
            .map_err(|_| error("invalid journal; original bytes retained"))
    }
    fn write(&self, image: &JournalImage, interrupt: bool) -> Result<(), JournalError> {
        let bytes = serde_json::to_vec(image).map_err(|_| error("journal serialization failed"))?;
        if bytes.len() as u64 > MAX_JOURNAL_BYTES {
            return Err(error(
                "journal capacity exceeded; migrate without discarding unresolved records",
            ));
        }
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let temp = self.path.with_file_name(format!(
            ".journal-write-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let mut f = options()
            .write(true)
            .create_new(true)
            .open(&temp)
            .map_err(error)?;
        f.write_all(&bytes).map_err(error)?;
        f.sync_all().map_err(error)?;
        if interrupt {
            return Err(error("injected interruption before atomic publication"));
        }
        regular(&self.path)?;
        fs::rename(&temp, &self.path).map_err(error)?;
        File::open(
            self.path
                .parent()
                .ok_or_else(|| error("missing journal parent"))?,
        )
        .and_then(|f| f.sync_all())
        .map_err(error)
    }
}
impl JournalStore for NativeJournalStore {
    fn claim_scope(&self, scope: &str) -> Result<Box<dyn JournalLease>, JournalError> {
        if !self
            .owners
            .lock()
            .map_err(|_| JournalError::Owned)?
            .insert(scope.to_owned())
        {
            return Err(JournalError::Owned);
        }
        Ok(Box::new(ScopeLease {
            owners: self.owners.clone(),
            scope: scope.to_owned(),
        }))
    }
    fn load(&self) -> Result<JournalImage, JournalError> {
        let _guard = self
            .serial
            .lock()
            .map_err(|_| error("store mutex poisoned"))?;
        let image = self.read()?;
        if image.schema_version != JOURNAL_SCHEMA {
            return Err(JournalError::Schema(image.schema_version));
        }
        Ok(image)
    }
    fn compare_exchange(&self, expected: u64, image: &JournalImage) -> Result<(), JournalError> {
        let _guard = self
            .serial
            .lock()
            .map_err(|_| error("store mutex poisoned"))?;
        let before = self.read()?;
        if before.revision != expected
            || image.revision
                != expected
                    .checked_add(1)
                    .ok_or_else(|| error("generation exhausted"))?
        {
            return Err(JournalError::Conflict);
        }
        if image.schema_version != JOURNAL_SCHEMA {
            return Err(JournalError::Schema(image.schema_version));
        }
        self.write(image, false)
    }
}
struct ScopeLease {
    owners: std::sync::Arc<Mutex<std::collections::BTreeSet<String>>>,
    scope: String,
}
impl Drop for ScopeLease {
    fn drop(&mut self) {
        if let Ok(mut owners) = self.owners.lock() {
            owners.remove(&self.scope);
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::panic)]
    use super::*;
    #[test]
    fn ancestry_barrier_precedes_publication_and_retries_existing_directories() {
        let root = PathBuf::from(std::env::var_os("RTAO_TEST_TMPDIR").unwrap())
            .join(format!("ancestry-{}", std::process::id()));
        fs::create_dir_all(&root).unwrap();
        let root = fs::canonicalize(root).unwrap();
        let path = root.join("first/second/journal.json");
        let fail = |directory: &Path| {
            assert!(
                !path.exists(),
                "publication preceded ancestor synchronization"
            );
            if directory == root {
                return Err(std::io::Error::other("injected ancestor sync failure"));
            }
            File::open(directory)?.sync_all()
        };
        assert!(NativeJournalStore::open_with_directory_sync(&path, fail).is_err());
        assert!(!path.exists());
        assert!(path.parent().unwrap().exists());
        let mut synced = Vec::new();
        let store = NativeJournalStore::open_with_directory_sync(&path, |directory| {
            assert!(!path.exists());
            File::open(directory)?.sync_all()?;
            synced.push(directory.to_path_buf());
            Ok(())
        })
        .unwrap();
        let expected = path
            .parent()
            .unwrap()
            .ancestors()
            .map(Path::to_path_buf)
            .collect::<Vec<_>>();
        assert_eq!(synced, expected);
        assert!(synced.contains(&root));
        assert!(path.exists());
        let mut image = store.load().unwrap();
        image.revision += 1;
        image
            .checkpoints
            .insert("retained".into(), serde_json::json!({"unresolved":true}));
        store.compare_exchange(0, &image).unwrap();
        drop(store);
        let original = fs::read(&path).unwrap();
        assert!(
            NativeJournalStore::open_with_directory_sync(&path, |_| {
                Err(std::io::Error::other("injected reopen barrier failure"))
            })
            .is_err()
        );
        assert_eq!(fs::read(&path).unwrap(), original);
        let reopened = NativeJournalStore::open(&path).unwrap();
        assert_eq!(reopened.load().unwrap().checkpoints, image.checkpoints);
        drop(reopened);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn interrupted_migration_preserves_original_image_and_extensions() {
        let parent = PathBuf::from(std::env::var_os("RTAO_TEST_TMPDIR").unwrap())
            .join(format!("migration-{}", std::process::id()));
        fs::create_dir_all(&parent).unwrap();
        let path = parent.join("journal.json");
        let store = NativeJournalStore::open(&path).unwrap();
        let mut old = JournalImage {
            schema_version: 1,
            ..JournalImage::default()
        };
        old.extensions
            .insert("unknown-evidence".into(), serde_json::json!({"value":7}));
        old.checkpoints
            .insert("unresolved".into(), serde_json::json!({"hash":"retained"}));
        old.operations.insert(
            "legacy-op".into(),
            super::super::OperationRecord {
                intent: super::super::OperationIntent {
                    id: "legacy-op".into(),
                    correlation_id: Some("revision".into()),
                    genesis: rtao_primitives::H256([0; 32]),
                    account: rtao_primitives::AccountId([1; 32]),
                    scope: "test".into(),
                    effects: serde_json::json!({"effect":"retain"}),
                    obligation: false,
                    semantic_policy: None,
                },
                material: serde_json::json!({"commit":"retain"}),
                material_generation: 0,
                incompatibility: None,
                extensions: Default::default(),
                attempts: vec![super::super::SignedAttempt {
                    hash: rtao_primitives::H256([3; 32]),
                    bytes: vec![1, 2, 3],
                    nonce: 7,
                    spec_version: 481,
                    transaction_version: 1,
                    start: 10,
                    expiry: Some(138),
                    era_anchor: rtao_primitives::H256([4; 32]),
                    era: serde_json::Value::Null,
                    metadata_hash: None,
                    material: serde_json::json!({"round":12}),
                    next: 10,
                    finalized: None,
                    outcome: super::super::AttemptOutcome::Unresolved,
                }],
            },
        );
        store.write(&old, false).unwrap();
        // Real older images have no material-generation field.
        let mut legacy = serde_json::to_value(&old).unwrap();
        legacy["operations"]["legacy-op"]
            .as_object_mut()
            .unwrap()
            .remove("material_generation");
        fs::write(&path, serde_json::to_vec(&legacy).unwrap()).unwrap();
        let original = fs::read(&path).unwrap();
        let mut candidate = store.read().unwrap();
        candidate.schema_version = JOURNAL_SCHEMA;
        candidate.revision += 1;
        assert!(store.write(&candidate, true).is_err());
        assert_eq!(fs::read(&path).unwrap(), original);
        drop(store);
        let migrated = NativeJournalStore::open(&path).unwrap();
        let after = migrated.load().unwrap();
        assert_eq!(after.schema_version, JOURNAL_SCHEMA);
        assert_eq!(after.extensions, old.extensions);
        assert_eq!(after.checkpoints, old.checkpoints);
        assert_eq!(
            serde_json::to_value(&after.operations).unwrap(),
            serde_json::to_value(&old.operations).unwrap()
        );
        assert_eq!(after.revision, old.revision + 1);
        drop(migrated);
        let reopened = NativeJournalStore::open(&path).unwrap();
        assert_eq!(reopened.load().unwrap().revision, after.revision);
        drop(reopened);
        fs::remove_dir_all(parent).unwrap();
    }
}
