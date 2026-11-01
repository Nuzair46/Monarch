use monarch::{
    capabilities::{DisplayCapabilities, DisplayMode},
    *,
};
use std::sync::{Arc, Mutex};

fn profile() -> Profile {
    // Reporter layout from #76, with adapter/connection identifiers anonymized.
    serde_json::from_str(include_str!("fixtures/issue_76_profile.json")).unwrap()
}

fn capabilities(layout: &Layout) -> Vec<DisplayCapabilities> {
    layout
        .outputs
        .iter()
        .map(|output| DisplayCapabilities {
            display_id: output.display_id.clone(),
            modes: vec![DisplayMode {
                resolution: output.resolution.clone(),
                refresh_rate_mhz: output.refresh_rate_mhz,
            }],
            modes_unavailable_reason: None,
            hdr_supported: false,
            hdr_enabled: Some(false),
            hdr_unavailable_reason: None,
            scale_percent: Some(100),
            scale_percentages: vec![100],
            scaling_unavailable_reason: None,
        })
        .collect()
}

#[test]
fn reporter_profile_uses_the_supported_portrait_desktop_rectangle() {
    let saved = profile().layout;
    let original = saved.clone();
    let repaired = profile_geometry::repair(&saved, &capabilities(&saved)).unwrap();
    assert_eq!(repaired.outputs[..3], saved.outputs[..3]);
    let mut expected = saved.outputs[3].clone();
    expected.resolution = Resolution {
        width: 1080,
        height: 1920,
    };
    assert_eq!(repaired.outputs[3], expected);
    assert_eq!(saved, original);
    capabilities::validate(&repaired, &capabilities(&saved)).unwrap();
    assert!(verification::verify_applied_layout(&saved, &repaired).is_err());
    verification::verify_applied_layout(&repaired, &repaired).unwrap();
    assert_eq!(
        profile_geometry::repair(&repaired, &capabilities(&saved)).unwrap(),
        repaired
    );
}

#[test]
fn corrected_left_hand_portrait_closes_its_gap_without_moving_primary() {
    let saved = profile().layout;
    let mut saved = Layout {
        outputs: vec![saved.outputs[0].clone(), saved.outputs[3].clone()],
    };
    saved.outputs[1].position = Position { x: -1920, y: 0 };
    let repaired = profile_geometry::repair(&saved, &capabilities(&saved)).unwrap();
    assert_eq!(repaired.outputs[0], saved.outputs[0]);
    assert_eq!(repaired.outputs[1].position, Position { x: -1080, y: 0 });
    assert_eq!(repaired.outputs[1].refresh_rate_mhz, 100_000);
    repaired.ensure_supported().unwrap();
}

#[test]
fn repair_requires_complete_matching_mode_evidence() {
    let saved = profile().layout;
    for scenario in 0..5 {
        let mut caps = capabilities(&saved);
        match scenario {
            0 => caps[3].modes_unavailable_reason = Some("detached".into()),
            1 => caps[3].modes.clear(),
            2 => caps[3].modes[0].refresh_rate_mhz = 60_000,
            3 => caps[3].display_id.edid_hash = Some(99),
            _ => {
                caps.pop();
            }
        }
        assert_eq!(profile_geometry::repair(&saved, &caps).unwrap(), saved);
    }
}

#[test]
fn supported_rotated_native_portrait_and_custom_modes_are_not_changed() {
    let mut saved = profile().layout;
    let mut caps = capabilities(&saved);
    caps[3].modes.push(DisplayMode {
        resolution: Resolution {
            width: 1080,
            height: 1920,
        },
        refresh_rate_mhz: saved.outputs[3].refresh_rate_mhz,
    });
    assert_eq!(profile_geometry::repair(&saved, &caps).unwrap(), saved);
    for rotation in [
        None,
        Some(Rotation::Landscape),
        Some(Rotation::LandscapeFlipped),
    ] {
        saved.outputs[3].rotation = rotation;
        assert_eq!(
            profile_geometry::repair(&saved, &capabilities(&saved)).unwrap(),
            saved
        );
    }
    saved.outputs[3].rotation = Some(Rotation::PortraitFlipped);
    assert_eq!(
        profile_geometry::repair(&saved, &capabilities(&saved))
            .unwrap()
            .outputs[3]
            .resolution,
        Resolution {
            width: 1080,
            height: 1920
        }
    );
    saved.outputs[3].enabled = false;
    assert_eq!(
        profile_geometry::repair(&saved, &capabilities(&saved)).unwrap(),
        saved
    );
}

#[test]
fn clone_sources_are_not_reinterpreted_and_new_overlaps_are_rejected() {
    let mut saved = profile().layout;
    saved.outputs[3].clone_group = Some("clone".into());
    assert_eq!(
        profile_geometry::repair(&saved, &capabilities(&saved)).unwrap(),
        saved
    );
    saved.outputs[3].clone_group = None;
    let mut lower = saved.outputs[3].clone();
    lower.display_id.target_id = 99;
    lower.display_id.edid_hash = Some(99);
    lower.rotation = Some(Rotation::Landscape);
    lower.position.y += 1080;
    saved.outputs.push(lower);
    saved.ensure_supported().unwrap();
    assert!(profile_geometry::repair(&saved, &capabilities(&saved)).is_err());
}

#[derive(Clone)]
struct Backend {
    inner: MockBackend,
    caps: Vec<DisplayCapabilities>,
    profile_calls: Arc<Mutex<usize>>,
}

impl DisplayBackend for Backend {
    fn list_displays(&self) -> Result<Vec<DisplayInfo>, ManagerError> {
        self.inner.list_displays()
    }
    fn get_layout(&self) -> Result<Layout, ManagerError> {
        self.inner.get_layout()
    }
    fn get_display_capabilities(&self) -> Result<Vec<DisplayCapabilities>, ManagerError> {
        Ok(self.caps.clone())
    }
    fn validate_layout(&self, layout: &Layout) -> Result<(), ManagerError> {
        capabilities::validate_transition(layout, &self.get_layout()?, &self.caps)
    }
    fn apply_layout(&self, layout: Layout) -> Result<(), ManagerError> {
        self.inner.apply_layout(layout)
    }
    fn apply_profile_layout(&self, layout: Layout) -> Result<(), ManagerError> {
        *self.profile_calls.lock().unwrap() += 1;
        self.inner.apply_layout(layout)
    }
}

#[test]
fn manager_repairs_profile_before_preflight_but_manual_edits_remain_exact() {
    let saved = profile();
    let mut current = saved.layout.clone();
    current.outputs[3].rotation = Some(Rotation::Landscape);
    let backend = Backend {
        inner: MockBackend::new(vec![], current.clone()).unwrap(),
        caps: capabilities(&saved.layout),
        profile_calls: Default::default(),
    };
    let store = MemoryConfigStore::new(AppConfig {
        profiles: vec![saved.clone()],
        ..Default::default()
    });
    let mut manager = MonarchDisplayManager::new(backend.clone(), store.clone()).unwrap();
    assert!(manager.apply_layout(saved.layout.clone()).is_err());
    assert_eq!(*backend.profile_calls.lock().unwrap(), 0);
    assert_eq!(backend.get_layout().unwrap(), current);
    manager.apply_profile("Laptop").unwrap();
    assert_eq!(*backend.profile_calls.lock().unwrap(), 1);
    assert_eq!(
        backend.get_layout().unwrap().outputs[3].resolution,
        Resolution {
            width: 1080,
            height: 1920
        }
    );
    assert_eq!(manager.list_profiles(), vec![saved]);
    assert_eq!(
        store.load().unwrap().pending_recovery,
        Some(current.clone())
    );
    manager.rollback_pending().unwrap();
    assert_eq!(
        *backend.profile_calls.lock().unwrap(),
        1,
        "rollback must use exact layouts"
    );
    assert_eq!(backend.get_layout().unwrap(), current);
}
