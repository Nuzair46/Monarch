#![cfg(target_os = "windows")]

use std::collections::{HashMap, HashSet};
use std::fs;
use std::mem::{size_of, MaybeUninit};
use std::path::PathBuf;
use std::sync::Mutex;

use monarch::{DisplayBackend, DisplayId, DisplayInfo, Layout, ManagerError};
use serde::{Deserialize, Serialize};

use super::apply::{
    active_color_state_signature, apply_layout_against_snapshot, capture_sdr_gamma_ramps,
    gamma_ramp_looks_identity, reapply_color_calibration_for_active_with_cached_sdr, GammaRampKey,
    GammaRampWords,
};
use super::enumerate::{query_active_topology, query_connected_topology};
use super::recovery::recover_layout;
use super::win32_types::{RawTopologySnapshot, TopologySnapshot};

const PERSISTED_RAW_SNAPSHOT_VERSION: u32 = 1;

#[derive(Debug, Serialize, Deserialize)]
struct PersistedRawSnapshot {
    version: u32,
    path_struct_size: usize,
    mode_struct_size: usize,
    paths: Vec<Vec<u8>>,
    modes: Vec<Vec<u8>>,
}

#[derive(Default)]
struct BackendCache {
    last_snapshot: Option<TopologySnapshot>,
    last_layout: Option<Layout>,
    last_displays: Vec<DisplayInfo>,
    sdr_gamma_cache: HashMap<GammaRampKey, GammaRampWords>,
}

#[derive(Default)]
pub struct WindowsDisplayBackend {
    cache: Mutex<BackendCache>,
}

impl WindowsDisplayBackend {
    pub fn new() -> Result<Self, ManagerError> {
        let backend = Self::default();
        let snapshot = {
            let mut fresh = query_connected_topology()?;
            // Keep the complete logical inventory, but never cache QDC_ALL_PATHS alternatives
            // as an apply-ready snapshot. Only its currently active routes have valid modes.
            fresh.raw.paths.retain(|path| path.flags & 1 != 0);
            if let Some(persisted_raw) = load_persisted_raw_snapshot() {
                merge_persisted_raw_for_fresh(fresh, &persisted_raw)
            } else {
                fresh
            }
        };
        let initial_sdr_ramps = capture_sdr_gamma_ramps(&snapshot);
        let raw_to_persist = snapshot.raw.clone();
        let mut cache = backend
            .cache
            .lock()
            .map_err(|_| ManagerError::Backend("windows backend cache poisoned".to_string()))?;
        cache.last_layout = Some(snapshot.layout.clone());
        cache.last_displays = snapshot.displays.clone();
        cache.last_snapshot = Some(snapshot);
        merge_sdr_gamma_cache(&mut cache.sdr_gamma_cache, initial_sdr_ramps);
        drop(cache);
        best_effort_persist_raw_snapshot(&raw_to_persist);
        Ok(backend)
    }

    fn refresh_active(&self) -> Result<(), ManagerError> {
        let snapshot = query_active_topology()?;
        let mut cache = self
            .cache
            .lock()
            .map_err(|_| ManagerError::Backend("windows backend cache poisoned".to_string()))?;

        cache.last_snapshot = Some(merge_snapshot_for_cache(
            cache.last_snapshot.as_ref(),
            snapshot.clone(),
        ));

        let mut merged_layout = cache
            .last_layout
            .clone()
            .unwrap_or_else(|| snapshot.layout.clone());
        for output in &mut merged_layout.outputs {
            if let Some(active) = snapshot
                .layout
                .outputs
                .iter()
                .find(|active| active.display_id == output.display_id)
            {
                *output = active.clone();
            } else {
                output.enabled = false;
                output.primary = false;
            }
        }
        for active in &snapshot.layout.outputs {
            if !merged_layout
                .outputs
                .iter()
                .any(|o| o.display_id == active.display_id)
            {
                merged_layout.outputs.push(active.clone());
            }
        }
        if !merged_layout.outputs.iter().any(|o| o.enabled && o.primary) {
            if let Some(first) = merged_layout.outputs.iter_mut().find(|o| o.enabled) {
                first.primary = true;
            }
        }
        cache.last_layout = Some(merged_layout);

        for display in &mut cache.last_displays {
            if let Some(active) = snapshot.displays.iter().find(|d| d.id == display.id) {
                *display = active.clone();
            } else {
                display.is_active = false;
                display.is_primary = false;
            }
        }
        for active in &snapshot.displays {
            if !cache.last_displays.iter().any(|d| d.id == active.id) {
                cache.last_displays.push(active.clone());
            }
        }
        cache.last_displays.sort_by(|a, b| {
            a.friendly_name
                .cmp(&b.friendly_name)
                .then(a.id.target_id.cmp(&b.id.target_id))
        });
        Ok(())
    }

    pub fn reapply_color_calibration(&self) -> Result<(), ManagerError> {
        let cached_sdr = {
            let cache = self
                .cache
                .lock()
                .map_err(|_| ManagerError::Backend("windows backend cache poisoned".to_string()))?;
            cache.sdr_gamma_cache.clone()
        };

        reapply_color_calibration_for_active_with_cached_sdr(&cached_sdr)?;
        let refreshed_snapshot = query_active_topology()?;

        let mut cache = self
            .cache
            .lock()
            .map_err(|_| ManagerError::Backend("windows backend cache poisoned".to_string()))?;
        merge_sdr_gamma_cache(
            &mut cache.sdr_gamma_cache,
            capture_sdr_gamma_ramps(&refreshed_snapshot),
        );
        Ok(())
    }

    pub fn color_state_signature(&self) -> Result<Option<String>, ManagerError> {
        let snapshot = query_active_topology()?;
        Ok(Some(active_color_state_signature(&snapshot)))
    }
}

fn merge_snapshot_for_cache(
    previous: Option<&TopologySnapshot>,
    fresh: TopologySnapshot,
) -> TopologySnapshot {
    let Some(previous) = previous else {
        return fresh;
    };

    // Preserve an older raw snapshot when it still covers the currently active outputs and
    // contains more paths. This keeps a recently-detached display path available for re-attach.
    if previous.raw.paths.len() > fresh.raw.paths.len()
        && raw_covers_active_outputs_raw(&previous.raw, &fresh.layout)
    {
        let mut merged = fresh;
        merged.raw = previous.raw.clone();
        return merged;
    }

    fresh
}

fn merge_persisted_raw_for_fresh(
    fresh: TopologySnapshot,
    persisted_raw: &RawTopologySnapshot,
) -> TopologySnapshot {
    if persisted_raw.paths.len() <= fresh.raw.paths.len() {
        return fresh;
    }
    if !raw_covers_active_outputs_raw(persisted_raw, &fresh.layout) {
        return fresh;
    }

    let mut merged = fresh;
    merged.raw = persisted_raw.clone();
    merged
}

fn raw_covers_active_outputs_raw(raw: &RawTopologySnapshot, layout: &Layout) -> bool {
    layout
        .outputs
        .iter()
        .filter(|output| output.enabled)
        .all(|output| {
            raw.paths.iter().any(|path| {
                let adapter_luid = ((path.targetInfo.adapterId.HighPart as i64 as u64) << 32)
                    | (path.targetInfo.adapterId.LowPart as u64);
                adapter_luid == output.display_id.adapter_luid
                    && path.targetInfo.id == output.display_id.target_id
            })
        })
}

impl DisplayBackend for WindowsDisplayBackend {
    fn list_displays(&self) -> Result<Vec<DisplayInfo>, ManagerError> {
        self.refresh_active()?;
        let cache = self
            .cache
            .lock()
            .map_err(|_| ManagerError::Backend("windows backend cache poisoned".to_string()))?;
        Ok(cache.last_displays.clone())
    }

    fn get_layout(&self) -> Result<Layout, ManagerError> {
        self.refresh_active()?;
        let cache = self
            .cache
            .lock()
            .map_err(|_| ManagerError::Backend("windows backend cache poisoned".to_string()))?;
        cache
            .last_layout
            .clone()
            .ok_or_else(|| ManagerError::Backend("no cached layout available".to_string()))
    }

    fn apply_layout(&self, layout: Layout) -> Result<(), ManagerError> {
        layout.ensure_valid()?;

        // Re-query the currently active topology so detach-only operations use a minimal base.
        // This reduces the chance of Windows re-touching unrelated outputs.
        let active_snapshot = query_active_topology()?;
        let needs_attach_paths = desired_enables_inactive_output(&layout, &active_snapshot.layout);

        let base_snapshot = {
            let cache = self
                .cache
                .lock()
                .map_err(|_| ManagerError::Backend("windows backend cache poisoned".to_string()))?;

            if !needs_attach_paths {
                active_snapshot.clone()
            } else if let Some(cached) = cache.last_snapshot.clone() {
                if raw_covers_active_outputs_raw(&cached.raw, &active_snapshot.layout) {
                    cached
                } else {
                    active_snapshot.clone()
                }
            } else {
                active_snapshot.clone()
            }
        };

        let working_layout = remap_layout_display_ids_for_snapshot(&layout, &base_snapshot.layout);
        let result = match apply_layout_against_snapshot(&working_layout, &base_snapshot) {
            Ok(snapshot) => Ok((snapshot, working_layout)),
            Err(initial_error) => reconnect_and_apply(&layout).map_err(|recovery_error| {
                ManagerError::Backend(format!(
                    "{initial_error}; reconnect failed: {recovery_error}"
                ))
            }),
        };
        let (next_snapshot, applied_layout) = match result {
            Ok(result) => result,
            Err(error) => {
                // The manager's confirmation timer starts only on success. A failed or
                // partial apply must restore the captured working desktop here instead.
                let rollback =
                    apply_layout_against_snapshot(&active_snapshot.layout, &active_snapshot)
                        .or_else(|_| {
                            reconnect_and_apply(&active_snapshot.layout)
                                .map(|(snapshot, _)| snapshot)
                        });
                let _ = self.refresh_active();
                return Err(ManagerError::Backend(match rollback {
                    Ok(_) => format!("{error}. The previous display layout was restored."),
                    Err(rollback_error) => format!("{error}. Restoring the previous layout also failed: {rollback_error}. Use Windows Display Settings to extend the disconnected displays."),
                }));
            }
        };
        let mut cache = self
            .cache
            .lock()
            .map_err(|_| ManagerError::Backend("windows backend cache poisoned".to_string()))?;
        let merged_snapshot =
            merge_snapshot_for_cache(cache.last_snapshot.as_ref(), next_snapshot.clone());
        let raw_to_persist = merged_snapshot.raw.clone();
        cache.last_snapshot = Some(merged_snapshot);
        merge_sdr_gamma_cache(
            &mut cache.sdr_gamma_cache,
            capture_sdr_gamma_ramps(&next_snapshot),
        );

        let mut merged_layout = applied_layout;
        for output in &mut merged_layout.outputs {
            if let Some(active) = next_snapshot
                .layout
                .outputs
                .iter()
                .find(|active| active.display_id == output.display_id)
            {
                output.position = active.position.clone();
                output.resolution = active.resolution.clone();
                output.refresh_rate_mhz = active.refresh_rate_mhz;
                output.enabled = true;
                output.primary = active.primary;
            }
        }
        cache.last_layout = Some(merged_layout);

        let mut displays = cache.last_displays.clone();
        for display in &mut displays {
            if let Some(active) = next_snapshot.displays.iter().find(|d| d.id == display.id) {
                *display = active.clone();
            } else {
                display.is_active = false;
                display.is_primary = false;
            }
        }
        for active in &next_snapshot.displays {
            if !displays.iter().any(|d| d.id == active.id) {
                displays.push(active.clone());
            }
        }
        cache.last_displays = displays;
        drop(cache);
        best_effort_persist_raw_snapshot(&raw_to_persist);

        Ok(())
    }

    fn color_state_signature(&self) -> Result<Option<String>, ManagerError> {
        WindowsDisplayBackend::color_state_signature(self)
    }

    fn reapply_color_calibration(&self) -> Result<(), ManagerError> {
        WindowsDisplayBackend::reapply_color_calibration(self)
    }
}

fn merge_sdr_gamma_cache(
    cache: &mut HashMap<GammaRampKey, GammaRampWords>,
    observed: HashMap<GammaRampKey, GammaRampWords>,
) {
    for (key, ramp) in observed {
        match cache.get(&key) {
            // Preserve a previous non-identity SDR ramp if the newly observed ramp looks like a
            // reset/default ramp (common after HDR transitions on some drivers).
            Some(existing)
                if !gamma_ramp_looks_identity(existing) && gamma_ramp_looks_identity(&ramp) => {}
            _ => {
                cache.insert(key, ramp);
            }
        }
    }
}

fn desired_enables_inactive_output(desired: &Layout, active_layout: &Layout) -> bool {
    desired.outputs.iter().any(|output| {
        output.enabled
            && !active_layout
                .outputs
                .iter()
                .any(|active| active.enabled && active.display_id == output.display_id)
    })
}

fn remap_layout_display_ids_for_snapshot(desired: &Layout, current: &Layout) -> Layout {
    let current_ids: HashSet<DisplayId> = current
        .outputs
        .iter()
        .map(|output| output.display_id.clone())
        .collect();

    if desired
        .outputs
        .iter()
        .all(|output| current_ids.contains(&output.display_id))
    {
        return desired.clone();
    }

    let mut remapped = desired.clone();
    let mut used: HashSet<DisplayId> = HashSet::new();
    for output in &remapped.outputs {
        if current_ids.contains(&output.display_id) {
            used.insert(output.display_id.clone());
        }
    }

    let mut current_by_edid: HashMap<u64, Vec<&monarch::OutputConfig>> = HashMap::new();
    for output in &current.outputs {
        if let Some(edid_hash) = output.display_id.edid_hash {
            current_by_edid.entry(edid_hash).or_default().push(output);
        }
    }

    for output in &mut remapped.outputs {
        if current_ids.contains(&output.display_id) {
            continue;
        }

        let mut replacement = None;

        if let Some(edid_hash) = output.display_id.edid_hash {
            let candidates = unique_unused_candidates(
                current_by_edid.get(&edid_hash).cloned().unwrap_or_default(),
                &used,
            );
            if candidates.len() == 1 {
                replacement = Some(candidates[0].display_id.clone());
            }
        }

        if replacement.is_none() {
            let candidates = unique_unused_candidates_by_target_id(
                output.display_id.target_id,
                &current.outputs,
                &used,
            );
            if candidates.len() == 1 {
                replacement = Some(candidates[0].display_id.clone());
            }
        }

        if let Some(next_id) = replacement {
            used.insert(next_id.clone());
            output.display_id = next_id;
        }
    }

    remapped
}

fn unique_unused_candidates<'a>(
    candidates: Vec<&'a monarch::OutputConfig>,
    used: &HashSet<DisplayId>,
) -> Vec<&'a monarch::OutputConfig> {
    candidates
        .into_iter()
        .filter(|candidate| !used.contains(&candidate.display_id))
        .collect()
}

fn unique_unused_candidates_by_target_id<'a>(
    target_id: u32,
    current_outputs: &'a [monarch::OutputConfig],
    used: &HashSet<DisplayId>,
) -> Vec<&'a monarch::OutputConfig> {
    current_outputs
        .iter()
        .filter(|candidate| candidate.display_id.target_id == target_id)
        .filter(|candidate| !used.contains(&candidate.display_id))
        .collect()
}

fn reconnect_and_apply(desired: &Layout) -> Result<(TopologySnapshot, Layout), ManagerError> {
    let connected = query_connected_topology()?;
    let desired = remap_layout_display_ids_for_snapshot(desired, &connected.layout);
    let active = recover_layout(&desired, &connected)?;
    // The topology-only activation creates fresh source/target modes. Now restore the
    // requested source resolution, primary display and placement using those modes.
    let desired = remap_layout_display_ids_for_snapshot(&desired, &active.layout);
    let snapshot = apply_layout_against_snapshot(&desired, &active)?;
    Ok((snapshot, desired))
}

#[cfg(test)]
mod hardware_tests {
    use super::*;

    struct RestoreOnDrop(TopologySnapshot);

    impl Drop for RestoreOnDrop {
        fn drop(&mut self) {
            if let Err(error) = apply_layout_against_snapshot(&self.0.layout, &self.0)
                .or_else(|_| reconnect_and_apply(&self.0.layout).map(|(snapshot, _)| snapshot))
            {
                eprintln!("Hardware test cleanup failed: {error}");
            }
        }
    }

    #[test]
    #[ignore = "Changes real displays. Run explicitly on a local console with at least three active monitors."]
    fn restores_two_detached_monitors_after_cold_start_with_incomplete_cache() {
        let original = query_active_topology().expect("local console display inventory");
        assert!(original.layout.enabled_output_count() >= 3);
        let _restore = RestoreOnDrop(original.clone());
        let backend = WindowsDisplayBackend::new().unwrap();
        let targets: Vec<_> = original
            .layout
            .outputs
            .iter()
            .filter(|o| o.enabled && !o.primary)
            .take(2)
            .map(|o| o.display_id.clone())
            .collect();
        let mut reduced = original.layout.clone();
        // Two consecutive detaches used to discard paths from the richer cache.
        for target in targets {
            reduced
                .outputs
                .iter_mut()
                .find(|o| o.display_id == target)
                .unwrap()
                .enabled = false;
            backend.apply_layout(reduced.clone()).unwrap();
        }
        drop(backend);

        // Simulate restarting with only the reduced raw snapshot left on disk.
        // Startup must still enumerate physically connected but inactive displays.
        let backend = WindowsDisplayBackend::new().unwrap();
        let inventory = backend.list_displays().unwrap();
        assert!(original
            .layout
            .outputs
            .iter()
            .all(|o| inventory.iter().any(|d| d.id == o.display_id)));
        backend.cache.lock().unwrap().last_snapshot = Some(query_active_topology().unwrap());
        backend.apply_layout(original.layout.clone()).unwrap();
        let restored = query_active_topology().unwrap();
        super::super::apply::verify_requested_outputs(&original.layout, &restored.layout).unwrap();
        for expected in &original.layout.outputs {
            let actual = restored
                .layout
                .outputs
                .iter()
                .find(|o| o.display_id == expected.display_id)
                .unwrap();
            assert_eq!(actual.position, expected.position);
            assert_eq!(actual.resolution, expected.resolution);
            assert_eq!(actual.primary, expected.primary);
        }
        println!(
            "Restored all {} active monitors after sequential detaches and a cold cache.",
            restored.layout.enabled_output_count()
        );
    }
}

fn best_effort_persist_raw_snapshot(raw: &RawTopologySnapshot) {
    if let Err(err) = persist_raw_snapshot(raw) {
        eprintln!("Monarch persisted topology snapshot write failed: {err}");
    }
}

fn persist_raw_snapshot(raw: &RawTopologySnapshot) -> Result<(), ManagerError> {
    let payload = PersistedRawSnapshot {
        version: PERSISTED_RAW_SNAPSHOT_VERSION,
        path_struct_size: size_of::<windows::Win32::Devices::Display::DISPLAYCONFIG_PATH_INFO>(),
        mode_struct_size: size_of::<windows::Win32::Devices::Display::DISPLAYCONFIG_MODE_INFO>(),
        paths: raw.paths.iter().map(struct_to_bytes).collect(),
        modes: raw.modes.iter().map(struct_to_bytes).collect(),
    };

    let path = persisted_raw_snapshot_path();
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|err| {
            ManagerError::Backend(format!(
                "failed to create persisted snapshot directory: {err}"
            ))
        })?;
    }

    let body = serde_json::to_vec(&payload)
        .map_err(|err| ManagerError::Backend(format!("failed to encode snapshot: {err}")))?;
    fs::write(&path, body)
        .map_err(|err| ManagerError::Backend(format!("failed to write snapshot: {err}")))?;
    Ok(())
}

fn load_persisted_raw_snapshot() -> Option<RawTopologySnapshot> {
    let path = persisted_raw_snapshot_path();
    let body = fs::read(path).ok()?;
    let payload: PersistedRawSnapshot = serde_json::from_slice(&body).ok()?;
    if payload.version != PERSISTED_RAW_SNAPSHOT_VERSION {
        return None;
    }
    if payload.path_struct_size
        != size_of::<windows::Win32::Devices::Display::DISPLAYCONFIG_PATH_INFO>()
        || payload.mode_struct_size
            != size_of::<windows::Win32::Devices::Display::DISPLAYCONFIG_MODE_INFO>()
    {
        return None;
    }

    let mut paths = Vec::with_capacity(payload.paths.len());
    for bytes in payload.paths {
        paths.push(struct_from_bytes::<
            windows::Win32::Devices::Display::DISPLAYCONFIG_PATH_INFO,
        >(&bytes)?);
    }

    let mut modes = Vec::with_capacity(payload.modes.len());
    for bytes in payload.modes {
        modes.push(struct_from_bytes::<
            windows::Win32::Devices::Display::DISPLAYCONFIG_MODE_INFO,
        >(&bytes)?);
    }

    Some(RawTopologySnapshot { paths, modes })
}

fn persisted_raw_snapshot_path() -> PathBuf {
    let config_path = monarch::FileConfigStore::default_config_path();
    config_path
        .parent()
        .map(|parent| parent.join("topology_snapshot.json"))
        .unwrap_or_else(|| PathBuf::from("topology_snapshot.json"))
}

fn struct_to_bytes<T>(value: &T) -> Vec<u8> {
    unsafe { std::slice::from_raw_parts((value as *const T).cast::<u8>(), size_of::<T>()).to_vec() }
}

fn struct_from_bytes<T>(bytes: &[u8]) -> Option<T> {
    if bytes.len() != size_of::<T>() {
        return None;
    }

    let mut value = MaybeUninit::<T>::uninit();
    unsafe {
        std::ptr::copy_nonoverlapping(bytes.as_ptr(), value.as_mut_ptr().cast::<u8>(), bytes.len());
        Some(value.assume_init())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use monarch::{OutputConfig, Position, Resolution};
    use windows::Win32::Devices::Display::{DISPLAYCONFIG_MODE_INFO, DISPLAYCONFIG_PATH_INFO};

    fn output(adapter: u64, target: u32, enabled: bool) -> OutputConfig {
        OutputConfig {
            display_id: DisplayId {
                adapter_luid: adapter,
                target_id: target,
                edid_hash: Some(10_000 + target as u64),
            },
            enabled,
            position: Position {
                x: (target as i32 - 1) * 1920,
                y: 0,
            },
            resolution: Resolution {
                width: 1920,
                height: 1080,
            },
            refresh_rate_mhz: 60_000,
            primary: target == 1 && enabled,
        }
    }

    fn snapshot(adapter: u64, targets: &[u32]) -> TopologySnapshot {
        let mut paths = Vec::new();
        let mut modes = Vec::new();
        for (source, target) in targets.iter().copied().enumerate() {
            let mut path = DISPLAYCONFIG_PATH_INFO::default();
            path.sourceInfo.adapterId.HighPart = (adapter >> 32) as i32;
            path.sourceInfo.adapterId.LowPart = adapter as u32;
            path.sourceInfo.id = source as u32;
            path.sourceInfo.Anonymous.modeInfoIdx = modes.len() as u32;
            path.targetInfo.adapterId = path.sourceInfo.adapterId;
            path.targetInfo.id = target;
            path.targetInfo.Anonymous.modeInfoIdx = modes.len() as u32 + 1;
            path.flags = 1;
            paths.push(path);
            for id in [source as u32, target] {
                let mut mode = DISPLAYCONFIG_MODE_INFO::default();
                mode.id = id;
                mode.adapterId = path.targetInfo.adapterId;
                modes.push(mode);
            }
        }
        TopologySnapshot {
            raw: RawTopologySnapshot { paths, modes },
            layout: Layout {
                outputs: targets
                    .iter()
                    .map(|target| output(adapter, *target, true))
                    .collect(),
            },
            displays: Vec::new(),
        }
    }

    fn raw_targets(snapshot: &TopologySnapshot) -> Vec<u32> {
        snapshot
            .raw
            .paths
            .iter()
            .map(|path| path.targetInfo.id)
            .collect()
    }

    #[test]
    fn cached_four_monitor_paths_survive_two_sequential_detaches() {
        let original = snapshot(1, &[1, 2, 3, 4]);
        let after_first_detach = merge_snapshot_for_cache(Some(&original), snapshot(1, &[1, 2, 3]));
        let after_second_detach =
            merge_snapshot_for_cache(Some(&after_first_detach), snapshot(1, &[1, 2]));

        assert_eq!(after_first_detach.layout.enabled_output_count(), 3);
        assert_eq!(after_second_detach.layout.enabled_output_count(), 2);
        assert_eq!(raw_targets(&after_second_detach), vec![1, 2, 3, 4]);
        assert_eq!(
            after_second_detach.raw.modes.len(),
            original.raw.modes.len()
        );
        for (previous, retained) in original
            .raw
            .paths
            .iter()
            .zip(&after_second_detach.raw.paths)
        {
            assert_eq!(previous.sourceInfo.id, retained.sourceInfo.id);
            assert_eq!(
                unsafe { previous.sourceInfo.Anonymous.modeInfoIdx },
                unsafe { retained.sourceInfo.Anonymous.modeInfoIdx },
            );
            assert_eq!(
                unsafe { previous.targetInfo.Anonymous.modeInfoIdx },
                unsafe { retained.targetInfo.Anonymous.modeInfoIdx },
            );
        }
        assert!(raw_covers_active_outputs_raw(
            &after_second_detach.raw,
            &original.layout
        ));
    }

    #[test]
    fn cached_paths_are_replaced_when_the_active_adapter_changes() {
        let original = snapshot(1, &[1, 2, 3, 4]);
        let fresh = snapshot(0xfedc_ba98_0000_0009, &[1, 2]);
        let expected_layout = fresh.layout.clone();

        let merged = merge_snapshot_for_cache(Some(&original), fresh);

        assert_eq!(raw_targets(&merged), vec![1, 2]);
        assert!(raw_covers_active_outputs_raw(&merged.raw, &expected_layout));
        assert!(!raw_covers_active_outputs_raw(
            &merged.raw,
            &original.layout
        ));
    }

    #[test]
    fn persisted_paths_restore_attach_routes_without_enabling_inactive_inventory() {
        let persisted = snapshot(1, &[1, 2, 3, 4]);
        let mut fresh = snapshot(1, &[1, 2]);
        fresh
            .layout
            .outputs
            .extend([output(1, 3, false), output(1, 4, false)]);

        let merged = merge_persisted_raw_for_fresh(fresh, &persisted.raw);

        assert_eq!(merged.layout.outputs.len(), 4);
        assert_eq!(merged.layout.enabled_output_count(), 2);
        assert_eq!(raw_targets(&merged), vec![1, 2, 3, 4]);
        assert!(raw_covers_active_outputs_raw(
            &merged.raw,
            &persisted.layout
        ));
    }

    #[test]
    fn saved_profile_remaps_connected_inactive_targets_after_adapter_change() {
        let desired = Layout {
            outputs: (1..=4).map(|target| output(1, target, true)).collect(),
        };
        let mut connected = desired.clone();
        for output in &mut connected.outputs {
            output.display_id.adapter_luid = 0xfedc_ba98_0000_0009;
            output.display_id.target_id += 100;
            output.enabled = output.display_id.target_id <= 102;
        }

        let remapped = remap_layout_display_ids_for_snapshot(&desired, &connected);

        assert_eq!(remapped.enabled_output_count(), 4);
        assert!(desired_enables_inactive_output(&remapped, &connected));
        for ((saved, fresh), actual) in desired
            .outputs
            .iter()
            .zip(&connected.outputs)
            .zip(&remapped.outputs)
        {
            assert_eq!(actual.display_id, fresh.display_id);
            let mut expected = saved.clone();
            expected.display_id = fresh.display_id.clone();
            assert_eq!(actual, &expected);
        }
    }
}
