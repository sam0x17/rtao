//! Shared book/cache writers use only fixture-owned public files and metadata.

use rtao::client::config::{ConfigFile, Paths};
use rtao_cli::{
    books::{Books, MultisigBook},
    call::{FsRawCallCache, RawCallCache},
};
use serde_json::{Value, json};
use std::{
    error::Error,
    fs,
    path::PathBuf,
    sync::atomic::{AtomicU64, Ordering},
};

type TestResult = Result<(), Box<dyn Error>>;
const LIMIT: u64 = 16 * 1024 * 1024;

struct Fixture {
    root: PathBuf,
    paths: Paths,
}
impl Fixture {
    fn new() -> Result<Self, Box<dyn Error>> {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let base = std::env::var_os("RTAO_TEMP_ROOT")
            .map(PathBuf::from)
            .unwrap_or_else(|| {
                PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/m5-book-cache-tests")
            });
        let root = base.join("tmp").join(format!(
            "book-cache-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&root)?;
        let paths = Paths::new(&root, |name| match name {
            "BTCLI_MULTISIGS_PATH" => Some(root.join("shared/multisigs.json").into_os_string()),
            "BTCLI_MULTISIG_CACHE" => Some(root.join("shared/calls.json").into_os_string()),
            _ => None,
        });
        Ok(Self { root, paths })
    }
    fn book(&self) -> MultisigBook {
        MultisigBook::new(&self.paths)
    }
    fn cache(&self) -> FsRawCallCache {
        FsRawCallCache::new(&self.paths)
    }
    fn path(&self, kind: ConfigFile) -> &std::path::Path {
        self.paths.path(kind)
    }
    fn backups(&self, kind: ConfigFile) -> Result<Vec<PathBuf>, Box<dyn Error>> {
        let path = self.path(kind);
        let prefix = format!(
            "{}.corrupt.",
            path.file_name().ok_or("file name")?.to_string_lossy()
        );
        Ok(fs::read_dir(path.parent().ok_or("parent")?)?
            .filter_map(Result::ok)
            .filter(|entry| entry.file_name().to_string_lossy().starts_with(&prefix))
            .map(|entry| entry.path())
            .collect())
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        // All offloaded writers have returned and no services or processes use
        // these task-owned public fixtures. Remove only this exact directory.
        let _ = fs::remove_dir_all(&self.root);
    }
}
fn entry(name: &str) -> Value {
    json!({"name":name,"threshold":2,"signatories":["alice","bob"],"note":"portable refs","custom":{"future":true}})
}
fn cache_spec(data: &[u8]) -> Result<(String, Value), Box<dyn Error>> {
    let hash = hex::encode(rtao::core::codec::storage::hash_param("Blake2_256", data)?);
    Ok((
        hash,
        json!({"target":"System.remark","params":{},"call_data":format!("0x{}",hex::encode(data))}),
    ))
}

#[tokio::test]
async fn multisig_mutations_preserve_order_extra_fields_and_require_exact_overwrite() -> TestResult
{
    let fixture = Fixture::new()?;
    let book = fixture.book();
    let first = entry("first");
    book.add(first.clone(), false).await?;
    book.add(entry("second"), false).await?;
    let before = fs::read(fixture.path(ConfigFile::Multisigs))?;
    assert!(book.add(entry("first"), false).await.is_err());
    assert_eq!(fs::read(fixture.path(ConfigFile::Multisigs))?, before);
    let mut replacement = entry("first");
    replacement["note"] = json!("replacement");
    book.add(replacement.clone(), true).await?;
    assert_eq!(
        book.load().await?,
        vec![entry("second"), replacement.clone()]
    );
    assert!(!book.remove("FIRST").await?);
    assert!(book.remove("second").await?);
    let loaded = Books::load(&fixture.paths).await?;
    assert_eq!(loaded.multisigs, vec![replacement]);
    assert!(fixture.backups(ConfigFile::Multisigs)?.is_empty());
    assert_eq!(
        fs::read_dir(
            fixture
                .path(ConfigFile::Multisigs)
                .parent()
                .ok_or("parent")?
        )?
        .count(),
        1
    );
    Ok(())
}

#[tokio::test]
async fn rejected_book_input_preserves_source_and_creates_no_replacement_artifacts() -> TestResult {
    let fixture = Fixture::new()?;
    let book = fixture.book();
    for invalid in [
        json!({"name":"x","threshold":0,"signatories":["a"]}),
        json!({"name":"x","threshold":2,"signatories":[17]}),
        json!({"name":"","threshold":1,"signatories":["a"]}),
    ] {
        assert!(book.add(invalid, true).await.is_err());
    }
    assert!(!fixture.path(ConfigFile::Multisigs).exists());
    book.add(entry("keep"), false).await?;
    let path = fixture.path(ConfigFile::Multisigs);
    let before = fs::read(path)?;
    let mut oversized = entry("oversized");
    oversized["note"] = json!("x".repeat(LIMIT as usize));
    assert!(book.add(oversized, true).await.is_err());
    assert_eq!(fs::read(path)?, before);
    let mut nested = json!(null);
    for _ in 0..140 {
        nested = json!([nested]);
    }
    let mut deep = entry("deep");
    deep["custom"] = nested;
    assert!(book.add(deep, true).await.is_err());
    assert_eq!(fs::read(path)?, before);
    let file = fs::OpenOptions::new().write(true).open(path)?;
    file.set_len(LIMIT + 1)?;
    drop(file);
    assert!(book.add(entry("blocked"), true).await.is_err());
    assert_eq!(fs::metadata(path)?.len(), LIMIT + 1);
    assert!(fixture.backups(ConfigFile::Multisigs)?.is_empty());
    assert_eq!(fs::read_dir(path.parent().ok_or("parent")?)?.count(), 1);
    Ok(())
}

#[tokio::test]
async fn corrupt_inputs_survive_and_rejected_output_creates_no_backups() -> TestResult {
    let fixture = Fixture::new()?;
    let path = fixture.path(ConfigFile::Multisigs);
    fs::create_dir_all(path.parent().ok_or("parent")?)?;
    fs::write(path, b"{broken book")?;
    assert!(fixture.book().load().await?.is_empty());
    assert!(fixture.backups(ConfigFile::Multisigs)?.is_empty());
    fixture.book().save(vec![entry("restored")]).await?;
    let backups = fixture.backups(ConfigFile::Multisigs)?;
    assert_eq!(backups.len(), 1);
    assert_eq!(fs::read(&backups[0])?, b"{broken book");
    let cache = fixture.cache();
    let path = fixture.path(ConfigFile::MultisigCache);
    fs::write(path, b"[]")?;
    let (hash, spec) = cache_spec(&[1, 2])?;
    let mut oversized = spec.clone();
    oversized["padding"] = json!("x".repeat(LIMIT as usize));
    assert!(cache.save(hash.clone(), oversized).await.is_err());
    assert_eq!(fs::read(path)?, b"[]");
    assert!(fixture.backups(ConfigFile::MultisigCache)?.is_empty());
    cache.save(hash, spec).await?;
    let backups = fixture.backups(ConfigFile::MultisigCache)?;
    assert_eq!(backups.len(), 1);
    assert_eq!(fs::read(&backups[0])?, b"[]");
    Ok(())
}

#[tokio::test]
async fn cache_rejects_hash_or_data_substitution_before_modifying_any_file() -> TestResult {
    let fixture = Fixture::new()?;
    let cache = fixture.cache();
    let (hash, spec) = cache_spec(&[1, 2])?;
    cache.save(hash.to_uppercase(), spec.clone()).await?;
    let path = fixture.path(ConfigFile::MultisigCache);
    let before = fs::read(path)?;
    let data: Value = serde_json::from_slice(&before)?;
    assert_eq!(data[format!("0x{hash}")]["call_data"], "0x0102");
    let mut changed = spec.clone();
    changed["call_data"] = json!("0x0103");
    assert!(cache.save(hash.clone(), changed).await.is_err());
    let mut changed = spec;
    changed["call_hash"] = json!("00".repeat(32));
    assert!(cache.save(hash, changed).await.is_err());
    assert!(cache.save("wrong".into(), json!({})).await.is_err());
    assert_eq!(fs::read(path)?, before);
    assert_eq!(fs::read_dir(path.parent().ok_or("parent")?)?.count(), 1);
    Ok(())
}

#[cfg(unix)]
#[tokio::test]
async fn writers_keep_restrictive_modes_and_refuse_symlink_and_fifo_destinations() -> TestResult {
    use std::os::unix::fs::{PermissionsExt, symlink};
    let fixture = Fixture::new()?;
    let book = fixture.book();
    book.add(entry("first"), false).await?;
    let path = fixture.path(ConfigFile::Multisigs);
    assert_eq!(fs::metadata(path)?.permissions().mode() & 0o777, 0o600);
    assert_eq!(
        fs::metadata(path.parent().ok_or("parent")?)?
            .permissions()
            .mode()
            & 0o777,
        0o700
    );
    fs::set_permissions(path, fs::Permissions::from_mode(0o640))?;
    book.add(entry("second"), false).await?;
    assert_eq!(fs::metadata(path)?.permissions().mode() & 0o777, 0o640);
    let actual = fixture.root.join("unrelated.json");
    fs::rename(path, &actual)?;
    let before = fs::read(&actual)?;
    symlink(&actual, path)?;
    assert!(book.add(entry("refused"), true).await.is_err());
    assert_eq!(fs::read(&actual)?, before);
    fs::remove_file(path)?;
    assert!(
        std::process::Command::new("mkfifo")
            .arg(path)
            .status()?
            .success()
    );
    assert!(book.add(entry("refused"), true).await.is_err());
    let cache_path = fixture.path(ConfigFile::MultisigCache);
    assert!(
        std::process::Command::new("mkfifo")
            .arg(cache_path)
            .status()?
            .success()
    );
    let (hash, spec) = cache_spec(&[1, 2])?;
    assert!(fixture.cache().save(hash, spec).await.is_err());
    Ok(())
}

#[tokio::test]
async fn cache_lookup_decodes_exact_bytes_and_ignores_untrusted_presentation_fields() -> TestResult
{
    let fixture = Fixture::new()?;
    let runtime = rtao_runtime::RuntimeHandle::parse(
        include_bytes!("../../../vendor/metadata/finney/478.v15.scale"),
        478,
        1,
        42,
    )?;
    let expected = rtao_runtime::calls::system::remark(vec![1, 2]).compose(&runtime)?;
    let (hash, mut spec) = cache_spec(expected.as_bytes())?;
    spec["target"] = json!("Balances.transfer_all");
    spec["params"] = json!({"dest":"untrusted-presentation-field"});
    let cache = fixture.cache();
    cache.save(hash.clone(), spec).await?;
    assert_eq!(
        cache.load_call(&hash, &runtime).await?,
        Some(expected.clone())
    );
    assert!(cache.load_call(&"00".repeat(32), &runtime).await?.is_none());
    let path = fixture.path(ConfigFile::MultisigCache);
    let mut entries: Value = serde_json::from_slice(&fs::read(path)?)?;
    entries[format!("0x{hash}")]["call_data"] = json!("0x0102");
    fs::write(path, serde_json::to_vec(&entries)?)?;
    let before = fs::read(path)?;
    assert!(cache.load_call(&hash, &runtime).await.is_err());
    assert_eq!(fs::read(path)?, before);
    assert!(fixture.backups(ConfigFile::MultisigCache)?.is_empty());

    let mut trailing = expected.as_bytes().to_vec();
    trailing.push(0);
    let (trailing_hash, spec) = cache_spec(&trailing)?;
    cache.save(trailing_hash.clone(), spec).await?;
    assert!(cache.load_call(&trailing_hash, &runtime).await.is_err());
    Ok(())
}
