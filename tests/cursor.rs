use monarch::{cursor::*, *};
fn output(id: u32, x: i32, y: i32) -> OutputConfig {
    OutputConfig {
        display_id: DisplayId {
            adapter_luid: 1,
            target_id: id,
            edid_hash: Some(id as u64),
            identity: MonitorIdentity {
                device_path: Some(format!("port-{id}")),
                edid_serial: Some(format!("serial-{id}")),
            },
        },
        enabled: true,
        primary: id == 1,
        position: Position { x, y },
        resolution: Resolution {
            width: 1920,
            height: 1080,
        },
        refresh_rate_mhz: 60000,
        rotation: Some(Rotation::Landscape),
        hdr_enabled: None,
        scale_percent: None,
        clone_group: None,
    }
}
fn calibration(o: &OutputConfig, w: u32, h: u32, x: i32, y: i32) -> Calibration {
    Calibration {
        display_key: identity::display_key(&o.display_id),
        identity: o.display_id.identity.clone(),
        width_mm: w,
        height_mm: h,
        position_mm: Position { x, y },
        clone_representative: false,
    }
}
fn context() -> InputContext {
    InputContext {
        injected: false,
        control_down: false,
        confined: false,
        input_desktop_available: true,
    }
}
fn pair() -> (Layout, Vec<Calibration>) {
    let l = Layout {
        outputs: vec![output(1, 0, 0), output(2, 1920, 0)],
    };
    let c = vec![
        calibration(&l.outputs[0], 600, 340, 0, 0),
        calibration(&l.outputs[1], 300, 170, 600, 0),
    ];
    (l, c)
}
#[test]
fn horizontal_mapping_matches_physical_height_and_preserves_interior_movement() {
    let (l, c) = pair();
    let m = Mapping::build(&l, &c);
    let p = m
        .map_motion(
            Point { x: 1910, y: 270 },
            Point { x: 1930, y: 270 },
            context(),
        )
        .unwrap();
    assert_eq!(p.y, 540);
    assert!((1920..1950).contains(&p.x));
    assert!(m
        .map_motion(Point { x: 10, y: 20 }, Point { x: 100, y: 90 }, context())
        .is_none());
    assert!(m
        .map_motion(
            Point { x: 1910, y: 900 },
            Point { x: 1930, y: 900 },
            context()
        )
        .is_none());
    // A clipped native edge still provides an outgoing direction.
    assert!(m
        .map_motion(
            Point { x: 1910, y: 270 },
            Point { x: 1919, y: 270 },
            context()
        )
        .is_some());
}
#[test]
fn maps_vertical_boundaries_and_physical_offsets() {
    let (mut l, mut c) = pair();
    l.outputs[1].position = Position { x: 0, y: 1080 };
    c[1].position_mm = Position { x: 0, y: 340 };
    let m = Mapping::build(&l, &c);
    let p = m
        .map_motion(
            Point { x: 480, y: 1070 },
            Point { x: 480, y: 1100 },
            context(),
        )
        .unwrap();
    assert_eq!(p.x, 960);
    assert!(p.y > 1080);
    c[1].position_mm.x = 100;
    let m = Mapping::build(&l, &c);
    let p = m
        .map_motion(
            Point { x: 480, y: 1070 },
            Point { x: 480, y: 1100 },
            context(),
        )
        .unwrap();
    assert_eq!(p.x, 320);
}
#[test]
fn negative_coordinates_and_leftward_crossings() {
    let (mut l, mut c) = pair();
    l.outputs[1].position.x = -1920;
    c[1].position_mm.x = -300;
    let m = Mapping::build(&l, &c);
    let p = m
        .map_motion(Point { x: 10, y: 270 }, Point { x: -20, y: 270 }, context())
        .unwrap();
    assert!(p.x < 0 && p.x > -100);
    assert_eq!(p.y, 540);
    let reverse = m
        .map_motion(Point { x: -10, y: 540 }, Point { x: 20, y: 540 }, context())
        .unwrap();
    assert!((0..100).contains(&reverse.x));
    assert_eq!(reverse.y, 270);
}
#[test]
fn rotation_swaps_panel_dimensions_and_dpi_does_not_change_mapping() {
    let (mut l, mut c) = pair();
    c[1].width_mm = 600;
    c[1].height_mm = 340;
    let mut expected = None;
    for rotation in [Rotation::Portrait, Rotation::PortraitFlipped] {
        l.outputs[1].rotation = Some(rotation);
        l.outputs[1].resolution = Resolution {
            width: 1080,
            height: 1920,
        };
        for scale in [100, 125, 200, 300] {
            l.outputs[1].scale_percent = Some(scale);
            let m = Mapping::build(&l, &c);
            let p = m
                .map_motion(
                    Point { x: 1910, y: 540 },
                    Point { x: 1930, y: 540 },
                    context(),
                )
                .unwrap();
            assert_eq!(p.y, 544);
            if let Some(expected) = expected {
                assert_eq!(p, expected);
            }
            expected = Some(p);
        }
    }
    for rotation in [Rotation::Landscape, Rotation::LandscapeFlipped] {
        l.outputs[1].rotation = Some(rotation);
        l.outputs[1].resolution = Resolution {
            width: 1920,
            height: 1080,
        };
        let m = Mapping::build(&l, &c);
        assert_eq!(
            m.map_motion(
                Point { x: 1910, y: 540 },
                Point { x: 1930, y: 540 },
                context()
            )
            .unwrap()
            .y,
            540
        );
    }
}
#[test]
fn fast_motion_can_cross_more_than_one_calibrated_boundary() {
    let l = Layout {
        outputs: vec![output(1, 0, 0), output(2, 1920, 0), output(3, 3840, 0)],
    };
    let c = l
        .outputs
        .iter()
        .enumerate()
        .map(|(i, o)| calibration(o, 600, 340, i as i32 * 600, 0))
        .collect::<Vec<_>>();
    let m = Mapping::build(&l, &c);
    let p = m
        .map_motion(
            Point { x: 1900, y: 270 },
            Point { x: 4000, y: 270 },
            context(),
        )
        .unwrap();
    assert!((4000..4010).contains(&p.x));
    assert_eq!(p.y, 270);
}
#[test]
fn bypass_conditions_and_invalid_calibration_pass_native_input() {
    let (l, c) = pair();
    let m = Mapping::build(&l, &c);
    for i in 0..4 {
        let mut ctx = context();
        match i {
            0 => ctx.injected = true,
            1 => ctx.control_down = true,
            2 => ctx.confined = true,
            _ => ctx.input_desktop_available = false,
        };
        assert!(m
            .map_motion(Point { x: 1910, y: 270 }, Point { x: 1930, y: 270 }, ctx)
            .is_none());
    }
    for invalid in 0..4 {
        let mut c = c.clone();
        match invalid {
            0 => c[0].width_mm = 0,
            1 => c[0].display_key = "invalid".into(),
            2 => c[1].position_mm.x = 500,
            _ => c[1].position_mm.x = 900,
        };
        let m = Mapping::build(&l, &c);
        assert!(m
            .map_motion(
                Point { x: 1910, y: 270 },
                Point { x: 1930, y: 270 },
                context()
            )
            .is_none());
    }
}
#[test]
fn clone_representative_defaults_to_stable_order_and_can_be_selected() {
    let mut a = output(1, 0, 0);
    a.clone_group = Some("pair".into());
    let mut b = output(2, 0, 0);
    b.clone_group = a.clone_group.clone();
    b.primary = true;
    let d = output(3, 1920, 0);
    let l = Layout {
        outputs: vec![b.clone(), d.clone(), a.clone()],
    };
    let mut c = vec![
        calibration(&a, 600, 340, 0, 0),
        calibration(&b, 300, 170, 0, 0),
        calibration(&d, 600, 340, 600, 0),
    ];
    let m = Mapping::build(&l, &c);
    assert_eq!(m.surfaces.len(), 2);
    assert_eq!(m.surfaces[0].display_id, a.display_id);
    c[1].clone_representative = true;
    c[2].position_mm.x = 300;
    let m = Mapping::build(&l, &c);
    assert_eq!(m.surfaces.len(), 2);
    assert_eq!(m.surfaces[0].display_id, b.display_id);
}
#[test]
fn calibration_follows_serial_identity_after_dock_or_adapter_change() {
    let (mut l, c) = pair();
    l.outputs[1].display_id.adapter_luid = 9;
    l.outputs[1].display_id.target_id = 8;
    l.outputs[1].display_id.edid_hash = Some(88);
    l.outputs[1].display_id.identity.device_path = Some("newport".into());
    let m = Mapping::build(&l, &c);
    assert_eq!(m.surfaces.len(), 2);
    assert_eq!(m.surfaces[1].display_id, l.outputs[1].display_id);
    l.outputs[1].display_id.identity.edid_serial = Some("replacement".into());
    assert_eq!(Mapping::build(&l, &c).surfaces.len(), 1);
}
#[test]
fn global_calibration_is_independent_of_profile_apply_and_defaults_off() {
    let (l, c) = pair();
    let backend = MockBackend::new(vec![], l.clone()).unwrap();
    let mut manager = MonarchDisplayManager::new(backend, MemoryConfigStore::default()).unwrap();
    assert!(!manager.settings().cursor_correction_enabled);
    let mut settings = manager.settings().clone();
    settings.cursor_correction_enabled = true;
    settings.cursor_calibrations = c;
    manager.update_settings(settings.clone()).unwrap();
    manager.save_profile("Desk").unwrap();
    manager.toggle_display(&l.outputs[1].display_id).unwrap();
    manager.confirm_current_layout().unwrap();
    manager.apply_profile("Desk").unwrap();
    assert_eq!(manager.settings(), &settings);
}
