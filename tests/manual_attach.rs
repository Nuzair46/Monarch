use monarch::*;

fn monitor(target: u32, x: i32, y: i32, enabled: bool) -> OutputConfig {
    OutputConfig {
        display_id: DisplayId {
            adapter_luid: 7,
            target_id: target,
            edid_hash: Some(u64::from(target)),
            identity: Default::default(),
        },
        enabled,
        primary: target == 10,
        position: Position { x, y },
        resolution: Resolution {
            width: 1920,
            height: 1080,
        },
        refresh_rate_mhz: 119_880,
        rotation: Some(Rotation::Landscape),
        hdr_enabled: None,
        scale_percent: None,
        clone_group: None,
    }
}

fn attach_and_revert(before: Layout, target: u32) -> Layout {
    let backend = MockBackend::new(vec![], before.clone()).unwrap();
    let mut manager =
        MonarchDisplayManager::new(backend.clone(), MemoryConfigStore::default()).unwrap();
    let id = &before
        .outputs
        .iter()
        .find(|o| o.display_id.target_id == target)
        .unwrap()
        .display_id;
    manager.toggle_display(id).unwrap();
    assert!(manager.has_pending_confirmation());
    let after = backend.current_layout().unwrap();
    assert!(after.ensure_supported().is_ok());
    for output in before.outputs.iter().filter(|o| o.enabled) {
        assert_eq!(
            after
                .outputs
                .iter()
                .find(|o| o.display_id == output.display_id)
                .unwrap(),
            output
        );
    }
    manager.rollback_pending().unwrap();
    assert_eq!(backend.current_layout().unwrap(), before);
    after
}

#[test]
fn reattach_keeps_valid_remembered_position_mode_and_other_detached_monitors() {
    let before = Layout {
        outputs: vec![
            monitor(10, 0, 0, true),
            monitor(30, -1920, 0, false),
            monitor(50, 1920, 0, true),
            monitor(70, 3840, 0, false),
        ],
    };
    let mut expected = before.clone();
    expected.outputs[1].enabled = true;
    assert_eq!(attach_and_revert(before, 30), expected);
}

#[test]
fn reattach_closes_a_stale_gap_without_moving_active_displays() {
    for position in [Position { x: -3840, y: 0 }, Position { x: 0, y: -2160 }] {
        let before = Layout {
            outputs: vec![
                monitor(10, 0, 0, true),
                monitor(30, position.x, position.y, false),
                monitor(50, 1920, 0, true),
            ],
        };
        let after = attach_and_revert(before, 30);
        assert_eq!(
            after.outputs[1].position,
            Position {
                x: position.x / 2,
                y: position.y / 2
            }
        );
        assert_eq!(after.outputs[1].refresh_rate_mhz, 119_880);
    }
}

#[test]
fn reattach_avoids_overlap_in_an_offset_portrait_desktop() {
    let mut upper = monitor(50, 1920, -1600, true);
    upper.resolution = Resolution {
        width: 1080,
        height: 1920,
    };
    upper.rotation = Some(Rotation::Portrait);
    let before = Layout {
        outputs: vec![monitor(10, 0, 0, true), monitor(30, 0, 0, false), upper],
    };
    let after = attach_and_revert(before, 30);
    assert_eq!(after.outputs[1].position, Position { x: 0, y: -1080 });
}

#[test]
fn reattach_requires_an_edge_rather_than_only_a_corner() {
    let before = Layout {
        outputs: vec![monitor(10, 0, 0, true), monitor(30, 1920, 1080, false)],
    };
    let after = attach_and_revert(before, 30);
    let position = &after.outputs[1].position;
    assert!((position.x == 1920 && position.y < 1080) || (position.y == 1080 && position.x < 1920));
}

#[test]
fn attach_with_no_geometry_keeps_automatic_mode_for_windows_to_resolve() {
    let mut unknown = monitor(30, 0, 0, false);
    unknown.resolution = Resolution {
        width: 0,
        height: 0,
    };
    let after = attach_and_revert(
        Layout {
            outputs: vec![monitor(10, 0, 0, true), unknown],
        },
        30,
    );
    assert_eq!(
        after.outputs[1].resolution,
        Resolution {
            width: 0,
            height: 0
        }
    );
}

struct PartialModeBackend {
    inner: MockBackend,
    reject_native: bool,
}

impl DisplayBackend for PartialModeBackend {
    fn get_layout(&self) -> Result<Layout, ManagerError> {
        self.inner.get_layout()
    }
    fn list_displays(&self) -> Result<Vec<DisplayInfo>, ManagerError> {
        self.inner.list_displays()
    }
    fn validate_layout(&self, requested: &Layout) -> Result<(), ManagerError> {
        use monarch::capabilities::{DisplayCapabilities, DisplayMode};
        let inventory = self.inner.get_layout()?;
        let capabilities = inventory
            .outputs
            .iter()
            .map(|o| DisplayCapabilities {
                display_id: o.display_id.clone(),
                modes: vec![DisplayMode {
                    resolution: o.resolution.clone(),
                    refresh_rate_mhz: if o.enabled {
                        o.refresh_rate_mhz
                    } else {
                        60_000
                    },
                }],
                modes_unavailable_reason: (!o.enabled)
                    .then(|| "Detached: partial mode list".into()),
                hdr_supported: false,
                hdr_enabled: None,
                hdr_unavailable_reason: Some("Unavailable".into()),
                scale_percent: None,
                scale_percentages: vec![],
                scaling_unavailable_reason: Some("Unavailable".into()),
            })
            .collect::<Vec<_>>();
        capabilities::validate(requested, &capabilities)?;
        assert_eq!(requested.outputs[1].refresh_rate_mhz, 119_880);
        if self.reject_native {
            return Err(ManagerError::Validation("native mode rejection".into()));
        }
        Ok(())
    }
    fn apply_layout(&self, layout: Layout) -> Result<(), ManagerError> {
        self.inner.apply_layout(layout)
    }
}

#[test]
fn manual_attach_validates_the_remembered_mode_without_silently_substituting_a_listed_mode() {
    for reject_native in [false, true] {
        let before = Layout {
            outputs: vec![monitor(10, 0, 0, true), monitor(30, 1920, 0, false)],
        };
        let inner = MockBackend::new(vec![], before.clone()).unwrap();
        let store = MemoryConfigStore::default();
        let backend = PartialModeBackend {
            inner: inner.clone(),
            reject_native,
        };
        let mut manager = MonarchDisplayManager::new(backend, store.clone()).unwrap();
        let result = manager.toggle_display(&before.outputs[1].display_id);
        if reject_native {
            assert!(result
                .unwrap_err()
                .to_string()
                .contains("native mode rejection"));
            assert_eq!(inner.current_layout().unwrap(), before);
            assert!(!manager.has_pending_confirmation());
            assert!(store.snapshot().unwrap().pending_recovery.is_none());
        } else {
            result.unwrap();
            let attached = inner.current_layout().unwrap();
            assert!(attached.outputs[1].enabled);
            assert_eq!(attached.outputs[1].refresh_rate_mhz, 119_880);
            manager.confirm_current_layout().unwrap();
            assert!(!manager.has_pending_confirmation());
        }
    }
}
