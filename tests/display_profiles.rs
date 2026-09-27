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
            physical_size_mm: None,
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
fn draft_save_retains_disconnected_settings_and_never_applies() {
    let backend = MockBackend::new(vec![], layout()).unwrap();
    let store = MemoryConfigStore::default();
    let mut manager = MonarchDisplayManager::new(backend.clone(), store.clone()).unwrap();
    let mut draft = cloned();
    draft.outputs[2].display_id.target_id = 99;
    draft.outputs[2].display_id.edid_hash = Some(99);
    draft.outputs[2].display_id.identity = MonitorIdentity {
        device_path: Some("missing".into()),
        edid_serial: Some("missing".into()),
    };
    draft.outputs[2].hdr_enabled = Some(true);
    manager
        .save_profile_layout("Edited".into(), draft.clone())
        .unwrap();
    assert_eq!(backend.current_layout().unwrap(), layout());
    assert!(!manager.has_pending_confirmation());
    assert_eq!(manager.list_profiles()[0].layout, draft);
    assert!(manager.apply_profile("Edited").is_err());
    assert_eq!(backend.current_layout().unwrap(), layout());
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
