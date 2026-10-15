#![allow(dead_code, clippy::unwrap_used, clippy::expect_used)]
use std::{
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};
static NEXT: AtomicU64 = AtomicU64::new(0);
pub struct TempRoot(PathBuf);
impl TempRoot {
    pub fn new() -> Self {
        let root = PathBuf::from(
            std::env::var_os("RTAO_TEMP_ROOT")
                .expect("wallet tests require the recorded task root"),
        );
        assert!(root.is_absolute());
        let parent = root.join("tmp/m2-wallet-tests");
        std::fs::create_dir_all(&parent).unwrap();
        let path = parent.join(format!(
            "run-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir(&path).unwrap();
        Self(path)
    }
    pub fn path(&self) -> &Path {
        &self.0
    }
}
impl Drop for TempRoot {
    fn drop(&mut self) {
        // Tests have no child daemons or configuration/venv references. All async
        // work is joined before this guard drops; remove only its created path.
        assert!(!self.0.is_symlink());
        std::fs::remove_dir_all(&self.0).unwrap();
    }
}
