use monarch::*;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};

fn layout() -> Layout {
    Layout {
        outputs: (1..=2)
            .map(|id| OutputConfig {
                display_id: DisplayId {
                    adapter_luid: 100,
                    target_id: id,
                    edid_hash: Some(id as u64),
                    identity: Default::default(),
                },
                enabled: true,
                primary: id == 1,
                rotation: Some(Rotation::Landscape),
                position: Position {
                    x: (id as i32 - 1) * 1920,
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

#[derive(Clone)]
struct FallibleStore {
    memory: MemoryConfigStore,
    fail: Arc<AtomicBool>,
}
impl ConfigStore for FallibleStore {
    fn load(&self) -> Result<AppConfig, ManagerError> {
        self.memory.load()
    }
    fn save(&self, config: &AppConfig) -> Result<(), ManagerError> {
        if self.fail.load(Ordering::Relaxed) {
            Err(ManagerError::Backend("injected disk write failure".into()))
        } else {
            self.memory.save(config)
        }
    }
}
fn store() -> FallibleStore {
    FallibleStore {
        memory: MemoryConfigStore::default(),
        fail: Arc::new(AtomicBool::new(false)),
    }
}

#[test]
fn failed_profile_save_preserves_live_profiles() {
    let store = store();
    let backend = MockBackend::new(vec![], layout()).unwrap();
    let mut manager = MonarchDisplayManager::new(backend, store.clone()).unwrap();
    store.fail.store(true, Ordering::Relaxed);
    assert!(manager.save_profile("reported as failed").is_err());
    assert!(manager.list_profiles().is_empty());
    assert!(
        store.memory.snapshot().unwrap().profiles.is_empty(),
        "disk differs from memory"
    );
}

#[test]
fn failed_settings_save_preserves_live_settings() {
    let store = store();
    let backend = MockBackend::new(vec![], layout()).unwrap();
    let mut manager = MonarchDisplayManager::new(backend, store.clone()).unwrap();
    let mut settings = manager.settings().clone();
    settings.revert_timeout_secs = 31;
    store.fail.store(true, Ordering::Relaxed);
    assert!(manager.update_settings(settings).is_err());
    assert_eq!(manager.settings().revert_timeout_secs, 10);
    assert_eq!(
        store
            .memory
            .snapshot()
            .unwrap()
            .settings
            .revert_timeout_secs,
        10
    );
}

struct PartialFailureBackend(MockBackend);
impl DisplayBackend for PartialFailureBackend {
    fn list_displays(&self) -> Result<Vec<DisplayInfo>, ManagerError> {
        self.0.list_displays()
    }
    fn get_layout(&self) -> Result<Layout, ManagerError> {
        self.0.get_layout()
    }
    fn apply_layout(&self, layout: Layout) -> Result<(), ManagerError> {
        self.0.apply_layout(layout)?;
        Err(ManagerError::Backend(
            "apply verification failed; restoring previous layout also failed".into(),
        ))
    }
}

#[test]
fn failed_apply_after_mutation_preserves_pending_recovery() {
    let backend = MockBackend::new(vec![], layout()).unwrap();
    let observer = backend.clone();
    let mut manager =
        MonarchDisplayManager::new(PartialFailureBackend(backend), MemoryConfigStore::default())
            .unwrap();
    let mut desired = layout();
    desired.outputs[1].enabled = false;
    assert!(manager.apply_layout(desired).is_err());
    assert_eq!(observer.current_layout().unwrap().enabled_output_count(), 1);
    assert!(manager.has_pending_confirmation());
    assert_eq!(
        manager.pending_confirmation_remaining(),
        Some(std::time::Duration::ZERO)
    );
    assert!(manager.config().pending_recovery.is_some());
    assert!(manager.rollback_if_confirmation_expired().is_err());
    assert!(manager.rollback_pending().is_err());
}

#[test]
fn duplicate_enabled_connectors_are_rejected() {
    let mut desired = layout();
    desired.outputs.push(desired.outputs[0].clone());
    assert!(desired.ensure_valid().is_err());
}

#[test]
fn restarting_manager_recovers_unconfirmed_change() {
    let backend = MockBackend::new(vec![], layout()).unwrap();
    let store = MemoryConfigStore::default();
    let mut manager = MonarchDisplayManager::new(backend.clone(), store.clone()).unwrap();
    let mut desired = layout();
    desired.outputs[1].enabled = false;
    manager.apply_layout(desired).unwrap();
    assert!(manager.has_pending_confirmation());
    drop(manager);
    let mut restarted = MonarchDisplayManager::new(backend.clone(), store).unwrap();
    assert_eq!(backend.current_layout().unwrap().enabled_output_count(), 1);
    assert!(restarted.has_pending_confirmation());
    assert!(restarted.rollback_if_confirmation_expired().unwrap());
    assert_eq!(backend.current_layout().unwrap().enabled_output_count(), 2);
    assert!(restarted.config().pending_recovery.is_none());
    assert_eq!(
        restarted
            .config()
            .last_known_good_layout
            .as_ref()
            .unwrap()
            .enabled_output_count(),
        2
    );
}

#[test]
fn legacy_custom_shortcut_mode_is_preserved() {
    let mut config = AppConfig::default();
    config.settings.profile_shortcut_base = None;
    config
        .settings
        .profile_shortcuts
        .insert("work".into(), "Ctrl+Alt+W".into());
    let backend = MockBackend::new(vec![], layout()).unwrap();
    let manager = MonarchDisplayManager::new(backend, MemoryConfigStore::new(config)).unwrap();
    assert_eq!(manager.settings().profile_shortcut_base.as_deref(), None);
    assert!(manager.settings().profile_shortcuts.contains_key("work"));
}

#[test]
fn journal_write_failure_prevents_any_display_mutation() {
    let backend = MockBackend::new(vec![], layout()).unwrap();
    let store = store();
    let mut manager = MonarchDisplayManager::new(backend.clone(), store.clone()).unwrap();
    let original = backend.current_layout().unwrap();
    store.fail.store(true, Ordering::Relaxed);
    let mut desired = original.clone();
    desired.outputs[1].enabled = false;
    assert!(manager.apply_layout(desired).is_err());
    assert_eq!(backend.current_layout().unwrap(), original);
    assert!(!manager.has_pending_confirmation());
}

#[test]
fn malformed_or_cloned_layout_is_rejected_before_journaling() {
    let backend = MockBackend::new(vec![], layout()).unwrap();
    let store = MemoryConfigStore::default();
    let mut manager = MonarchDisplayManager::new(backend.clone(), store.clone()).unwrap();
    let original_config = store.snapshot().unwrap();
    for invalid_property in 0..5 {
        let mut desired = layout();
        match invalid_property {
            0 => desired.outputs[1].primary = true,
            1 => desired.outputs[1].position = Position { x: 0, y: 0 },
            2 => desired.outputs[1].resolution.width = 0,
            3 => desired.outputs[1].position.x = i32::MIN,
            _ => desired.outputs[1].refresh_rate_mhz = u32::MAX,
        }
        assert!(manager.apply_layout(desired).is_err());
        assert!(!manager.has_pending_confirmation());
        assert_eq!(store.snapshot().unwrap(), original_config);
        assert_eq!(backend.current_layout().unwrap(), layout());
    }
}

#[test]
fn rotation_round_trips_and_is_verified() {
    let mut portrait = layout();
    portrait.outputs[1].rotation = Some(Rotation::Portrait);
    portrait.outputs[1].resolution = Resolution {
        width: 1080,
        height: 1920,
    };
    let serialized = serde_json::to_string(&portrait).unwrap();
    let restored: Layout = serde_json::from_str(&serialized).unwrap();
    assert_eq!(restored, portrait);
    let mut wrong = portrait.clone();
    wrong.outputs[1].rotation = Some(Rotation::Landscape);
    assert!(monarch::verification::verify_applied_layout(&portrait, &wrong).is_err());
}

#[test]
fn legacy_profile_without_rotation_preserves_current_orientation() {
    let mut current = layout();
    current.outputs[1].rotation = Some(Rotation::Portrait);
    let mut legacy = current.clone();
    legacy.outputs[0].rotation = None;
    legacy.outputs[1].rotation = None;
    let config = AppConfig {
        profiles: vec![Profile {
            name: "legacy".into(),
            layout: legacy,
        }],
        ..Default::default()
    };
    let backend = MockBackend::new(vec![], current.clone()).unwrap();
    let mut manager =
        MonarchDisplayManager::new(backend.clone(), MemoryConfigStore::new(config)).unwrap();
    manager.apply_profile("legacy").unwrap();
    assert_eq!(backend.current_layout().unwrap(), current);
}

#[test]
fn failed_delete_does_not_remove_profile_from_memory() {
    let store = store();
    let backend = MockBackend::new(vec![], layout()).unwrap();
    let mut manager = MonarchDisplayManager::new(backend, store.clone()).unwrap();
    manager.save_profile("work").unwrap();
    store.fail.store(true, Ordering::Relaxed);
    assert!(manager.delete_profile("work").is_err());
    assert_eq!(manager.list_profiles().len(), 1);
}

#[test]
fn legacy_configuration_is_migrated_and_custom_shortcuts_survive_settings_save() {
    let legacy: AppConfig = serde_json::from_value(serde_json::json!({
        "settings": { "profile_shortcut_base": null, "profile_shortcuts": { "work": "Ctrl+Alt+W" } }
    }))
    .unwrap();
    let store = MemoryConfigStore::new(legacy);
    let backend = MockBackend::new(vec![], layout()).unwrap();
    let mut manager = MonarchDisplayManager::new(backend, store.clone()).unwrap();
    assert_eq!(
        store.snapshot().unwrap().schema_version,
        monarch::model::CONFIG_SCHEMA_VERSION
    );
    let mut settings = manager.settings().clone();
    settings.revert_timeout_secs = 17;
    manager.update_settings(settings).unwrap();
    assert_eq!(manager.settings().profile_shortcut_base, None);
    assert_eq!(manager.settings().profile_shortcuts["work"], "Ctrl+Alt+W");
}

#[test]
fn startup_migration_preserves_serial_when_current_query_temporarily_loses_it() {
    let mut saved = layout();
    saved.outputs[0].display_id.identity.edid_serial = Some("serial-123".into());
    let mut current = saved.clone();
    current.outputs[0].display_id.identity.edid_serial = None;
    let config = AppConfig {
        profiles: vec![Profile {
            name: "work".into(),
            layout: saved,
        }],
        ..Default::default()
    };
    let store = MemoryConfigStore::new(config);
    let backend = MockBackend::new(vec![], current).unwrap();
    let manager = MonarchDisplayManager::new(backend, store).unwrap();
    assert_eq!(
        manager.list_profiles()[0].layout.outputs[0]
            .display_id
            .identity
            .edid_serial
            .as_deref(),
        Some("serial-123")
    );
}
