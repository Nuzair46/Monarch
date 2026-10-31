use monarch::*;
use std::sync::{Arc, Mutex};

#[derive(Clone, Copy)]
enum Failure {
    Preflight,
    Wake,
    Generic,
    Recovery,
    Repeated,
}

#[derive(Clone)]
struct ChangingBackend {
    inner: MockBackend,
    failure: Failure,
    calls: Arc<Mutex<Vec<Layout>>>,
    validations: Arc<Mutex<usize>>,
}

impl DisplayBackend for ChangingBackend {
    fn audio_snapshot(&self) -> Result<AudioSnapshot, ManagerError> {
        Ok(AudioSnapshot {
            defaults: AudioDefaults::default(),
            devices: Vec::new(),
            unavailable_reason: None,
        })
    }

    fn set_audio_defaults(&self, defaults: &AudioDefaults) -> Result<(), ManagerError> {
        if defaults == &AudioDefaults::default() {
            Ok(())
        } else {
            Err(ManagerError::Backend(
                "requested audio is unavailable".into(),
            ))
        }
    }

    fn list_displays(&self) -> Result<Vec<DisplayInfo>, ManagerError> {
        self.inner.list_displays()
    }

    fn get_layout(&self) -> Result<Layout, ManagerError> {
        self.inner.get_layout()
    }

    fn validate_layout(&self, desired: &Layout) -> Result<(), ManagerError> {
        let mut count = self.validations.lock().unwrap();
        *count += 1;
        if matches!(self.failure, Failure::Preflight) && *count == 1 {
            let mut current = self.inner.get_layout()?;
            current.outputs.retain(|o| o.display_id.target_id != 2);
            self.inner.apply_layout(current.clone())?;
            identity::resolve_layout(desired, &current)?;
        }
        Ok(())
    }

    fn apply_layout(&self, desired: Layout) -> Result<(), ManagerError> {
        let mut calls = self.calls.lock().unwrap();
        calls.push(desired.clone());
        let attempt = calls.len();
        drop(calls);
        if matches!(self.failure, Failure::Preflight)
            || (attempt > 1 && !matches!(self.failure, Failure::Repeated))
        {
            return self.inner.apply_layout(desired);
        }
        let previous = self.inner.get_layout()?;
        let mut partial = desired.clone();
        let missing = if attempt == 1 { 2 } else { 3 };
        partial
            .outputs
            .retain(|o| o.display_id.target_id != missing);
        transaction::apply_with_recovery(
            || {
                if partial.enabled_output_count() > 0 {
                    self.inner.apply_layout(partial.clone())?;
                }
                if matches!(self.failure, Failure::Generic) {
                    Err(ManagerError::Backend("SetDisplayConfig failed: 31".into()))
                } else {
                    identity::resolve_layout(&desired, &partial).map(|_| ())
                }
            },
            || {
                if matches!(self.failure, Failure::Recovery) {
                    Err(ManagerError::Backend("recovery failed".into()))
                } else {
                    self.inner.apply_layout(previous)
                }
            },
        )
    }
}

fn layout() -> Layout {
    Layout {
        outputs: (1..=3)
            .map(|id| OutputConfig {
                display_id: DisplayId {
                    adapter_luid: 1,
                    target_id: id,
                    edid_hash: Some(id as u64),
                    identity: Default::default(),
                },
                enabled: true,
                primary: id == 1,
                position: Position {
                    x: (id as i32 - 1) * 1920,
                    y: 0,
                },
                resolution: Resolution {
                    width: 1920,
                    height: 1080,
                },
                refresh_rate_mhz: 60_000,
                rotation: Some(Rotation::Landscape),
                hdr_enabled: None,
                scale_percent: None,
                clone_group: None,
            })
            .collect(),
    }
}

fn manager(
    failure: Failure,
    ignore: bool,
    saved: Layout,
) -> (
    MonarchDisplayManager<ChangingBackend, MemoryConfigStore>,
    ChangingBackend,
    MemoryConfigStore,
) {
    let mut current = layout();
    for output in &mut current.outputs[1..] {
        output.enabled = false;
    }
    let backend = ChangingBackend {
        inner: MockBackend::new(Vec::new(), current).unwrap(),
        failure,
        calls: Default::default(),
        validations: Default::default(),
    };
    let store = MemoryConfigStore::new(AppConfig {
        profiles: vec![Profile {
            name: "Desk".into(),
            layout: saved,
            audio_output: None,
            display_options: ProfileDisplayOptions {
                ignore_missing_displays: ignore,
            },
        }],
        ..Default::default()
    });
    (
        MonarchDisplayManager::new(backend.clone(), store.clone()).unwrap(),
        backend,
        store,
    )
}

#[test]
fn skips_a_display_that_loses_identity_during_preflight() {
    let (mut manager, backend, _) = manager(Failure::Preflight, true, layout());
    manager.apply_profile("Desk").unwrap();
    let calls = backend.calls.lock().unwrap();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].outputs.len(), 2);
    assert_eq!(calls[0].outputs[1].display_id.target_id, 3);
    assert_eq!(calls[0].outputs[1].position.x, 1920);
}

#[test]
fn skips_unwoken_display_after_verified_recovery_even_if_connected_inventory_retains_it() {
    let saved = layout();
    let (mut manager, backend, store) = manager(Failure::Wake, true, saved.clone());
    let previous = backend.get_layout().unwrap();
    manager.apply_profile("Desk").unwrap();
    let calls = backend.calls.lock().unwrap();
    assert_eq!(calls.len(), 2);
    assert_eq!(calls[1].outputs.len(), 2);
    assert_eq!(calls[1].outputs[1].display_id.target_id, 3);
    assert_eq!(calls[1].outputs[1].position.x, 1920);
    assert_eq!(manager.list_profiles()[0].layout, saved);
    assert_eq!(
        store.load().unwrap().pending_recovery,
        Some(previous.clone())
    );
    assert!(manager.has_pending_confirmation());
    drop(calls);
    manager.rollback_pending().unwrap();
    assert_eq!(backend.get_layout().unwrap(), previous);
}

#[test]
fn strict_profiles_still_fail_and_clear_the_recovered_journal() {
    let (mut manager, backend, store) = manager(Failure::Wake, false, layout());
    let previous = backend.get_layout().unwrap();
    assert!(matches!(
        manager.apply_profile("Desk"),
        Err(ManagerError::DisplaysUnavailableRestored(_))
    ));
    assert_eq!(backend.calls.lock().unwrap().len(), 1);
    assert_eq!(backend.get_layout().unwrap(), previous);
    assert!(!manager.has_pending_confirmation());
    assert!(store.load().unwrap().pending_recovery.is_none());
}

#[test]
fn generic_driver_errors_do_not_drop_connected_displays() {
    let (mut manager, backend, _) = manager(Failure::Generic, true, layout());
    assert!(matches!(
        manager.apply_profile("Desk"),
        Err(ManagerError::ApplyRestored(_))
    ));
    assert_eq!(backend.calls.lock().unwrap().len(), 1);
}

#[test]
fn failed_recovery_never_retries_and_keeps_the_journal() {
    let (mut manager, backend, store) = manager(Failure::Recovery, true, layout());
    assert!(matches!(
        manager.apply_profile("Desk"),
        Err(ManagerError::RecoveryRequired(_))
    ));
    assert_eq!(backend.calls.lock().unwrap().len(), 1);
    assert!(manager.has_pending_confirmation());
    assert!(store.load().unwrap().pending_recovery.is_some());
}

#[test]
fn retry_is_bounded_when_a_second_display_disappears() {
    let (mut manager, backend, store) = manager(Failure::Repeated, true, layout());
    let previous = backend.get_layout().unwrap();
    assert!(matches!(
        manager.apply_profile("Desk"),
        Err(ManagerError::DisplaysUnavailableRestored(_))
    ));
    assert_eq!(backend.calls.lock().unwrap().len(), 2);
    assert_eq!(backend.get_layout().unwrap(), previous);
    assert!(!manager.has_pending_confirmation());
    assert!(store.load().unwrap().pending_recovery.is_none());
}

#[test]
fn refuses_to_apply_when_the_only_enabled_profile_display_cannot_wake() {
    let mut saved = layout();
    saved.outputs.retain(|o| o.display_id.target_id == 2);
    saved.outputs[0].position.x = 0;
    saved.outputs[0].primary = true;
    let (mut manager, backend, store) = manager(Failure::Wake, true, saved);
    assert!(manager
        .apply_profile("Desk")
        .unwrap_err()
        .to_string()
        .contains("no enabled profile displays"));
    assert_eq!(backend.calls.lock().unwrap().len(), 1);
    assert!(store.load().unwrap().pending_recovery.is_none());
}

#[test]
fn skipping_a_sleeping_display_does_not_silently_skip_its_requested_audio() {
    let (_, backend, store) = manager(Failure::Wake, true, layout());
    let previous = backend.get_layout().unwrap();
    let mut config = store.load().unwrap();
    config.profiles[0].audio_output = Some(AudioOutput {
        id: "sleeping-hdmi".into(),
        name: "TV".into(),
    });
    store.save(&config).unwrap();
    let mut manager = MonarchDisplayManager::new(backend.clone(), store.clone()).unwrap();
    let error = manager.apply_profile("Desk").unwrap_err();
    assert!(error.to_string().contains("requested audio is unavailable"));
    assert_eq!(backend.calls.lock().unwrap().len(), 3); // first apply, retry, audio rollback
    assert_eq!(backend.get_layout().unwrap(), previous);
    assert!(!manager.has_pending_confirmation());
    assert!(store.load().unwrap().pending_recovery.is_none());
}

#[derive(Clone)]
struct JournalFailure {
    store: MemoryConfigStore,
    writes: Arc<Mutex<usize>>,
}

impl ConfigStore for JournalFailure {
    fn load(&self) -> Result<AppConfig, ManagerError> {
        self.store.load()
    }
    fn save(&self, config: &AppConfig) -> Result<(), ManagerError> {
        let mut writes = self.writes.lock().unwrap();
        *writes += 1;
        if *writes == 2 {
            return Err(ManagerError::Io(std::io::Error::other(
                "journal clear failed",
            )));
        }
        self.store.save(config)
    }
}

#[test]
fn journal_cleanup_failure_blocks_retry_after_native_recovery() {
    let (_, backend, memory) = manager(Failure::Wake, true, layout());
    let previous = backend.get_layout().unwrap();
    let store = JournalFailure {
        store: memory.clone(),
        writes: Default::default(),
    };
    let mut manager = MonarchDisplayManager::new(backend.clone(), store).unwrap();
    assert!(manager
        .apply_profile("Desk")
        .unwrap_err()
        .to_string()
        .contains("journal clear failed"));
    assert_eq!(backend.calls.lock().unwrap().len(), 1);
    assert_eq!(backend.get_layout().unwrap(), previous);
    assert!(manager.has_pending_confirmation());
    assert!(memory.load().unwrap().pending_recovery.is_some());
}
