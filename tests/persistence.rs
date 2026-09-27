use monarch::*;
use std::{
    fs,
    path::PathBuf,
    sync::atomic::{AtomicU64, Ordering},
};

struct TempDir(PathBuf);
impl TempDir {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let path = std::env::temp_dir().join(format!(
            "monarch-store-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&path).unwrap();
        Self(path)
    }
    fn store(&self) -> FileConfigStore {
        FileConfigStore::new(self.0.join("config.json"))
    }
}
impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[test]
fn corrupt_primary_recovers_backup_and_does_not_overwrite_it_with_corrupt_bytes() {
    let dir = TempDir::new();
    let store = dir.store();
    let initial = AppConfig::default();
    store.save(&initial).unwrap();
    let mut changed = initial.clone();
    changed.settings.revert_timeout_secs = 27;
    store.save(&changed).unwrap();
    fs::write(store.path(), b"{truncated").unwrap();
    assert_eq!(store.load().unwrap(), initial);
    store.save(&changed).unwrap();
    assert_eq!(store.load().unwrap(), changed);
    fs::write(store.path(), b"bad").unwrap();
    assert_eq!(store.load().unwrap(), initial);
}

#[test]
fn failed_backup_write_preserves_primary_and_cleans_temporary_files() {
    let dir = TempDir::new();
    let store = dir.store();
    let initial = AppConfig::default();
    store.save(&initial).unwrap();
    fs::create_dir(store.path().with_extension("json.bak")).unwrap();
    let mut changed = initial.clone();
    changed.settings.revert_timeout_secs = 27;
    assert!(store.save(&changed).is_err());
    assert_eq!(store.load().unwrap(), initial);
    assert_eq!(fs::read_dir(&dir.0).unwrap().count(), 2);
}

#[test]
fn existing_corrupt_config_is_not_silently_reset_to_defaults() {
    let dir = TempDir::new();
    let store = dir.store();
    assert_eq!(store.load().unwrap(), AppConfig::default());
    fs::write(store.path(), b"bad").unwrap();
    assert!(store.load().is_err());
}

#[test]
fn future_schema_is_rejected_without_overwriting_the_file() {
    let dir = TempDir::new();
    let store = dir.store();
    let future = AppConfig {
        schema_version: u32::MAX,
        ..Default::default()
    };
    store.save(&future).unwrap();
    // Schema rejection precedes enumeration, even with no valid mock layout.
    let backend = EmptyBackend;
    assert!(MonarchDisplayManager::new(backend, store.clone()).is_err());
    assert_eq!(store.load().unwrap(), future);
}

struct EmptyBackend;
impl DisplayBackend for EmptyBackend {
    fn list_displays(&self) -> Result<Vec<DisplayInfo>, ManagerError> {
        panic!("schema should be rejected before enumeration")
    }
    fn get_layout(&self) -> Result<Layout, ManagerError> {
        panic!("schema should be rejected before enumeration")
    }
    fn apply_layout(&self, _: Layout) -> Result<(), ManagerError> {
        unreachable!()
    }
}
