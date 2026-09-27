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
fn valid_config_round_trips_and_retains_previous_valid_backup() {
    let dir = TempDir::new();
    let store = dir.store();
    let initial = AppConfig::default();
    store.save(&initial).unwrap();
    let mut changed = initial.clone();
    changed.settings.revert_timeout_secs = 27;
    store.save(&changed).unwrap();
    assert_eq!(store.load().unwrap(), changed);
    let backup: AppConfig =
        serde_json::from_slice(&fs::read(store.path().with_extension("json.bak")).unwrap())
            .unwrap();
    assert_eq!(backup, initial);
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
fn corrupt_config_and_backup_are_deleted_before_starting_fresh() {
    let dir = TempDir::new();
    let store = dir.store();
    assert_eq!(store.load().unwrap(), AppConfig::default());
    for backup in [
        b"bad".to_vec(),
        serde_json::to_vec(&saved_config()).unwrap(),
    ] {
        fs::write(store.path(), b"{truncated").unwrap();
        fs::write(store.path().with_extension("json.bak"), backup).unwrap();
        assert_reset(&store);
    }
}

#[test]
fn missing_primary_does_not_resurrect_an_orphaned_backup() {
    let dir = TempDir::new();
    let store = dir.store();
    fs::write(
        store.path().with_extension("json.bak"),
        serde_json::to_vec(&saved_config()).unwrap(),
    )
    .unwrap();
    assert_reset(&store);
}

#[test]
fn older_and_future_schemas_reset_instead_of_migrating_or_blocking_startup() {
    for version in [0, 1, u32::MAX] {
        let mut config = saved_config();
        config.schema_version = version;
        assert_startup_resets(serde_json::to_value(config).unwrap());
    }
}

#[test]
fn incomplete_current_format_is_reset_instead_of_filling_missing_fields() {
    for field in [
        "/schema_version",
        "/settings",
        "/settings/global_shortcuts_enabled",
        "/settings/startup_profile_name",
        "/pending_recovery",
        "/profiles/0/layout/outputs/0/rotation",
        "/profiles/0/layout/outputs/0/display_id/identity",
        "/profiles/0/layout/outputs/0/display_id/identity/edid_serial",
        "/profiles/0/layout/outputs/0/display_id/edid_hash",
    ] {
        let mut config = serde_json::to_value(saved_config()).unwrap();
        let (parent, key) = field.rsplit_once('/').unwrap();
        config
            .pointer_mut(parent)
            .unwrap()
            .as_object_mut()
            .unwrap()
            .remove(key);
        assert_startup_resets(config);
    }
}

#[test]
fn unknown_fields_and_unsupported_profiles_reset_the_entire_config() {
    for (field, value) in [
        ("/obsolete_setting", serde_json::json!(true)),
        ("/settings/obsolete_setting", serde_json::json!(true)),
        ("/profiles/0/name", serde_json::json!("  ")),
        (
            "/profiles/0/layout/outputs/0/rotation",
            serde_json::json!("diagonal"),
        ),
        (
            "/profiles/0/layout/outputs/0/old_mode",
            serde_json::json!(1),
        ),
        (
            "/profiles/0/layout/outputs/0/resolution/width",
            serde_json::json!(0),
        ),
        (
            "/profiles/0/layout/outputs/1/position/x",
            serde_json::json!(0),
        ),
        (
            "/profiles/0/layout/outputs/1/display_id/target_id",
            serde_json::json!(1),
        ),
        ("/pending_recovery/outputs", serde_json::json!([])),
        ("/last_restorable_layout/outputs", serde_json::json!([])),
        ("/settings/revert_timeout_secs", serde_json::json!(0)),
        (
            "/settings/display_toggle_shortcuts",
            serde_json::json!({"64:1": "Ctrl+Alt+W"}),
        ),
    ] {
        let mut config = serde_json::to_value(saved_config()).unwrap();
        let (parent, key) = field.rsplit_once('/').unwrap();
        config.pointer_mut(parent).unwrap()[key] = value;
        assert_startup_resets(config);
    }
    let mut config = saved_config();
    config.profiles.push(config.profiles[0].clone());
    assert_startup_resets(serde_json::to_value(config).unwrap());
}

#[test]
fn current_profiles_and_recovery_survive_disconnected_monitors_without_rewriting() {
    let dir = TempDir::new();
    let store = dir.store();
    let config = saved_config();
    store.save(&config).unwrap();
    let mut disconnected = layout();
    disconnected.outputs.truncate(1);
    disconnected.outputs[0].display_id.adapter_luid = 999;
    let backend = MockBackend::new(vec![], disconnected.clone()).unwrap();
    let manager = MonarchDisplayManager::new(backend.clone(), store.clone()).unwrap();
    assert_eq!(manager.config(), &config);
    assert_eq!(store.load().unwrap(), config);
    assert!(manager.has_pending_confirmation());
    assert_eq!(backend.current_layout().unwrap(), disconnected);
}

#[test]
fn unsupported_in_memory_config_also_starts_with_defaults() {
    let mut unsupported = saved_config();
    unsupported.profiles[0].layout.outputs.clear();
    let store = MemoryConfigStore::new(unsupported);
    let backend = MockBackend::new(vec![], layout()).unwrap();
    let manager = MonarchDisplayManager::new(backend, store.clone()).unwrap();
    assert!(manager.list_profiles().is_empty());
    assert_eq!(manager.settings(), &AppSettings::default());
    assert!(!manager.has_pending_confirmation());
    assert!(store.snapshot().unwrap().is_supported());
}

#[test]
fn cloned_live_desktop_does_not_prevent_startup_or_seed_unsupported_recovery() {
    let dir = TempDir::new();
    let store = dir.store();
    fs::write(store.path(), b"outdated").unwrap();
    let mut cloned = layout();
    cloned.outputs[1].position.x = 0;
    let backend = MockBackend::new(vec![], cloned.clone()).unwrap();
    let manager = MonarchDisplayManager::new(backend.clone(), store.clone()).unwrap();
    assert!(manager.config().is_supported());
    assert!(manager.config().last_known_good_layout.is_none());
    assert!(manager.config().last_restorable_layout.is_none());
    assert_eq!(backend.current_layout().unwrap(), cloned);
    assert_eq!(store.load().unwrap(), AppConfig::default());
}

#[test]
fn io_failure_is_reported_without_treating_the_path_as_obsolete_configuration() {
    let dir = TempDir::new();
    let store = dir.store();
    fs::create_dir(store.path()).unwrap();
    assert!(store.load().is_err());
    assert!(store.path().is_dir());
}

fn assert_reset(store: &FileConfigStore) {
    assert_eq!(store.load().unwrap(), AppConfig::default());
    assert!(!store.path().exists());
    assert!(!store.path().with_extension("json.bak").exists());
}

fn assert_startup_resets(invalid: serde_json::Value) {
    let dir = TempDir::new();
    let store = dir.store();
    // Even a usable backup must not bring back profiles/settings after a reset.
    store.save(&saved_config()).unwrap();
    store.save(&saved_config()).unwrap();
    fs::write(store.path(), serde_json::to_vec(&invalid).unwrap()).unwrap();
    let backend = MockBackend::new(vec![], layout()).unwrap();
    let manager = MonarchDisplayManager::new(backend.clone(), store.clone()).unwrap();
    assert!(manager.list_profiles().is_empty(), "not reset: {invalid}");
    assert_eq!(manager.settings(), &AppSettings::default());
    assert!(!manager.has_pending_confirmation());
    assert_eq!(backend.current_layout().unwrap(), layout());
    assert_eq!(store.load().unwrap(), *manager.config());
    assert!(manager.config().is_supported());
    assert!(!store.path().with_extension("json.bak").exists());
    // Loading the freshly written configuration on the next launch succeeds too.
    let restarted = MonarchDisplayManager::new(backend, store).unwrap();
    assert_eq!(restarted.config(), manager.config());
}

fn saved_config() -> AppConfig {
    let mut config = AppConfig {
        profiles: vec![Profile {
            name: "work".into(),
            layout: layout(),
        }],
        last_known_good_layout: Some(layout()),
        last_restorable_layout: Some(layout()),
        pending_recovery: Some(layout()),
        ..Default::default()
    };
    config.settings.revert_timeout_secs = 27;
    config
}

fn layout() -> Layout {
    Layout {
        outputs: (1..=2)
            .map(|target| OutputConfig {
                display_id: DisplayId {
                    adapter_luid: 100,
                    target_id: target,
                    edid_hash: Some(target as u64),
                    identity: Default::default(),
                },
                enabled: true,
                primary: target == 1,
                rotation: Some(Rotation::Landscape),
                hdr_enabled: None,
                scale_percent: None,
                clone_group: None,
                position: Position {
                    x: (target as i32 - 1) * 1920,
                    y: 0,
                },
                resolution: Resolution {
                    width: 1920,
                    height: 1080,
                },
                refresh_rate_mhz: 60_000,
            })
            .collect(),
    }
}

#[test]
fn valid_v2_migration_preserves_every_record_and_original_bytes() {
    let dir = TempDir::new();
    let store = dir.store();
    let expected = saved_config();
    let mut v2 = serde_json::to_value(&expected).unwrap();
    v2["schema_version"] = serde_json::json!(2);
    fn strip(value: &mut serde_json::Value) {
        match value {
            serde_json::Value::Object(map) => {
                for key in [
                    "hdr_enabled",
                    "scale_percent",
                    "clone_group",
                    "cursor_correction_enabled",
                    "cursor_calibrations",
                ] {
                    map.remove(key);
                }
                for child in map.values_mut() {
                    strip(child);
                }
            }
            serde_json::Value::Array(values) => {
                for child in values {
                    strip(child);
                }
            }
            _ => {}
        }
    }
    strip(&mut v2);
    let bytes = serde_json::to_vec_pretty(&v2).unwrap();
    fs::write(store.path(), &bytes).unwrap();
    assert_eq!(store.load().unwrap(), expected);
    assert_eq!(
        fs::read(store.path().with_extension("json.v2.bak")).unwrap(),
        bytes
    );
    assert_eq!(store.load().unwrap(), expected);
}

#[test]
fn failed_migration_backup_does_not_replace_v2_config() {
    let dir = TempDir::new();
    let store = dir.store();
    let mut old = saved_config();
    old.schema_version = 2;
    let bytes = serde_json::to_vec(&old).unwrap();
    fs::write(store.path(), &bytes).unwrap();
    fs::create_dir(store.path().with_extension("json.v2.bak")).unwrap();
    assert!(store.load().is_err());
    assert_eq!(fs::read(store.path()).unwrap(), bytes);
}
