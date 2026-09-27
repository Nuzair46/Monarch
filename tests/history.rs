use monarch::{history::GeometryHistory, *};

fn observed(serial: &str) -> Layout {
    Layout {
        outputs: vec![OutputConfig {
            display_id: DisplayId {
                adapter_luid: 1,
                target_id: 1,
                edid_hash: Some(7),
                identity: MonitorIdentity {
                    edid_serial: Some(serial.into()),
                    device_path: Some("port".into()),
                },
            },
            enabled: true,
            primary: true,
            rotation: Some(Rotation::Portrait),
            hdr_enabled: None,
            scale_percent: None,
            clone_group: None,
            resolution: Resolution {
                width: 1080,
                height: 1920,
            },
            position: Position { x: -1080, y: 0 },
            refresh_rate_mhz: 60_000,
        }],
    }
}

#[test]
fn history_restores_preferences_only_for_a_live_inactive_target() {
    let old = observed("panel-a");
    let mut history = GeometryHistory::default();
    history.remember(&old);
    let mut disconnected = Layout::default();
    history.complete_inventory(&mut disconnected);
    assert!(disconnected.outputs.is_empty());
    let mut inactive = old.clone();
    inactive.outputs[0].enabled = false;
    inactive.outputs[0].display_id.adapter_luid = 99;
    inactive.outputs[0].resolution = Resolution {
        width: 0,
        height: 0,
    };
    inactive.outputs[0].rotation = None;
    history.complete_inventory(&mut inactive);
    assert!(!inactive.outputs[0].enabled);
    assert_eq!(inactive.outputs[0].display_id.adapter_luid, 99);
    assert_eq!(inactive.outputs[0].resolution, old.outputs[0].resolution);
    assert_eq!(inactive.outputs[0].rotation, Some(Rotation::Portrait));
}

#[test]
fn reused_endpoint_keeps_different_physical_histories_without_confusing_them() {
    let mut history = GeometryHistory::default();
    history.remember(&observed("panel-a"));
    let mut replacement = observed("panel-b");
    replacement.outputs[0].resolution.width = 1440;
    history.remember(&replacement);
    assert!(history.is_valid());
    let history: GeometryHistory =
        serde_json::from_str(&serde_json::to_string(&history).unwrap()).unwrap();
    let mut original = observed("panel-a");
    original.outputs[0].enabled = false;
    original.outputs[0].resolution = Resolution {
        width: 0,
        height: 0,
    };
    history.complete_inventory(&mut original);
    assert_eq!(original.outputs[0].resolution.width, 1080);
}

#[test]
fn active_geometry_always_wins_over_history() {
    let mut history = GeometryHistory::default();
    history.remember(&observed("panel-a"));
    let mut current = observed("panel-a");
    current.outputs[0].resolution.width = 1440;
    let original = current.clone();
    history.complete_inventory(&mut current);
    assert_eq!(current, original);
}

#[test]
fn version_one_geometry_is_not_reused_by_monarch_two() {
    let mut history = GeometryHistory::default();
    history.remember(&observed("old-panel"));
    let mut old = serde_json::to_value(history).unwrap();
    old["version"] = serde_json::json!(1);
    let old: GeometryHistory = serde_json::from_value(old).unwrap();
    assert!(!old.is_valid());
}
