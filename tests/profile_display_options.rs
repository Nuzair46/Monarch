use monarch::*;

fn layout() -> Layout {
    Layout {
        outputs: (1..=3)
            .map(|target_id| OutputConfig {
                display_id: DisplayId {
                    adapter_luid: 1,
                    target_id,
                    edid_hash: Some(u64::from(target_id)),
                    identity: Default::default(),
                },
                enabled: true,
                primary: target_id == 1,
                position: Position {
                    x: (target_id as i32 - 1) * 1920,
                    y: 0,
                },
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

fn manager(
    current: Layout,
    saved: Layout,
    ignore_missing_displays: bool,
) -> (
    MonarchDisplayManager<MockBackend, MemoryConfigStore>,
    MockBackend,
    MemoryConfigStore,
) {
    let backend = MockBackend::new(Vec::new(), current).unwrap();
    let store = MemoryConfigStore::new(AppConfig {
        profiles: vec![Profile {
            name: "Portable".into(),
            layout: saved,
            audio_output: None,
            display_options: ProfileDisplayOptions {
                ignore_missing_displays,
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

fn desk_layout() -> Layout {
    let mut saved = layout();
    saved.outputs[0].resolution = Resolution {
        width: 5120,
        height: 1440,
    };
    saved.outputs[1].position = Position { x: -197, y: 1445 };
    saved.outputs[2].position = Position { x: 1723, y: 1445 };
    saved.outputs[2].resolution = Resolution {
        width: 1690,
        height: 1056,
    };
    let mut portable = saved.outputs[2].clone();
    portable.display_id.target_id = 4;
    portable.display_id.edid_hash = Some(4);
    portable.position = Position { x: 3413, y: 1440 };
    portable.resolution = Resolution {
        width: 720,
        height: 1490,
    };
    portable.rotation = Some(Rotation::Portrait);
    saved.outputs.push(portable);
    saved
}

#[test]
fn missing_portable_bridge_closes_exact_five_pixel_desk_gap_as_a_group() {
    let saved = desk_layout();
    saved.ensure_supported().unwrap();
    let mut current = saved.clone();
    current.outputs.pop();
    let (mut m, backend, _) = manager(current.clone(), saved.clone(), true);
    m.apply_profile("Portable").unwrap();
    let applied = backend.current_layout().unwrap();
    assert_eq!(applied.outputs[0], saved.outputs[0]);
    for i in [1, 2] {
        assert_eq!(applied.outputs[i].position.x, saved.outputs[i].position.x);
        assert_eq!(applied.outputs[i].position.y, 1440);
        let mut expected = saved.outputs[i].clone();
        expected.position.y -= 5;
        assert_eq!(applied.outputs[i], expected);
    }
    assert_eq!(m.list_profiles()[0].layout, saved);
    m.rollback_pending().unwrap();
    assert_eq!(backend.current_layout().unwrap(), current);
    backend.apply_layout(saved.clone()).unwrap();
    m.apply_profile("Portable").unwrap();
    assert_eq!(backend.current_layout().unwrap(), saved);
    assert!(!m.has_pending_confirmation());
}

#[test]
fn missing_bridge_moves_clone_group_together_and_leaves_disabled_geometry_alone() {
    let mut saved = layout();
    saved.outputs[2].clone_group = Some("pair".into());
    let mut clone = saved.outputs[2].clone();
    clone.display_id.target_id = 4;
    clone.display_id.edid_hash = Some(4);
    saved.outputs.push(clone);
    let mut disabled = saved.outputs[0].clone();
    disabled.display_id.target_id = 5;
    disabled.display_id.edid_hash = Some(5);
    disabled.enabled = false;
    disabled.primary = false;
    disabled.position.x = -8000;
    saved.outputs.push(disabled.clone());
    let mut current = saved.clone();
    current.outputs.remove(1);
    let (mut m, backend, _) = manager(current, saved.clone(), true);
    m.apply_profile("Portable").unwrap();
    let applied = backend.current_layout().unwrap();
    assert_eq!(applied.outputs[1].position.x, 1920);
    assert_eq!(applied.outputs[1].position, applied.outputs[2].position);
    assert!(applied.outputs[1].shares_source(&applied.outputs[2]));
    assert_eq!(applied.outputs[3], disabled);
    assert_eq!(m.list_profiles()[0].layout, saved);
}

#[test]
fn no_repositioning_when_all_enabled_displays_are_present_or_missing_only_disabled() {
    for missing_disabled in [false, true] {
        let mut saved = desk_layout();
        saved.outputs[3].enabled = false;
        let mut current = saved.clone();
        if missing_disabled {
            current.outputs.pop();
        }
        let (mut m, backend, _) = manager(current, saved.clone(), true);
        m.apply_profile("Portable").unwrap();
        let applied = backend.current_layout().unwrap();
        for i in [0, 1, 2] {
            assert_eq!(applied.outputs[i], saved.outputs[i]);
        }
    }
}

#[test]
fn multiple_disconnected_groups_join_without_overlap_and_keep_primary_fixed() {
    let mut saved = layout();
    saved.outputs[2].position = Position { x: 0, y: 1080 };
    let mut upper = saved.outputs[0].clone();
    upper.display_id.target_id = 4;
    upper.display_id.edid_hash = Some(4);
    upper.primary = false;
    upper.position = Position { x: -2000, y: -1100 };
    saved.outputs.push(upper);
    let mut right = saved.outputs[1].clone();
    right.display_id.target_id = 5;
    right.display_id.edid_hash = Some(5);
    right.position = Position { x: 5000, y: 0 };
    saved.outputs.push(right);
    let mut current = saved.clone();
    current.outputs.remove(1);
    let (mut m, backend, _) = manager(current, saved.clone(), true);
    m.apply_profile("Portable").unwrap();
    let applied = backend.current_layout().unwrap();
    applied.ensure_supported().unwrap();
    assert_eq!(applied.outputs[0], saved.outputs[0]);
    assert_eq!(applied.outputs[1], saved.outputs[2]);
    let mut reached = vec![0];
    loop {
        let next = (0..applied.outputs.len()).find(|i| {
            !reached.contains(i)
                && reached.iter().any(|&j| {
                    let a = &applied.outputs[*i];
                    let b = &applied.outputs[j];
                    ((a.position.x + a.resolution.width as i32 == b.position.x
                        || b.position.x + b.resolution.width as i32 == a.position.x)
                        && a.position.y < b.position.y + b.resolution.height as i32
                        && b.position.y < a.position.y + a.resolution.height as i32)
                        || ((a.position.y + a.resolution.height as i32 == b.position.y
                            || b.position.y + b.resolution.height as i32 == a.position.y)
                            && a.position.x < b.position.x + b.resolution.width as i32
                            && b.position.x < a.position.x + a.resolution.width as i32)
                })
        });
        match next {
            Some(i) => reached.push(i),
            None => break,
        }
    }
    assert_eq!(reached.len(), applied.outputs.len());
    assert_eq!(m.list_profiles()[0].layout, saved);
}

#[test]
fn creation_saves_explicit_options_and_overwrite_can_disable_them() {
    let (mut m, backend, store) = manager(layout(), layout(), false);
    let options = ProfileDisplayOptions {
        ignore_missing_displays: true,
    };
    m.save_profile_with_options("New", None, Some(options))
        .unwrap();
    assert_eq!(backend.current_layout().unwrap(), layout());
    assert!(!m.has_pending_confirmation());
    let mut reloaded = MonarchDisplayManager::new(backend, store).unwrap();
    assert_eq!(
        reloaded
            .list_profiles()
            .iter()
            .find(|p| p.name == "New")
            .unwrap()
            .display_options,
        options
    );
    reloaded.save_profile_with_audio("New", None).unwrap();
    assert_eq!(
        reloaded
            .list_profiles()
            .iter()
            .find(|p| p.name == "New")
            .unwrap()
            .display_options,
        options
    );
    reloaded
        .save_profile_with_options("New", None, Some(Default::default()))
        .unwrap();
    assert_eq!(
        reloaded
            .list_profiles()
            .iter()
            .find(|p| p.name == "New")
            .unwrap()
            .display_options,
        Default::default()
    );
}

#[test]
fn legacy_profiles_default_to_strict_options() {
    let (m, _, _) = manager(layout(), layout(), false);
    let mut json = serde_json::to_value(&m.list_profiles()[0]).unwrap();
    json.as_object_mut().unwrap().remove("display_options");
    let restored: Profile = serde_json::from_value(json).unwrap();
    assert_eq!(restored.display_options, ProfileDisplayOptions::default());
}

#[test]
fn strict_profile_rejects_missing_enabled_display_without_changing_layout() {
    let mut current = layout();
    current.outputs.pop();
    let (mut m, backend, _) = manager(current.clone(), layout(), false);
    assert!(m
        .apply_profile("Portable")
        .unwrap_err()
        .to_string()
        .contains("not connected"));
    assert_eq!(backend.current_layout().unwrap(), current);
    assert!(!m.has_pending_confirmation());
}

#[test]
fn ignores_missing_display_without_rewriting_saved_profile_and_can_revert() {
    let mut current = layout();
    current.outputs.pop();
    let mut saved = layout();
    saved.outputs[1].refresh_rate_mhz = 75_000;
    let (mut m, backend, _) = manager(current.clone(), saved.clone(), true);
    m.apply_profile("Portable").unwrap();
    assert_eq!(backend.current_layout().unwrap().outputs.len(), 2);
    assert_eq!(
        backend.current_layout().unwrap().outputs[1].refresh_rate_mhz,
        75_000
    );
    assert_eq!(m.list_profiles()[0].layout, saved);
    m.rollback_pending().unwrap();
    assert_eq!(backend.current_layout().unwrap(), current);
    backend.apply_layout(layout()).unwrap();
    m.apply_profile("Portable").unwrap();
    assert_eq!(backend.current_layout().unwrap().enabled_output_count(), 3);
    m.confirm_current_layout().unwrap();
}

#[test]
fn missing_primary_rebases_remaining_display_and_clears_singleton_clone_group() {
    let mut saved = layout();
    saved.outputs.truncate(2);
    saved.outputs[0].clone_group = Some("pair".into());
    saved.outputs[1] = OutputConfig {
        display_id: saved.outputs[1].display_id.clone(),
        ..saved.outputs[0].clone()
    };
    let mut current = layout();
    current.outputs.remove(0);
    let (mut m, backend, _) = manager(current, saved, true);
    m.apply_profile("Portable").unwrap();
    let applied = backend.current_layout().unwrap();
    assert_eq!(applied.enabled_output_count(), 1);
    assert!(applied.outputs[0].primary);
    assert_eq!(applied.outputs[0].position, Position { x: 0, y: 0 });
    assert_eq!(applied.outputs[0].clone_group, None);
}

#[test]
fn missing_extended_primary_selects_remaining_profile_monitor_at_origin() {
    let saved = layout();
    let mut current = layout();
    current.outputs.remove(0);
    current.outputs[0].primary = true;
    current.outputs[0].position.x = 0;
    current.outputs[1].position.x = 1920;
    let (mut m, backend, _) = manager(current, saved, true);
    m.apply_profile("Portable").unwrap();
    let applied = backend.current_layout().unwrap();
    assert!(applied.outputs[0].primary);
    assert_eq!(applied.outputs[0].position.x, 0);
    assert_eq!(applied.outputs[1].position.x, 1920);
}

#[test]
fn connected_but_disabled_saved_display_is_enabled_not_skipped() {
    let mut current = layout();
    current.outputs[1].enabled = false;
    let (mut m, backend, _) = manager(current, layout(), true);
    m.apply_profile("Portable").unwrap();
    assert_eq!(backend.current_layout().unwrap(), layout());
}

#[test]
fn many_saved_monitors_cannot_map_to_one_live_target() {
    let mut current = layout();
    current.outputs.truncate(1);
    let mut saved = layout();
    saved.outputs.truncate(2);
    saved.outputs[1].display_id.edid_hash = saved.outputs[0].display_id.edid_hash;
    let (mut m, backend, _) = manager(current.clone(), saved, true);
    assert!(m
        .apply_profile("Portable")
        .unwrap_err()
        .to_string()
        .contains("same connected monitor"));
    assert_eq!(backend.current_layout().unwrap(), current);
}

#[test]
fn all_saved_enabled_displays_missing_is_an_error_even_with_extra_displays() {
    let mut saved = layout();
    saved.outputs.truncate(1);
    let mut current = layout();
    current.outputs.remove(0);
    current.outputs[0].primary = true;
    let (mut m, backend, _) = manager(current.clone(), saved, true);
    assert!(m
        .apply_profile("Portable")
        .unwrap_err()
        .to_string()
        .contains("no enabled profile displays"));
    assert_eq!(backend.current_layout().unwrap(), current);
    assert!(!m.has_pending_confirmation());
}

#[test]
fn unlisted_displays_are_not_preserved_with_either_missing_display_policy() {
    for ignore in [false, true] {
        let current = layout();
        let mut saved = current.clone();
        saved.outputs.truncate(1);
        saved.outputs[0].refresh_rate_mhz = 75_000;
        let (mut m, backend, _) = manager(current, saved, ignore);
        m.apply_profile("Portable").unwrap();
        let applied = backend.current_layout().unwrap();
        assert_eq!(applied.enabled_output_count(), 1);
    }
}

#[test]
fn explicitly_disabled_display_stays_disabled() {
    let mut current = layout();
    current.outputs[2].enabled = false;
    let mut saved = layout();
    saved.outputs.truncate(2);
    saved.outputs[1].enabled = false;
    let (mut m, backend, _) = manager(current, saved, true);
    m.apply_profile("Portable").unwrap();
    let applied = backend.current_layout().unwrap();
    assert!(applied.outputs[0].enabled);
    assert!(!applied.outputs[1].enabled);
    assert_eq!(applied.outputs.len(), 2);
}

#[test]
fn ambiguous_identity_is_never_skipped() {
    let mut saved = layout();
    saved.outputs.truncate(1);
    saved.outputs[0].display_id.adapter_luid = 9;
    let mut current = layout();
    current.outputs[1].display_id.edid_hash = current.outputs[0].display_id.edid_hash;
    let (mut m, backend, _) = manager(current.clone(), saved, true);
    assert!(m
        .apply_profile("Portable")
        .unwrap_err()
        .to_string()
        .contains("ambiguous"));
    assert_eq!(backend.current_layout().unwrap(), current);
}

#[test]
fn removed_option_loads_without_resetting_profiles_and_is_not_serialized() {
    for ignore in [false, true] {
        for removed in [false, true] {
            let (m, backend, _) = manager(layout(), layout(), ignore);
            let mut config = serde_json::to_value(AppConfig {
                profiles: m.list_profiles(),
                ..Default::default()
            })
            .unwrap();
            config["profiles"][0]["display_options"]["preserve_unlisted_displays"] =
                serde_json::json!(removed);
            let restored: AppConfig = serde_json::from_value(config).unwrap();
            let mut reloaded =
                MonarchDisplayManager::new(backend, MemoryConfigStore::new(restored)).unwrap();
            assert_eq!(
                reloaded.list_profiles()[0]
                    .display_options
                    .ignore_missing_displays,
                ignore
            );
            assert_eq!(reloaded.list_profiles()[0].layout, layout());
            reloaded.apply_profile("Portable").unwrap();
            let serialized = serde_json::to_value(&reloaded.list_profiles()[0]).unwrap();
            assert_eq!(
                serialized["display_options"],
                serde_json::json!({
                    "ignore_missing_displays": ignore,
                })
            );
        }
    }
    assert!(
        serde_json::from_value::<ProfileDisplayOptions>(serde_json::json!({
            "unknown_option": true,
        }))
        .is_err()
    );
}

#[test]
fn options_are_persisted_preserved_on_overwrite_and_locked_during_confirmation() {
    let (mut m, backend, store) = manager(layout(), layout(), false);
    let before = backend.current_layout().unwrap();
    let options = ProfileDisplayOptions {
        ignore_missing_displays: true,
    };
    m.set_profile_display_options("Portable", options).unwrap();
    assert_eq!(backend.current_layout().unwrap(), before);
    m.save_profile("Portable").unwrap();
    let mut reloaded = MonarchDisplayManager::new(backend.clone(), store).unwrap();
    assert_eq!(reloaded.list_profiles()[0].display_options, options);
    let mut changed = layout();
    changed.outputs[1].enabled = false;
    reloaded.apply_layout(changed).unwrap();
    assert!(reloaded
        .set_profile_display_options("Portable", Default::default())
        .is_err());
    assert_eq!(reloaded.list_profiles()[0].display_options, options);
}
