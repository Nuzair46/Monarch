use monarch::*;
use std::cell::RefCell;
use std::rc::Rc;
use std::time::Duration;

fn desk() -> Layout {
    Layout {
        outputs: (1..=2)
            .map(|target_id| OutputConfig {
                display_id: DisplayId {
                    adapter_luid: 1,
                    target_id,
                    edid_hash: None,
                    identity: Default::default(),
                },
                enabled: target_id == 1,
                primary: target_id == 1,
                position: Position { x: 0, y: 0 },
                resolution: Resolution {
                    width: 1920,
                    height: 1080,
                },
                refresh_rate_mhz: 60_000,
                rotation: None,
                hdr_enabled: None,
                scale_percent: None,
                clone_group: None,
            })
            .collect(),
    }
}
fn tv() -> Layout {
    let mut layout = desk();
    for output in &mut layout.outputs {
        output.enabled = !output.enabled;
        output.primary = output.enabled;
    }
    layout
}
fn original_audio() -> AudioDefaults {
    AudioDefaults {
        console: Some("speakers".into()),
        multimedia: Some("music".into()),
        communications: Some("headset".into()),
    }
}
fn output(id: &str) -> AudioOutput {
    AudioOutput {
        id: id.into(),
        name: "Identical friendly name".into(),
    }
}

#[derive(Clone)]
struct Store {
    memory: MemoryConfigStore,
    fail: Rc<RefCell<bool>>,
}
impl ConfigStore for Store {
    fn load(&self) -> Result<AppConfig, ManagerError> {
        self.memory.load()
    }
    fn save(&self, config: &AppConfig) -> Result<(), ManagerError> {
        if *self.fail.borrow() {
            return Err(ManagerError::Backend("disk failure".into()));
        }
        self.memory.save(config)
    }
}
#[derive(Clone, Copy, PartialEq)]
enum Failure {
    Display,
    DisplayRestored,
    Console,
    Multimedia,
    Verification,
}
struct State {
    layout: Layout,
    defaults: AudioDefaults,
    events: Vec<&'static str>,
    failure: Option<Failure>,
    fail_recovery: bool,
    unavailable: bool,
}
#[derive(Clone)]
struct Backend {
    state: Rc<RefCell<State>>,
    store: Store,
}
impl DisplayBackend for Backend {
    fn list_displays(&self) -> Result<Vec<DisplayInfo>, ManagerError> {
        Ok(vec![])
    }
    fn get_layout(&self) -> Result<Layout, ManagerError> {
        Ok(self.state.borrow().layout.clone())
    }
    fn apply_layout(&self, layout: Layout) -> Result<(), ManagerError> {
        assert!(
            self.store.load()?.pending_recovery.is_some(),
            "journal must precede display mutation"
        );
        let mut state = self.state.borrow_mut();
        state.events.push("display");
        let previous = state.layout.clone();
        state.layout = layout;
        // Simulate Windows changing defaults when the active HDMI display changes.
        state.defaults.console = Some("windows-auto".into());
        if state.failure == Some(Failure::Display) {
            state.failure = None;
            return Err(ManagerError::Backend(
                "display failed after mutation".into(),
            ));
        }
        if state.failure == Some(Failure::DisplayRestored) {
            state.failure = None;
            state.layout = previous;
            return Err(ManagerError::ApplyRestored(
                "display restored natively".into(),
            ));
        }
        Ok(())
    }
    fn audio_snapshot(&self) -> Result<AudioSnapshot, ManagerError> {
        let state = self.state.borrow();
        if state.unavailable {
            return Ok(AudioSnapshot::unavailable("audio service unavailable"));
        }
        Ok(AudioSnapshot {
            devices: ["speakers", "music", "headset", "tv"]
                .map(|id| AudioDevice {
                    output: output(id),
                    available: id != "tv" || state.layout.outputs[1].enabled,
                })
                .to_vec(),
            defaults: state.defaults.clone(),
            unavailable_reason: None,
        })
    }
    fn set_audio_defaults(&self, defaults: &AudioDefaults) -> Result<(), ManagerError> {
        assert!(
            self.store.load()?.pending_recovery_audio.is_some(),
            "journal must precede audio mutation"
        );
        let snapshot = self.audio_snapshot()?;
        let mut state = self.state.borrow_mut();
        state.events.push("audio");
        if state.fail_recovery && defaults.console.as_deref() == Some("speakers") {
            return Err(ManagerError::Backend(
                "previous endpoint disconnected".into(),
            ));
        }
        for id in [
            &defaults.console,
            &defaults.multimedia,
            &defaults.communications,
        ]
        .into_iter()
        .flatten()
        {
            if !snapshot
                .devices
                .iter()
                .any(|device| &device.output.id == id && device.available)
            {
                return Err(ManagerError::NotFound(format!(
                    "audio output '{id}' unavailable"
                )));
            }
        }
        if defaults.console.is_some() {
            state.defaults.console = defaults.console.clone();
        }
        if state.failure == Some(Failure::Console) {
            state.failure = None;
            return Err(ManagerError::Backend("second role rejected".into()));
        }
        if defaults.multimedia.is_some() {
            state.defaults.multimedia = defaults.multimedia.clone();
        }
        if state.failure == Some(Failure::Multimedia) {
            state.failure = None;
            return Err(ManagerError::Backend("setter failed after mutation".into()));
        }
        if defaults.communications.is_some() {
            state.defaults.communications = defaults.communications.clone();
        }
        if state.failure == Some(Failure::Verification) {
            state.failure = None;
            state.defaults.console = Some("music".into());
            return Err(ManagerError::Backend("verification mismatch".into()));
        }
        assert!(defaults.matches(&state.defaults));
        Ok(())
    }
}
type Manager = MonarchDisplayManager<Backend, Store>;
fn fixture() -> (Manager, Backend, Store) {
    let config = AppConfig {
        profiles: vec![Profile {
            name: "TV".into(),
            layout: tv(),
            audio_output: Some(output("tv")),
        }],
        ..Default::default()
    };
    let store = Store {
        memory: MemoryConfigStore::new(config),
        fail: Rc::new(RefCell::new(false)),
    };
    let backend = Backend {
        store: store.clone(),
        state: Rc::new(RefCell::new(State {
            layout: desk(),
            defaults: original_audio(),
            events: vec![],
            failure: None,
            fail_recovery: false,
            unavailable: false,
        })),
    };
    (
        Manager::new(backend.clone(), store.clone()).unwrap(),
        backend,
        store,
    )
}

#[test]
fn profile_audio_save_is_durable_without_capturing_or_mutating_displays() {
    let (mut manager, backend, store) = fixture();
    manager.set_profile_audio("TV", Some("music")).unwrap();
    let profile = &manager.list_profiles()[0];
    assert_eq!(profile.layout, tv());
    assert_eq!(profile.audio_output.as_ref().unwrap().id, "music");
    assert_eq!(store.load().unwrap().profiles[0], *profile);
    assert_eq!(backend.state.borrow().defaults, original_audio());
    assert!(backend.state.borrow().events.is_empty());
    assert!(manager
        .set_profile_audio("TV", Some("Identical friendly name"))
        .is_err());
    manager.set_profile_audio("TV", None).unwrap();
    assert!(manager.list_profiles()[0].audio_output.is_none());
}

#[test]
fn saving_current_layout_with_audio_never_switches_audio_and_overwrite_retains_preference() {
    let (mut manager, backend, _) = fixture();
    manager
        .save_profile_with_audio("Desk", Some(output("music")))
        .unwrap();
    manager.save_profile("Desk").unwrap();
    let profile = manager
        .list_profiles()
        .into_iter()
        .find(|p| p.name == "Desk")
        .unwrap();
    assert_eq!(profile.layout, desk());
    assert_eq!(profile.audio_output.unwrap().id, "music");
    assert!(backend.state.borrow().events.is_empty());
}

#[test]
fn hdmi_selection_follows_topology_and_confirmation_restores_each_original_role() {
    let (mut manager, backend, store) = fixture();
    assert!(!manager.audio_snapshot().unwrap().devices[3].available);
    manager.apply_profile("TV").unwrap();
    assert_eq!(backend.state.borrow().events, ["display", "audio"]);
    assert_eq!(
        backend.state.borrow().defaults,
        AudioDefaults {
            console: Some("tv".into()),
            multimedia: Some("tv".into()),
            communications: Some("headset".into())
        }
    );
    assert!(manager.pending_confirmation_remaining().unwrap() > Duration::from_secs(9));
    assert_eq!(
        store.load().unwrap().pending_recovery_audio,
        Some(original_audio())
    );
    assert!(manager.set_profile_audio("TV", None).is_err());
    manager.rollback_pending().unwrap();
    assert_eq!(backend.state.borrow().layout, desk());
    assert_eq!(backend.state.borrow().defaults, original_audio());
    assert!(store.load().unwrap().pending_recovery_audio.is_none());
}

#[test]
fn confirmed_profile_can_restore_previous_layout_and_audio_later() {
    let (mut manager, backend, _) = fixture();
    manager.apply_profile("TV").unwrap();
    manager.confirm_current_layout().unwrap();
    manager.restore_last_layout().unwrap();
    assert_eq!(backend.state.borrow().defaults, original_audio());
    assert_eq!(backend.state.borrow().layout, desk());
    assert!(!manager.has_pending_confirmation());
}

#[test]
fn audio_only_profile_does_not_reapply_the_topology() {
    let (mut manager, backend, _) = fixture();
    manager
        .save_profile_with_audio("Music", Some(output("music")))
        .unwrap();
    manager.apply_profile("Music").unwrap();
    assert_eq!(backend.state.borrow().events, ["audio"]);
    manager.confirm_current_layout().unwrap();
    manager
        .save_profile_with_audio("Headset", Some(output("headset")))
        .unwrap();
    manager.apply_profile("Headset").unwrap();
    manager.confirm_current_layout().unwrap();
    assert_eq!(backend.state.borrow().events, ["audio", "audio"]);
    assert_eq!(
        backend.state.borrow().defaults.console.as_deref(),
        Some("headset")
    );
}

#[test]
fn leave_unchanged_does_not_set_audio_but_timeout_restores_windows_side_effects() {
    let (mut manager, backend, _) = fixture();
    manager.set_profile_audio("TV", None).unwrap();
    manager.set_confirmation_timeout(Duration::ZERO);
    manager.apply_profile("TV").unwrap();
    assert_eq!(backend.state.borrow().events, ["display"]);
    assert!(manager.rollback_if_confirmation_expired().unwrap());
    assert_eq!(backend.state.borrow().defaults, original_audio());
}

#[test]
fn all_apply_failure_stages_recover_audio_and_displays() {
    for failure in [
        Failure::Display,
        Failure::DisplayRestored,
        Failure::Console,
        Failure::Multimedia,
        Failure::Verification,
    ] {
        let (mut manager, backend, store) = fixture();
        backend.state.borrow_mut().failure = Some(failure);
        assert!(manager.apply_profile("TV").is_err());
        if manager.has_pending_confirmation() {
            manager.rollback_pending().unwrap();
        }
        assert_eq!(backend.state.borrow().defaults, original_audio());
        assert_eq!(backend.state.borrow().layout, desk());
        assert!(store.load().unwrap().pending_recovery.is_none());
        assert!(store.load().unwrap().pending_recovery_audio.is_none());
    }
}

#[test]
fn missing_endpoint_never_falls_back_to_an_identically_named_device() {
    let (mut manager, backend, _) = fixture();
    manager
        .save_profile_with_audio("Missing", Some(output("old-driver-id")))
        .unwrap();
    assert!(manager.apply_profile("Missing").is_err());
    assert_eq!(backend.state.borrow().defaults, original_audio());
    assert!(!manager.has_pending_confirmation());
    assert_eq!(
        manager
            .list_profiles()
            .iter()
            .find(|p| p.name == "Missing")
            .unwrap()
            .audio_output
            .as_ref()
            .unwrap()
            .id,
        "old-driver-id"
    );
}

#[test]
fn recovery_journal_survives_audio_restore_failure_and_restart() {
    let (mut manager, backend, store) = fixture();
    backend.state.borrow_mut().failure = Some(Failure::Console);
    backend.state.borrow_mut().fail_recovery = true;
    assert!(matches!(
        manager.apply_profile("TV"),
        Err(ManagerError::RecoveryRequired(_))
    ));
    assert!(manager.has_pending_confirmation());
    assert_eq!(
        store.load().unwrap().pending_recovery_audio,
        Some(original_audio())
    );
    drop(manager);
    let mut restarted = Manager::new(backend.clone(), store.clone()).unwrap();
    assert!(restarted.rollback_if_confirmation_expired().is_err());
    backend.state.borrow_mut().fail_recovery = false;
    assert!(restarted.rollback_if_confirmation_expired().unwrap());
    assert_eq!(backend.state.borrow().defaults, original_audio());
    assert_eq!(backend.state.borrow().layout, desk());
    assert!(store.load().unwrap().pending_recovery_audio.is_none());
}

#[test]
fn crash_after_successful_apply_recovers_before_confirmation() {
    let (mut manager, backend, store) = fixture();
    manager.apply_profile("TV").unwrap();
    drop(manager);
    let mut restarted = Manager::new(backend.clone(), store).unwrap();
    assert!(restarted.rollback_if_confirmation_expired().unwrap());
    assert_eq!(backend.state.borrow().defaults, original_audio());
}

#[test]
fn failed_journal_write_prevents_audio_or_display_mutation() {
    let (mut manager, backend, store) = fixture();
    *store.fail.borrow_mut() = true;
    assert!(manager.apply_profile("TV").is_err());
    assert!(backend.state.borrow().events.is_empty());
    assert!(!manager.has_pending_confirmation());
}

#[test]
fn failed_profile_audio_save_and_confirmation_leave_recoverable_state() {
    let (mut manager, backend, store) = fixture();
    *store.fail.borrow_mut() = true;
    assert!(manager.set_profile_audio("TV", None).is_err());
    assert!(manager.list_profiles()[0].audio_output.is_some());
    *store.fail.borrow_mut() = false;
    manager.apply_profile("TV").unwrap();
    *store.fail.borrow_mut() = true;
    assert!(manager.confirm_current_layout().is_err());
    assert_eq!(
        store.load().unwrap().pending_recovery_audio,
        Some(original_audio())
    );
    *store.fail.borrow_mut() = false;
    manager.rollback_pending().unwrap();
    assert_eq!(backend.state.borrow().defaults, original_audio());
}

#[test]
fn unavailable_audio_service_blocks_audio_profiles_but_not_display_only_profiles() {
    let (mut manager, backend, _) = fixture();
    backend.state.borrow_mut().unavailable = true;
    assert!(manager.apply_profile("TV").is_err());
    assert!(backend.state.borrow().events.is_empty());
    manager.set_profile_audio("TV", None).unwrap();
    manager.apply_profile("TV").unwrap();
    manager.rollback_pending().unwrap();
}

#[test]
fn optional_audio_fields_round_trip_and_current_display_only_configs_keep_working() {
    let (mut manager, _, store) = fixture();
    manager.apply_profile("TV").unwrap();
    let config = store.load().unwrap();
    let encoded = serde_json::to_string(&config).unwrap();
    let decoded: AppConfig = serde_json::from_str(&encoded).unwrap();
    assert_eq!(decoded, config);
    assert!(decoded.is_supported());
    let mut value = serde_json::to_value(config).unwrap();
    value
        .as_object_mut()
        .unwrap()
        .remove("pending_recovery_audio");
    value
        .as_object_mut()
        .unwrap()
        .remove("last_restorable_audio");
    value["profiles"][0]
        .as_object_mut()
        .unwrap()
        .remove("audio_output");
    let config: AppConfig = serde_json::from_value(value).unwrap();
    assert!(config.is_supported());
    assert!(config.profiles[0].audio_output.is_none());
}

#[test]
fn endpoint_ids_reject_embedded_nuls_and_empty_ids() {
    for id in ["", " ", "speakers\0ignored"] {
        assert!(!output(id).is_valid());
        assert!(!AudioDefaults::playback(id).is_valid());
    }
    let (manager, _, _) = fixture();
    let mut config = manager.config().clone();
    config.profiles[0].audio_output = Some(output("bad\0id"));
    assert!(!config.is_supported());
}
