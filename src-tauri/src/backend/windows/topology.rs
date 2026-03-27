#![cfg(target_os = "windows")]

use std::collections::{HashMap, HashSet};
use std::fs;
use std::mem::{size_of, MaybeUninit};
use std::path::PathBuf;
use std::sync::Mutex;

use monarch::{DisplayBackend, DisplayId, DisplayInfo, Layout, ManagerError};
use serde::{Deserialize, Serialize};
use crate::diagnostics;

use super::apply::{
    active_color_state_signature, apply_layout_against_snapshot, capture_sdr_gamma_ramps,
    force_topology_extend, gamma_ramp_looks_identity,
    reapply_color_calibration_for_active_with_cached_sdr,
    GammaRampKey, GammaRampWords,
};
use super::enumerate::query_active_topology;
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
            let fresh = query_active_topology()?;
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
        diagnostics::log(format!(
            "topology_apply:start:outputs={}",
            layout.outputs.len()
        ));

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
        let (next_snapshot, applied_layout) =
            match apply_layout_against_snapshot(&working_layout, &base_snapshot) {
                Ok(snapshot) => (snapshot, working_layout),
                Err(error) if is_set_display_invalid_parameter(&error) => {
                    diagnostics::log("topology_apply:retry:reason=setdisplayconfig_87");
                    force_topology_extend()?;
                    std::thread::sleep(std::time::Duration::from_millis(700));
                    let recovered_snapshot = query_active_topology()?;
                    let retry_layout =
                        remap_layout_display_ids_for_snapshot(&working_layout, &recovered_snapshot.layout);
                    let snapshot = apply_layout_against_snapshot(&retry_layout, &recovered_snapshot)?;
                    (snapshot, retry_layout)
                }
                Err(error) => {
                    diagnostics::log(format!("topology_apply:error:{error}"));
                    return Err(error);
                }
            };
        let mut cache = self
            .cache
            .lock()
            .map_err(|_| ManagerError::Backend("windows backend cache poisoned".to_string()))?;
        let merged_snapshot = merge_snapshot_for_cache(
            Some(&base_snapshot),
            next_snapshot.clone(),
        );
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
        diagnostics::log("topology_apply:done");

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
            let candidates =
                unique_unused_candidates(current_by_edid.get(&edid_hash).cloned().unwrap_or_default(), &used);
            if candidates.len() == 1 {
                replacement = Some(candidates[0].display_id.clone());
            }
        }

        if replacement.is_none() {
            let candidates =
                unique_unused_candidates_by_target_id(output.display_id.target_id, &current.outputs, &used);
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

fn is_set_display_invalid_parameter(error: &ManagerError) -> bool {
    matches!(
        error,
        ManagerError::Backend(message) if message.contains("SetDisplayConfig failed: 87")
    )
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
            ManagerError::Backend(format!("failed to create persisted snapshot directory: {err}"))
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
        paths.push(struct_from_bytes::<windows::Win32::Devices::Display::DISPLAYCONFIG_PATH_INFO>(
            &bytes,
        )?);
    }

    let mut modes = Vec::with_capacity(payload.modes.len());
    for bytes in payload.modes {
        modes.push(struct_from_bytes::<windows::Win32::Devices::Display::DISPLAYCONFIG_MODE_INFO>(
            &bytes,
        )?);
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
        std::ptr::copy_nonoverlapping(
            bytes.as_ptr(),
            value.as_mut_ptr().cast::<u8>(),
            bytes.len(),
        );
        Some(value.assume_init())
    }
}
