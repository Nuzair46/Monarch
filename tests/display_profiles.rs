use monarch::capabilities::*;
use monarch::*;
use std::cell::RefCell;
fn layout() -> Layout {
    Layout {
        outputs: (1..=3)
            .map(|id| OutputConfig {
                display_id: DisplayId {
                    adapter_luid: 1,
                    target_id: id,
                    edid_hash: Some(id.into()),
                    identity: MonitorIdentity {
                        device_path: Some(format!("port{id}")),
                        edid_serial: Some(format!("serial{id}")),
                    },
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
                refresh_rate_mhz: 59940,
                rotation: Some(Rotation::Landscape),
                hdr_enabled: Some(false),
                scale_percent: Some(125),
                clone_group: None,
            })
            .collect(),
    }
}
fn caps(layout: &Layout) -> Vec<DisplayCapabilities> {
    layout
        .outputs
        .iter()
        .map(|o| DisplayCapabilities {
            display_id: o.display_id.clone(),
            modes: vec![DisplayMode {
                resolution: o.resolution.clone(),
                refresh_rate_mhz: o.refresh_rate_mhz,
            }],
            modes_unavailable_reason: None,
            hdr_supported: true,
            hdr_enabled: Some(false),
            hdr_unavailable_reason: None,
            scale_percent: Some(125),
            scale_percentages: vec![100, 125, 150],
            scaling_unavailable_reason: None,
        })
        .collect()
}
fn cloned() -> Layout {
    let mut l = layout();
    l.outputs[0].clone_group = Some("pair".into());
    l.outputs[1].clone_group = Some("pair".into());
    l.outputs[1].position.x = 0;
    l.outputs[1].primary = true;
    l
}
#[test]
fn capabilities_reject_missing_modes_hdr_and_scaling_without_rounding_refresh() {
    let l = layout();
    let supported = caps(&l);
    assert!(validate(&l, &supported).is_ok());
    for field in 0..6 {
        let mut c = supported.clone();
        match field {
            0 => {
                c.remove(1);
            }
            1 => c[1].modes.clear(),
            2 => c[1].modes[0].refresh_rate_mhz = 60000,
            3 => c[1].hdr_enabled = None,
            4 => c[1].scale_percent = None,
            _ => c[1].scale_percentages = vec![100],
        }
        assert!(validate(&l, &c).is_err(), "field {field}");
    }
    let mut portrait = l.clone();
    portrait.outputs[0].rotation = Some(Rotation::Portrait);
    portrait.outputs[0].resolution = Resolution {
        width: 1080,
        height: 1920,
    };
    assert!(validate(&portrait, &supported).is_ok());
}
#[test]
fn partial_mode_lists_defer_exact_mode_validation_to_the_native_backend() {
    let l = layout();
    let mut supported = caps(&l);
    supported[1].modes[0].refresh_rate_mhz = 60_000;
    supported[1].modes_unavailable_reason = Some("Detached monitor: partial mode list".into());
    assert!(validate(&l, &supported).is_ok());
    supported[1].modes.clear();
    assert!(validate(&l, &supported).is_ok());
    // An incomplete mode list does not relax identity, HDR, or scaling checks.
    supported[1].hdr_enabled = None;
    assert!(validate(&l, &supported).is_err());
    supported[1].hdr_enabled = Some(false);
    supported[1].scale_percentages.clear();
    assert!(validate(&l, &supported).is_err());
    supported.remove(1);
    assert!(validate(&l, &supported).is_err());
}
#[test]
fn clone_groups_share_primary_but_never_infer_membership_from_overlap() {
    let mut l = cloned();
    assert!(l.ensure_supported().is_ok());
    l.outputs[1].clone_group = None;
    assert!(l.ensure_supported().is_err());
    for field in 0..4 {
        let mut l = cloned();
        match field {
            0 => l.outputs[1].scale_percent = Some(150),
            1 => l.outputs[1].resolution.width = 1280,
            2 => l.outputs[1].position.y = 10,
            _ => l.outputs[1].primary = false,
        };
        assert!(l.ensure_valid().is_err());
    }
}
#[test]
fn verification_compares_members_not_local_group_names_and_checks_preferences() {
    let l = cloned();
    let mut got = l.clone();
    for o in got.outputs.iter_mut().take(2) {
        o.clone_group = Some("other".into());
    }
    assert!(verification::verify_applied_layout(&l, &got).is_ok());
    for field in 0..3 {
        let mut got = l.clone();
        match field {
            0 => got.outputs[1].clone_group = None,
            1 => got.outputs[0].hdr_enabled = Some(true),
            _ => got.outputs[0].scale_percent = Some(150),
        };
        assert!(verification::verify_applied_layout(&l, &got).is_err());
    }
}
#[test]
fn saving_current_profile_preserves_preferences_without_applying() {
    let current = cloned();
    let backend = MockBackend::new(vec![], current.clone()).unwrap();
    let mut manager =
        MonarchDisplayManager::new(backend.clone(), MemoryConfigStore::default()).unwrap();
    manager.save_profile("Desk").unwrap();
    assert_eq!(backend.current_layout().unwrap(), current);
    assert!(!manager.has_pending_confirmation());
    assert_eq!(manager.list_profiles()[0].layout, current);
}
#[test]
fn detach_clone_member_collapses_group_and_reattach_extends_without_overlap() {
    let backend = MockBackend::new(vec![], cloned()).unwrap();
    let mut manager =
        MonarchDisplayManager::new(backend.clone(), MemoryConfigStore::default()).unwrap();
    manager
        .toggle_display(&cloned().outputs[1].display_id)
        .unwrap();
    manager.confirm_current_layout().unwrap();
    let detached = backend.current_layout().unwrap();
    assert!(detached.outputs[0].primary);
    assert!(detached.outputs[0].clone_group.is_none());
    manager
        .toggle_display(&cloned().outputs[1].display_id)
        .unwrap();
    let extended = backend.current_layout().unwrap();
    assert!(extended.ensure_supported().is_ok());
    assert!(extended.outputs[1].clone_group.is_none());
}
#[test]
fn persisted_recovery_restores_modes_hdr_scale_and_clones_after_restart() {
    let previous = cloned();
    let backend = MockBackend::new(vec![], previous.clone()).unwrap();
    let store = MemoryConfigStore::default();
    let mut manager = MonarchDisplayManager::new(backend.clone(), store.clone()).unwrap();
    manager.apply_layout(layout()).unwrap();
    drop(manager);
    let mut restarted = MonarchDisplayManager::new(backend.clone(), store).unwrap();
    assert!(restarted.rollback_if_confirmation_expired().unwrap());
    assert_eq!(backend.current_layout().unwrap(), previous);
}
#[test]
fn every_failed_stage_restores_all_captured_settings_and_failed_recovery_is_explicit() {
    for fail_at in 0..5 {
        let previous = cloned();
        let state = RefCell::new(previous.clone());
        let result = transaction::apply_with_recovery(
            || {
                for stage in 0..5 {
                    match stage {
                        0 => *state.borrow_mut() = layout(),
                        1 => {
                            state.borrow_mut().outputs[1].rotation =
                                Some(Rotation::LandscapeFlipped)
                        }
                        2 => state.borrow_mut().outputs[0].hdr_enabled = Some(true),
                        3 => state.borrow_mut().outputs[0].scale_percent = Some(150),
                        _ => {}
                    }
                    if stage == fail_at {
                        return Err(ManagerError::Backend(format!("failure at stage {stage}")));
                    }
                }
                Ok(())
            },
            || {
                *state.borrow_mut() = previous.clone();
                verification::verify_applied_layout(&previous, &state.borrow())
            },
        );
        assert!(matches!(result, Err(ManagerError::ApplyRestored(_))));
        assert_eq!(*state.borrow(), previous);
    }
    assert!(matches!(
        transaction::apply_with_recovery(
            || Err(ManagerError::Backend("apply".into())),
            || Err(ManagerError::Backend("rollback".into()))
        ),
        Err(ManagerError::RecoveryRequired(_))
    ));
}

#[test]
fn saved_profile_identity_is_not_replaced_by_a_different_panel_on_the_same_port() {
    let old = layout();
    let mut live = old.clone();
    live.outputs[1].display_id.identity.edid_serial = Some("different-panel".into());
    let backend = MockBackend::new(vec![], live).unwrap();
    let mut config = AppConfig::default();
    config.profiles.push(Profile {
        audio_output: None,
        name: "Disconnected".into(),
        layout: old.clone(),
    });
    let mut manager = MonarchDisplayManager::new(backend, MemoryConfigStore::new(config)).unwrap();
    assert_eq!(
        manager.list_profiles()[0].layout.outputs[1].display_id,
        old.outputs[1].display_id
    );
    assert!(manager.apply_profile("Disconnected").is_err());
}
#[test]
fn an_unobservable_previous_preference_cannot_be_lost_from_the_recovery_journal() {
    let mut unknown = layout();
    unknown.outputs[0].hdr_enabled = None;
    let backend = MockBackend::new(vec![], unknown.clone()).unwrap();
    let store = MemoryConfigStore::default();
    let mut manager = MonarchDisplayManager::new(backend.clone(), store.clone()).unwrap();
    assert!(manager.apply_layout(layout()).is_err());
    assert_eq!(backend.current_layout().unwrap(), unknown);
    assert!(store.snapshot().unwrap().pending_recovery.is_none());
}

#[test]
fn only_connected_detached_targets_defer_hdr_and_scaling_validation() {
    let desired = layout();
    let mut current = desired.clone();
    current.outputs[2].enabled = false;
    let mut capabilities = caps(&desired);
    capabilities[2].hdr_supported = false;
    capabilities[2].hdr_enabled = None;
    capabilities[2].scale_percent = None;
    capabilities[2].scale_percentages.clear();

    assert!(validate_transition(&desired, &current, &capabilities).is_ok());
    // Once active, unknown capabilities must still fail, rather than silently
    // dropping the saved preference or treating unknown as HDR off/100% DPI.
    assert!(validate(&desired, &capabilities).is_err());
    assert!(validate_transition(&desired, &desired, &capabilities).is_err());

    // Identity and known mode restrictions still apply before activation.
    let mut wrong_mode = capabilities.clone();
    wrong_mode[2].modes[0].refresh_rate_mhz = 60_000;
    assert!(validate_transition(&desired, &current, &wrong_mode).is_err());
    let mut replaced = capabilities.clone();
    replaced[2].display_id.identity.edid_serial = Some("replacement".into());
    assert!(validate_transition(&desired, &current, &replaced).is_err());
    assert!(validate_transition(&desired, &current, &capabilities[..2]).is_err());
    current.outputs.pop();
    assert!(validate_transition(&desired, &current, &capabilities).is_err());
}

/// Model the Windows boundary: inactive targets cannot report HDR/DPI, topology
/// activation precedes preference checks, and failed checks restore the capture.
#[derive(Clone)]
struct PreferenceBackend {
    inner: MockBackend,
    supported: std::rc::Rc<RefCell<Vec<DisplayCapabilities>>>,
}

impl PreferenceBackend {
    fn new(current: Layout) -> Self {
        Self {
            supported: std::rc::Rc::new(RefCell::new(caps(&current))),
            inner: MockBackend::new(vec![], current).unwrap(),
        }
    }
}

impl DisplayBackend for PreferenceBackend {
    fn list_displays(&self) -> Result<Vec<DisplayInfo>, ManagerError> {
        self.inner.list_displays()
    }

    fn get_layout(&self) -> Result<Layout, ManagerError> {
        self.inner.get_layout()
    }

    fn get_display_capabilities(&self) -> Result<Vec<DisplayCapabilities>, ManagerError> {
        let current = self.get_layout()?;
        let mut capabilities = self.supported.borrow().clone();
        for cap in &mut capabilities {
            let output = current
                .outputs
                .iter()
                .find(|o| o.display_id == cap.display_id)
                .unwrap();
            if output.enabled {
                cap.hdr_enabled = cap.hdr_enabled.and(output.hdr_enabled);
                cap.scale_percent = cap.scale_percent.and(output.scale_percent);
            } else {
                cap.hdr_supported = false;
                cap.hdr_enabled = None;
                cap.scale_percent = None;
                cap.scale_percentages.clear();
            }
        }
        Ok(capabilities)
    }

    fn validate_layout(&self, desired: &Layout) -> Result<(), ManagerError> {
        validate_transition(
            desired,
            &self.get_layout()?,
            &self.get_display_capabilities()?,
        )
    }

    fn apply_layout(&self, desired: Layout) -> Result<(), ManagerError> {
        let previous = self.get_layout()?;
        transaction::apply_with_recovery(
            || {
                let mut activated = desired.clone();
                for output in &mut activated.outputs {
                    let old = previous
                        .outputs
                        .iter()
                        .find(|o| o.display_id == output.display_id)
                        .unwrap();
                    output.hdr_enabled = output.enabled.then_some(old.hdr_enabled.unwrap_or(false));
                    output.scale_percent =
                        output.enabled.then_some(old.scale_percent.unwrap_or(100));
                }
                self.inner.apply_layout(activated)?;
                validate(&desired, &self.get_display_capabilities()?)?;
                let mut applied = desired.clone();
                for output in applied.outputs.iter_mut().filter(|o| !o.enabled) {
                    output.hdr_enabled = None;
                    output.scale_percent = None;
                }
                self.inner.apply_layout(applied)?;
                verification::verify_applied_layout(&desired, &self.get_layout()?)
            },
            || self.inner.apply_layout(previous.clone()),
        )
    }
}

#[test]
fn profiles_reattach_with_saved_hdr_and_scaling_and_can_revert_after_restart() {
    for (hdr_supported, hdr) in [(false, false), (true, false), (true, true)] {
        let mut all = layout();
        all.outputs[2].hdr_enabled = Some(hdr);
        all.outputs[2].scale_percent = Some(150);
        let backend = PreferenceBackend::new(all.clone());
        backend.supported.borrow_mut()[2].hdr_supported = hdr_supported;
        let store = MemoryConfigStore::default();
        let mut manager = MonarchDisplayManager::new(backend.clone(), store.clone()).unwrap();

        manager.save_profile("All monitors").unwrap();
        manager.toggle_display(&all.outputs[2].display_id).unwrap();
        manager.confirm_current_layout().unwrap();
        manager.save_profile("Desk").unwrap();
        let desk = backend.get_layout().unwrap();
        assert!(!desk.outputs[2].enabled);
        assert_eq!(desk.outputs[2].hdr_enabled, None);
        assert_eq!(desk.outputs[2].scale_percent, None);

        manager.apply_profile("All monitors").unwrap();
        assert_eq!(backend.get_layout().unwrap(), all);
        assert!(manager.has_pending_confirmation());
        manager.confirm_current_layout().unwrap();
        manager.apply_profile("Desk").unwrap();
        manager.confirm_current_layout().unwrap();

        manager.set_confirmation_timeout(std::time::Duration::ZERO);
        manager.apply_profile("All monitors").unwrap();
        assert!(manager.rollback_if_confirmation_expired().unwrap());
        assert_eq!(backend.get_layout().unwrap(), desk);

        manager.apply_profile("All monitors").unwrap();
        drop(manager);
        let mut restarted = MonarchDisplayManager::new(backend.clone(), store.clone()).unwrap();
        assert!(restarted.rollback_if_confirmation_expired().unwrap());
        assert_eq!(backend.get_layout().unwrap(), desk);
        assert!(store.snapshot().unwrap().pending_recovery.is_none());
        assert_eq!(restarted.list_profiles()[0].layout, all);
    }
}

#[test]
fn unsupported_preferences_after_reattach_restore_the_previous_profile() {
    for failure in 0..4 {
        let mut all = layout();
        all.outputs[2].hdr_enabled = Some(true);
        all.outputs[2].scale_percent = Some(150);
        let backend = PreferenceBackend::new(all.clone());
        let store = MemoryConfigStore::default();
        let mut manager = MonarchDisplayManager::new(backend.clone(), store.clone()).unwrap();
        manager.save_profile("All monitors").unwrap();
        manager.toggle_display(&all.outputs[2].display_id).unwrap();
        manager.confirm_current_layout().unwrap();
        manager.save_profile("Desk").unwrap();
        let desk = backend.get_layout().unwrap();
        match failure {
            0 => backend.supported.borrow_mut()[2].hdr_supported = false,
            1 => backend.supported.borrow_mut()[2].hdr_enabled = None,
            2 => backend.supported.borrow_mut()[2].scale_percentages = vec![100],
            _ => backend.supported.borrow_mut()[2].scale_percent = None,
        }

        let result = manager.apply_profile("All monitors");
        assert!(
            matches!(result, Err(ManagerError::ApplyRestored(_))),
            "{result:?}"
        );
        assert_eq!(backend.get_layout().unwrap(), desk);
        assert!(!manager.has_pending_confirmation());
        assert!(store.snapshot().unwrap().pending_recovery.is_none());
        assert_eq!(manager.list_profiles()[0].layout, all);
    }
}
