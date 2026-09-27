#![cfg(target_os = "windows")]

use std::collections::{HashMap, HashSet};
use std::fs;
use std::mem::{size_of, MaybeUninit};
use std::path::PathBuf;
use std::sync::Mutex;

use crate::diagnostics;
use monarch::{DisplayBackend, DisplayId, DisplayInfo, Layout, ManagerError};
use serde::{Deserialize, Serialize};

use super::apply::{
    active_color_state_signature, apply_attach_paths, apply_layout_against_snapshot,
    build_attach_paths, capture_sdr_gamma_ramps, gamma_ramp_looks_identity,
    reapply_color_calibration_for_active_with_cached_sdr, run_display_switch_extend,
    try_topology_extend, validate_attach_paths, GammaRampKey, GammaRampWords,
};
use super::enumerate::{query_active_only_topology, query_active_topology, snapshot_from_raw};
use super::win32_types::{luid_to_u64, AttachablePath, RawTopologySnapshot, TopologySnapshot};

const PERSISTED_RAW_SNAPSHOT_VERSION: u32 = 1;
const DISPLAYCONFIG_PATH_ACTIVE_FLAG: u32 = 0x0000_0001;

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
    /// Set by `invalidate_cache`: the next refresh re-seeds detached-display knowledge from the
    /// persisted snapshot (when it still matches this boot), so an invalidation does not lose a
    /// detached display whenever QDC_DATABASE_CURRENT enrichment fails afterwards.
    reseed_persisted: bool,
}

#[derive(Default)]
pub struct WindowsDisplayBackend {
    cache: Mutex<BackendCache>,
}

impl WindowsDisplayBackend {
    pub fn new() -> Result<Self, ManagerError> {
        let backend = Self::default();
        let mut persist_now = true;
        let snapshot = {
            let fresh = query_active_topology()?;
            match load_persisted_raw_snapshot() {
                Some(persisted_raw) if persisted_raw.paths.len() > fresh.raw.paths.len() => {
                    match merge_persisted_raw_for_fresh(&fresh, &persisted_raw) {
                        Some(merged) => merged,
                        None => {
                            // Two very different causes land here, and only one justifies a
                            // rewrite:
                            //  (a) no connector in common -> adapter LUID churn across a reboot.
                            //      The file describes a boot that no longer exists and never will
                            //      again: a fossil. Overwrite it, or it blocks forever — the only
                            //      other writer is a successful apply, which a stale snapshot can
                            //      itself block.
                            //  (b) connectors still overlap -> the LUIDs are current and the
                            //      active set simply changed (e.g. the user attached something
                            //      from Windows Display settings). The persisted paths of the
                            //      OTHER connectors are still real and may be the last record of
                            //      a detached display, so keep them: a kept fossil is inert (this
                            //      session uses `fresh` and the merge re-rejects it), a destroyed
                            //      one never comes back.
                            let persisted_connectors = raw_path_connectors(&persisted_raw);
                            let fresh_connectors = raw_path_connectors(&fresh.raw);
                            persist_now = persisted_connectors.is_disjoint(&fresh_connectors);
                            diagnostics::log(if persist_now {
                                "topology_persist:replace:persisted_snapshot_from_another_boot"
                            } else {
                                "topology_persist:keep:persisted_snapshot_rejected_but_current"
                            });
                            fresh
                        }
                    }
                }
                _ => fresh,
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
        if persist_now {
            best_effort_persist_raw_snapshot(&raw_to_persist);
        }
        Ok(backend)
    }

    /// Drop every cached snapshot/layout/display entry (the SDR gamma cache is kept) so the next
    /// refresh rebuilds state from a fresh enriched enumeration. Used after system resume, when
    /// stale adapter LUIDs and transient EDID read failures would otherwise pollute the cache.
    pub fn invalidate_cache(&self) -> Result<(), ManagerError> {
        let mut cache = self
            .cache
            .lock()
            .map_err(|_| ManagerError::Backend("windows backend cache poisoned".to_string()))?;
        cache.last_snapshot = None;
        cache.last_layout = None;
        cache.last_displays.clear();
        cache.reseed_persisted = true;
        drop(cache);
        diagnostics::log("backend_cache:invalidated");
        Ok(())
    }

    /// Diagnostic hook the manager calls before rejecting a profile/restore whose enabled outputs
    /// do not resolve. It deliberately does NOT touch the topology: nothing it could do would
    /// help, so all it does is record WHY the display is unusable.
    ///
    /// Why no rescue is possible here: resolving means the display is ENUMERATED, not that it is
    /// active. The QDC_ALL_PATHS seeder already puts every connected-but-detached display in the
    /// layout (as `is_active=false`), so anything attachable already resolves; conversely an
    /// output that does NOT resolve names a display Windows is not enumerating at all — powered
    /// off, unplugged, or reported with `targetAvailable=FALSE`. Neither rescue can conjure that:
    /// an explicit attach only raises the ACTIVE flag on a path that must already exist, and
    /// SDC_TOPOLOGY_EXTEND replays the persistence database. (Attaching a display that is
    /// enumerated under a different identity than the profile expects would not make the profile
    /// resolve either — it would just move the user's screens for nothing.)
    ///
    /// This used to force an extend. In the field that only produced an unrequested topology flip
    /// — attaching every connected-inactive display, including a TV the user had just asked the
    /// profile to DETACH — plus a 3.5s stall, before failing anyway with ERROR_GEN_FAILURE.
    pub fn prepare_attach_targets(&self, desired: &Layout) -> Result<(), ManagerError> {
        let snapshot = query_active_topology()?;
        let unresolved = unresolved_enabled_outputs(desired, &snapshot);
        if unresolved.is_empty() {
            return Ok(());
        }

        let used_source_keys = active_source_keys(&snapshot);
        for output in &unresolved {
            let description = describe_output_for_error(output, &snapshot);
            let candidates = select_attach_candidates(
                &snapshot.attachable,
                &output.display_id,
                &used_source_keys,
            );
            if candidates.is_empty() {
                diagnostics::log(format!(
                    "prepare_attach_targets:no_candidate:{description}:skip_extend"
                ));
            } else {
                // Canary: an attach candidate for an output that does not resolve means the
                // connector now carries a display with a different identity than the profile
                // expects (its EDID changed). Attaching it would not make the profile resolve,
                // so the topology is still left alone.
                diagnostics::log(format!(
                    "prepare_attach_targets:candidate_for_unresolved_output:{description}:skip_extend"
                ));
            }
        }
        Ok(())
    }

    fn refresh_active(&self) -> Result<(), ManagerError> {
        let mut snapshot = query_active_topology()?;
        let fresh_connectors = raw_path_connectors(&snapshot.raw);
        let mut cache = self
            .cache
            .lock()
            .map_err(|_| ManagerError::Backend("windows backend cache poisoned".to_string()))?;

        if cache.reseed_persisted {
            cache.reseed_persisted = false;
            // The cache was invalidated (resume): re-seed detached-display entries from the
            // persisted snapshot so they survive a failed enrichment. The merge already rejects
            // snapshots with stale adapter LUIDs, preserving the point of the invalidation.
            if let Some(persisted_raw) = load_persisted_raw_snapshot() {
                if persisted_raw.paths.len() > snapshot.raw.paths.len() {
                    if let Some(merged) = merge_persisted_raw_for_fresh(&snapshot, &persisted_raw) {
                        diagnostics::log("backend_cache:reseeded_from_persisted_snapshot");
                        snapshot = merged;
                    }
                }
            }
        }

        cache.last_snapshot = Some(merge_snapshot_for_cache(
            cache.last_snapshot.as_ref(),
            snapshot.clone(),
        ));
        cache.last_layout = Some(merge_layout_with_fresh(
            cache.last_layout.as_ref(),
            &snapshot.layout,
            &fresh_connectors,
        ));
        cache.last_displays =
            merge_displays_with_fresh(&cache.last_displays, &snapshot.displays, &fresh_connectors);
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
    fresh: &TopologySnapshot,
    persisted_raw: &RawTopologySnapshot,
) -> Option<TopologySnapshot> {
    if !raw_covers_active_outputs_raw(persisted_raw, &fresh.layout) {
        return None;
    }

    let persisted_snapshot = snapshot_from_raw(persisted_raw.clone()).ok()?;

    let fresh_connectors = raw_path_connectors(&fresh.raw);
    Some(TopologySnapshot {
        raw: persisted_snapshot.raw,
        layout: merge_layout_with_fresh(
            Some(&persisted_snapshot.layout),
            &fresh.layout,
            &fresh_connectors,
        ),
        displays: merge_displays_with_fresh(
            &persisted_snapshot.displays,
            &fresh.displays,
            &fresh_connectors,
        ),
        // Attach candidates come from the live ALL_PATHS enumeration, never from persisted data.
        attachable: fresh.attachable.clone(),
    })
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

fn raw_path_connectors(raw: &RawTopologySnapshot) -> HashSet<(u64, u32)> {
    raw.paths
        .iter()
        .map(|path| {
            (
                luid_to_u64(
                    path.targetInfo.adapterId.HighPart,
                    path.targetInfo.adapterId.LowPart,
                ),
                path.targetInfo.id,
            )
        })
        .collect()
}

/// A cached entry is a stale duplicate when the same physical monitor (same EDID hash) shows up
/// in the fresh snapshot under a different (adapter_luid, target_id) AND the cached connector's
/// path no longer exists in the fresh snapshot at all (e.g. adapter LUID churn after resume or
/// reboot). Twin monitors with identical EDIDs are protected: both connectors keep existing.
fn cached_id_is_stale_duplicate<'a>(
    cached: &DisplayId,
    mut fresh_ids: impl Iterator<Item = &'a DisplayId>,
    fresh_connectors: &HashSet<(u64, u32)>,
) -> bool {
    let Some(edid_hash) = cached.edid_hash else {
        return false;
    };
    let cached_connector = (cached.adapter_luid, cached.target_id);
    if fresh_connectors.contains(&cached_connector) {
        return false;
    }
    fresh_ids.any(|fresh| {
        fresh.edid_hash == Some(edid_hash)
            && (fresh.adapter_luid, fresh.target_id) != cached_connector
    })
}

/// Enabled outputs of `desired` that fail to resolve against `snapshot`'s enumeration after
/// remapping — the same criterion the manager uses before rejecting a profile/restore.
fn unresolved_enabled_outputs(
    desired: &Layout,
    snapshot: &TopologySnapshot,
) -> Vec<monarch::OutputConfig> {
    let remapped = remap_layout_display_ids_for_snapshot(
        desired,
        &snapshot.layout,
        &raw_path_connectors(&snapshot.raw),
    );
    let current_ids: HashSet<DisplayId> = snapshot
        .layout
        .outputs
        .iter()
        .map(|output| output.display_id.clone())
        .collect();
    remapped
        .outputs
        .into_iter()
        .filter(|output| output.enabled && !current_ids.contains(&output.display_id))
        .collect()
}

/// True for an inactive entry with no usable geometry: the 0x0 sentinel the ALL_PATHS seeder
/// emits for connected-but-detached displays, whose real geometry Windows does not report.
fn output_is_geometry_sentinel(output: &monarch::OutputConfig) -> bool {
    !output.enabled && output.resolution.width == 0 && output.resolution.height == 0
}

fn display_is_geometry_sentinel(display: &DisplayInfo) -> bool {
    !display.is_active && display.resolution.width == 0 && display.resolution.height == 0
}

fn merge_layout_with_fresh(
    previous: Option<&Layout>,
    fresh: &Layout,
    fresh_connectors: &HashSet<(u64, u32)>,
) -> Layout {
    let Some(previous) = previous else {
        return fresh.clone();
    };

    let mut outputs: Vec<monarch::OutputConfig> = Vec::new();
    for cached in &previous.outputs {
        if cached_id_is_stale_duplicate(
            &cached.display_id,
            fresh.outputs.iter().map(|output| &output.display_id),
            fresh_connectors,
        ) {
            continue;
        }

        let cached_connector = (cached.display_id.adapter_luid, cached.display_id.target_id);
        let next = if let Some(active) = fresh.outputs.iter().find(|active| {
            (active.display_id.adapter_luid, active.display_id.target_id) == cached_connector
        }) {
            // Same connector: the fresh data wins. Keep a known EDID hash when the fresh read
            // transiently failed so the display identity stays stable across the glitch.
            let mut next = active.clone();
            if next.display_id.edid_hash.is_none() {
                next.display_id.edid_hash = cached.display_id.edid_hash;
            }
            // A seeded inactive entry carries no geometry (0x0 sentinel): keep the last real
            // geometry we knew for this connector instead of overwriting it every refresh.
            if output_is_geometry_sentinel(&next) && !output_is_geometry_sentinel(cached) {
                next.position = cached.position.clone();
                next.resolution = cached.resolution.clone();
                next.refresh_rate_mhz = cached.refresh_rate_mhz;
            }
            next
        } else {
            let mut inactive = cached.clone();
            inactive.enabled = false;
            inactive.primary = false;
            inactive
        };
        push_output_preferring_known_edid(&mut outputs, next);
    }
    for active in &fresh.outputs {
        let connector = (active.display_id.adapter_luid, active.display_id.target_id);
        if !outputs.iter().any(|output| {
            (output.display_id.adapter_luid, output.display_id.target_id) == connector
        }) {
            outputs.push(active.clone());
        }
    }

    let mut merged = Layout { outputs };
    if !merged
        .outputs
        .iter()
        .any(|output| output.enabled && output.primary)
    {
        if let Some(first) = merged.outputs.iter_mut().find(|output| output.enabled) {
            first.primary = true;
        }
    }
    merged
}

fn push_output_preferring_known_edid(
    outputs: &mut Vec<monarch::OutputConfig>,
    next: monarch::OutputConfig,
) {
    let connector = (next.display_id.adapter_luid, next.display_id.target_id);
    if let Some(existing) = outputs
        .iter_mut()
        .find(|output| (output.display_id.adapter_luid, output.display_id.target_id) == connector)
    {
        if existing.display_id.edid_hash.is_none() && next.display_id.edid_hash.is_some() {
            *existing = next;
        }
        return;
    }
    outputs.push(next);
}

fn merge_displays_with_fresh(
    previous: &[DisplayInfo],
    fresh: &[DisplayInfo],
    fresh_connectors: &HashSet<(u64, u32)>,
) -> Vec<DisplayInfo> {
    let mut merged: Vec<DisplayInfo> = Vec::new();
    for cached in previous {
        if cached_id_is_stale_duplicate(
            &cached.id,
            fresh.iter().map(|display| &display.id),
            fresh_connectors,
        ) {
            continue;
        }

        let cached_connector = (cached.id.adapter_luid, cached.id.target_id);
        let next = if let Some(active) = fresh
            .iter()
            .find(|active| (active.id.adapter_luid, active.id.target_id) == cached_connector)
        {
            let mut next = active.clone();
            if next.id.edid_hash.is_none() {
                next.id.edid_hash = cached.id.edid_hash;
            }
            // Seeded inactive entries carry the 0x0 sentinel: keep the last real geometry known
            // for this connector so the UI does not flip to 0x0 on every refresh tick.
            if display_is_geometry_sentinel(&next) && !display_is_geometry_sentinel(cached) {
                next.resolution = cached.resolution.clone();
                next.refresh_rate_mhz = cached.refresh_rate_mhz;
            }
            next
        } else {
            let mut inactive = cached.clone();
            inactive.is_active = false;
            inactive.is_primary = false;
            inactive
        };
        push_display_preferring_known_edid(&mut merged, next);
    }
    for active in fresh {
        let connector = (active.id.adapter_luid, active.id.target_id);
        if !merged
            .iter()
            .any(|display| (display.id.adapter_luid, display.id.target_id) == connector)
        {
            merged.push(active.clone());
        }
    }
    merged.sort_by(|left, right| {
        left.friendly_name
            .cmp(&right.friendly_name)
            .then(left.id.target_id.cmp(&right.id.target_id))
    });
    merged
}

fn push_display_preferring_known_edid(displays: &mut Vec<DisplayInfo>, next: DisplayInfo) {
    let connector = (next.id.adapter_luid, next.id.target_id);
    if let Some(existing) = displays
        .iter_mut()
        .find(|display| (display.id.adapter_luid, display.id.target_id) == connector)
    {
        if existing.id.edid_hash.is_none() && next.id.edid_hash.is_some() {
            *existing = next;
        }
        return;
    }
    displays.push(next);
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
        // Capture before the first mutation, not after an unsuccessful first apply.
        let previous = capture_pre_recovery_state()?;
        match self.apply_layout_inner(layout) {
            Ok(()) => Ok(()),
            Err(error) => {
                let rollback = restore_pre_extend_topology(&previous);
                let _ = self.refresh_active();
                Err(ManagerError::Backend(match rollback {
                    Ok(()) => format!("{error}. The previous display layout was restored."),
                    Err(rollback_error) => format!("{error}. Restoring the previous layout also failed: {rollback_error}. Use Windows Display Settings to recover."),
                }))
            }
        }
    }

    fn color_state_signature(&self) -> Result<Option<String>, ManagerError> {
        WindowsDisplayBackend::color_state_signature(self)
    }

    fn reapply_color_calibration(&self) -> Result<(), ManagerError> {
        WindowsDisplayBackend::reapply_color_calibration(self)
    }

    fn invalidate_cache(&self) -> Result<(), ManagerError> {
        WindowsDisplayBackend::invalidate_cache(self)
    }

    fn prepare_attach_targets(&self, desired: &Layout) -> Result<(), ManagerError> {
        WindowsDisplayBackend::prepare_attach_targets(self, desired)
    }
}

impl WindowsDisplayBackend {
    fn apply_layout_inner(&self, layout: Layout) -> Result<(), ManagerError> {
        layout.ensure_valid()?;
        diagnostics::log(format!(
            "topology_apply:start:outputs={}",
            layout.outputs.len()
        ));

        // Re-query the currently active topology so detach-only operations use a minimal base.
        // This reduces the chance of Windows re-touching unrelated outputs.
        let active_snapshot = query_active_topology()?;
        let needs_attach_paths = desired_enables_inactive_output(&layout, &active_snapshot.layout);

        let base_snapshot = if !needs_attach_paths {
            // Detach-only change: apply against a minimal, non-enriched active snapshot so
            // database-sourced paths are never fed into SetDisplayConfig (avoids spurious
            // error-87 risks mid-detach).
            query_active_only_topology()?
        } else {
            let cache = self
                .cache
                .lock()
                .map_err(|_| ManagerError::Backend("windows backend cache poisoned".to_string()))?;

            if let Some(cached) = cache.last_snapshot.clone() {
                if raw_covers_active_outputs_raw(&cached.raw, &active_snapshot.layout) {
                    cached
                } else {
                    active_snapshot.clone()
                }
            } else {
                active_snapshot.clone()
            }
        };

        let working_layout = remap_layout_display_ids_for_snapshot(
            &layout,
            &base_snapshot.layout,
            &raw_path_connectors(&active_snapshot.raw),
        );

        let missing_attach_outputs =
            enabled_outputs_missing_from_raw(&working_layout, &base_snapshot.raw);
        let (next_snapshot, applied_layout) = if !missing_attach_outputs.is_empty() {
            // The base snapshot has no path for these outputs, so flipping active flags would be
            // a silent no-op (SetDisplayConfig returns 0 on an unchanged active set). Recover
            // UNCONDITIONALLY: an "is it connected?" guard here would rely on the same
            // enumeration that just failed to surface the display, blocking exactly the case it
            // must cure. The cost for a genuinely absent monitor is one harmless attempt
            // followed by the same precise error from the retry validation.
            for output in &missing_attach_outputs {
                diagnostics::log(format!(
                    "recover:extend_attempt:{}",
                    describe_output_for_error(output, &base_snapshot)
                ));
            }
            recover_apply_with_topology_extend(
                &working_layout,
                &missing_attach_outputs,
                &active_snapshot,
            )?
        } else {
            match apply_layout_against_snapshot(&working_layout, &base_snapshot) {
                Ok(snapshot) => (snapshot, working_layout),
                Err(error) if is_set_display_invalid_parameter(&error) => {
                    diagnostics::log("topology_apply:retry:reason=setdisplayconfig_87");
                    recover_apply_with_topology_extend(&working_layout, &[], &active_snapshot)?
                }
                Err(error) => {
                    diagnostics::log(format!("topology_apply:error:{error}"));
                    return Err(error);
                }
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
        diagnostics::log("topology_apply:done");

        Ok(())
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

fn remap_layout_display_ids_for_snapshot(
    desired: &Layout,
    current: &Layout,
    enumerated_connectors: &HashSet<(u64, u32)>,
) -> Layout {
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
            replacement = choose_remap_candidate(&candidates, enumerated_connectors)
                .map(|candidate| candidate.display_id.clone());
        }

        if replacement.is_none() && output.display_id.edid_hash.is_none() {
            // Never guess across adapters in the hash-less fallback: iGPU/dGPU pairs reuse the
            // same target id numbering, so a cross-adapter pick could hit the wrong monitor.
            let candidates = unique_unused_candidates_by_target_id(
                output.display_id.target_id,
                &current.outputs,
                &used,
            );
            if candidates_share_one_adapter(&candidates) {
                replacement = choose_remap_candidate(&candidates, enumerated_connectors)
                    .map(|candidate| candidate.display_id.clone());
            }
        }

        if let Some(next_id) = replacement {
            used.insert(next_id.clone());
            output.display_id = next_id;
        }
    }

    remapped
}

fn candidates_share_one_adapter(candidates: &[&monarch::OutputConfig]) -> bool {
    let mut adapters = candidates
        .iter()
        .map(|candidate| candidate.display_id.adapter_luid);
    let Some(first) = adapters.next() else {
        return true;
    };
    adapters.all(|adapter| adapter == first)
}

fn choose_remap_candidate<'a>(
    candidates: &[&'a monarch::OutputConfig],
    enumerated_connectors: &HashSet<(u64, u32)>,
) -> Option<&'a monarch::OutputConfig> {
    if candidates.is_empty() {
        return None;
    }
    if candidates.len() == 1 {
        return Some(candidates[0]);
    }

    // Deterministic tie-break for duplicate identities (e.g. a stale cached entry plus the same
    // physical monitor re-enumerated under a new adapter LUID after resume/reboot): prefer the
    // candidate whose connector is currently enumerated, then the single enabled one. Two active
    // identical twins stay ambiguous and are left unmapped.
    let enumerated: Vec<_> = candidates
        .iter()
        .copied()
        .filter(|candidate| {
            enumerated_connectors.contains(&(
                candidate.display_id.adapter_luid,
                candidate.display_id.target_id,
            ))
        })
        .collect();
    if enumerated.len() == 1 {
        return Some(enumerated[0]);
    }

    let pool = if enumerated.is_empty() {
        candidates.to_vec()
    } else {
        enumerated
    };
    let enabled: Vec<_> = pool
        .iter()
        .copied()
        .filter(|candidate| candidate.enabled)
        .collect();
    if enabled.len() == 1 {
        return Some(enabled[0]);
    }

    None
}

fn enabled_outputs_missing_from_raw<'a>(
    layout: &'a Layout,
    raw: &RawTopologySnapshot,
) -> Vec<&'a monarch::OutputConfig> {
    let active = RawTopologySnapshot {
        paths: raw
            .paths
            .iter()
            .filter(|p| p.flags & DISPLAYCONFIG_PATH_ACTIVE_FLAG != 0)
            .copied()
            .collect(),
        modes: Vec::new(),
    };
    let connectors = raw_path_connectors(&active);
    layout
        .outputs
        .iter()
        .filter(|output| output.enabled)
        .filter(|output| {
            !connectors.contains(&(output.display_id.adapter_luid, output.display_id.target_id))
        })
        .collect()
}

fn describe_output_for_error(
    output: &monarch::OutputConfig,
    base_snapshot: &TopologySnapshot,
) -> String {
    let edid = output
        .display_id
        .edid_hash
        .map(|value| format!("{value:016x}"))
        .unwrap_or_else(|| "-".to_string());
    let friendly = base_snapshot
        .displays
        .iter()
        .find(|display| {
            display.id == output.display_id
                || (output.display_id.edid_hash.is_some()
                    && display.id.edid_hash == output.display_id.edid_hash)
        })
        .map(|display| format!("'{}' ", display.friendly_name))
        .unwrap_or_default();
    format!(
        "{friendly}(target_id={}, edid_hash={edid})",
        output.display_id.target_id
    )
}

const RECOVER_SETTLE_DEADLINE: std::time::Duration = std::time::Duration::from_millis(3500);
const RECOVER_SETTLE_STEP: std::time::Duration = std::time::Duration::from_millis(250);
/// Grace window after an explicit attach Windows already accepted: it only has to cover the
/// display's handshake, so it is much shorter than the deadline for an extend that may have to
/// wake a target from scratch.
const ATTACH_SETTLE_DEADLINE: std::time::Duration = std::time::Duration::from_millis(1500);

/// Fill in geometry for enabled outputs that still carry the 0x0 sentinel (a display seeded from
/// ALL_PATHS and never active on this boot) using the post-extend snapshot, where Windows has
/// just assigned it a real source mode.
fn fill_sentinel_geometry_from_snapshot(layout: &mut Layout, snapshot: &TopologySnapshot) {
    for output in &mut layout.outputs {
        if !output.enabled || output.resolution.width != 0 || output.resolution.height != 0 {
            continue;
        }
        let Some(active) = snapshot
            .layout
            .outputs
            .iter()
            .find(|active| active.display_id == output.display_id)
        else {
            continue;
        };
        output.position = active.position.clone();
        output.resolution = active.resolution.clone();
        output.refresh_rate_mhz = active.refresh_rate_mhz;
    }
}

/// Source keys `(source adapter luid, source id)` currently driving an active path. Attaching a
/// target onto a busy source would clone that display instead of extending onto it.
fn active_source_keys(snapshot: &TopologySnapshot) -> HashSet<(u64, u32)> {
    snapshot
        .raw
        .paths
        .iter()
        .filter(|path| path.flags & DISPLAYCONFIG_PATH_ACTIVE_FLAG != 0)
        .map(|path| {
            (
                luid_to_u64(
                    path.sourceInfo.adapterId.HighPart,
                    path.sourceInfo.adapterId.LowPart,
                ),
                path.sourceInfo.id,
            )
        })
        .collect()
}

fn attachable_source_key(candidate: &AttachablePath) -> (u64, u32) {
    (
        luid_to_u64(
            candidate.path.sourceInfo.adapterId.HighPart,
            candidate.path.sourceInfo.adapterId.LowPart,
        ),
        candidate.path.sourceInfo.id,
    )
}

/// Attach candidates for `display_id` whose source is currently free. ALL_PATHS reports one
/// entry per (source, target) combination; picking a busy source would clone rather than extend,
/// so those combinations are dropped rather than rewritten.
fn select_attach_candidates<'a>(
    attachable: &'a [AttachablePath],
    display_id: &DisplayId,
    used_source_keys: &HashSet<(u64, u32)>,
) -> Vec<&'a AttachablePath> {
    attachable
        .iter()
        .filter(|candidate| {
            candidate.adapter_luid == display_id.adapter_luid
                && candidate.target_id == display_id.target_id
        })
        .filter(|candidate| !used_source_keys.contains(&attachable_source_key(candidate)))
        .collect()
}

/// Activate every still-missing enabled output in ONE SetDisplayConfig call.
///
/// The supplied path array is the complete topology, so a per-display call would deactivate
/// whatever the previous call activated. Instead the batch grows one candidate at a time, each
/// step confirmed with a free SDC_VALIDATE dry-run (alternate sources are tried when a candidate
/// is refused), and a single apply lands at the end — one topology flip, not N.
///
/// Returns true only when the final apply returned 0. That still does NOT prove any display came
/// back (SetDisplayConfig returns 0 for a no-op), so the caller must confirm against a fresh
/// enumeration before deciding to skip the extend.
fn try_batch_explicit_attach(
    missing: &[&monarch::OutputConfig],
    active_snapshot: &TopologySnapshot,
) -> bool {
    // Guard: the error-87 recovery path calls in with nothing missing. Without this, an empty
    // batch would report "everything attached" and silently kill the extend fallback.
    if missing.is_empty() {
        return false;
    }

    let mut used_source_keys = active_source_keys(active_snapshot);
    let mut batch: Vec<&AttachablePath> = Vec::new();

    for output in missing {
        let description = describe_output_for_error(output, active_snapshot);
        let candidates = select_attach_candidates(
            &active_snapshot.attachable,
            &output.display_id,
            &used_source_keys,
        );
        if candidates.is_empty() {
            diagnostics::log(format!("recover:no_attachable_candidate:{description}"));
            continue;
        }

        let mut accepted = false;
        for candidate in candidates {
            batch.push(candidate);
            let paths = build_attach_paths(&batch, active_snapshot);
            let status = validate_attach_paths(&paths, active_snapshot);
            diagnostics::log(format!(
                "recover:explicit_attach:{description}:source={}:validate={status}",
                attachable_source_key(candidate).1
            ));
            if status == 0 {
                // Claim the source so a later output in this batch cannot reuse it.
                used_source_keys.insert(attachable_source_key(candidate));
                accepted = true;
                break;
            }
            batch.pop();
        }
        if !accepted {
            diagnostics::log(format!(
                "recover:explicit_attach:{description}:no_candidate_validated"
            ));
        }
    }

    if batch.is_empty() {
        return false;
    }

    let paths = build_attach_paths(&batch, active_snapshot);
    let status = apply_attach_paths(&paths, active_snapshot);
    diagnostics::log(format!(
        "recover:explicit_attach:batch={}:apply={status}",
        batch.len()
    ));
    status == 0
}

/// Best-effort undo of a recovery that did not pan out. Both the explicit attach and the extend
/// change (and persist) the topology, so leaving them in place would silently rewrite the user's
/// setup on a failed attach. Re-applying the pre-recovery layout works because its enabled set
/// only covers the previously active outputs, and apply's `unwrap_or(false)` disables everything
/// the recovery added.
fn restore_pre_extend_topology(previous: &TopologySnapshot) -> Result<(), ManagerError> {
    apply_layout_against_snapshot(&previous.layout, previous).map(|_| ())
}

/// The pre-recovery topology is the ONLY rollback net on a machine with no internal panel, so it
/// is a hard precondition rather than an optional extra: capture it (with one retry, because it
/// fails exactly when a transient QueryDisplayConfig hiccup is most likely) or do not touch the
/// topology at all.
fn capture_pre_recovery_state() -> Result<TopologySnapshot, ManagerError> {
    match query_active_only_topology() {
        Ok(snapshot) => Ok(snapshot),
        Err(first_error) => {
            diagnostics::log(format!(
                "recover:pre_state_query_failed:{first_error}:retrying"
            ));
            std::thread::sleep(RECOVER_SETTLE_STEP);
            query_active_only_topology()
        }
    }
}

enum SettleOutcome {
    Settled(TopologySnapshot, Layout),
    StillMissing(String),
}

/// Poll a fresh enumeration until every enabled output of `working_layout` resolves, or the
/// deadline passes. Polling (rather than one fixed sleep) is what an HDMI/TV handshake needs,
/// and the remap is redone on every attempt because the connector can come back under a
/// different (adapter_luid, target_id).
///
/// Reports what it observed and nothing more: rollback and error wording are the caller's call.
fn settle_poll(
    working_layout: &Layout,
    deadline: std::time::Duration,
    label: &str,
) -> Result<SettleOutcome, ManagerError> {
    let deadline_at = std::time::Instant::now() + deadline;
    let mut attempt = 0usize;
    loop {
        attempt += 1;
        std::thread::sleep(RECOVER_SETTLE_STEP);
        let snapshot = query_active_only_topology()?;
        let layout = remap_layout_display_ids_for_snapshot(
            working_layout,
            &snapshot.layout,
            &raw_path_connectors(&snapshot.raw),
        );
        let missing = enabled_outputs_missing_from_raw(&layout, &snapshot.raw);
        diagnostics::log(format!(
            "recover:settle_poll:{label}:{attempt}:missing={}",
            missing.len()
        ));
        if missing.is_empty() {
            return Ok(SettleOutcome::Settled(snapshot, layout));
        }
        if std::time::Instant::now() >= deadline_at {
            return Ok(SettleOutcome::StillMissing(describe_output_for_error(
                missing[0], &snapshot,
            )));
        }
    }
}

/// Apply the desired layout once the recovery has brought every output back.
fn finish_recovery(
    recovered_snapshot: TopologySnapshot,
    mut retry_layout: Layout,
) -> Result<(TopologySnapshot, Layout), ManagerError> {
    fill_sentinel_geometry_from_snapshot(&mut retry_layout, &recovered_snapshot);
    let snapshot = apply_layout_against_snapshot(&retry_layout, &recovered_snapshot)?;
    Ok((snapshot, retry_layout))
}

fn recover_apply_with_topology_extend(
    working_layout: &Layout,
    missing: &[&monarch::OutputConfig],
    active_snapshot: &TopologySnapshot,
) -> Result<(TopologySnapshot, Layout), ManagerError> {
    // Every recovery step actually attempted, so the final error can name them honestly.
    let mut attempted: Vec<&str> = Vec::new();

    // (a) Explicit attach: activates these exact targets from their own enumerated paths, the
    // way Windows Display settings does. SDC_TOPOLOGY_EXTEND cannot substitute for it — it
    // replays the last extended configuration from the persistence database, and a Monarch
    // detach (saved with SDC_SAVE_TO_DATABASE) already removed this display from that entry.
    if try_batch_explicit_attach(missing, active_snapshot) {
        attempted.push("an explicit attach");
        // A 0 from SetDisplayConfig only means "accepted", never "the display is back": confirm
        // against a fresh enumeration, and keep escalating if it did not actually return.
        match settle_poll(working_layout, ATTACH_SETTLE_DEADLINE, "attach") {
            Ok(SettleOutcome::Settled(snapshot, layout)) => {
                diagnostics::log("recover:resolved:explicit_attach");
                return finish_recovery(snapshot, layout);
            }
            Ok(SettleOutcome::StillMissing(_)) => {
                diagnostics::log("recover:attach_not_observed:escalating");
            }
            Err(error) => {
                return Err(error);
            }
        }
    }

    // (b) CCD topology extend. Its status cannot judge success (0 is also returned for a no-op),
    // so the settle poll decides.
    attempted.push("a topology extend");
    try_topology_extend();
    let still_missing = match settle_poll(working_layout, RECOVER_SETTLE_DEADLINE, "extend") {
        Ok(SettleOutcome::Settled(snapshot, layout)) => {
            diagnostics::log("recover:resolved:topology_extend");
            return finish_recovery(snapshot, layout);
        }
        Ok(SettleOutcome::StillMissing(description)) => description,
        Err(error) => {
            return Err(error);
        }
    };

    // (c) DisplaySwitch: same shell path as Win+P, last resort.
    diagnostics::log(format!("recover:escalate:display_switch:{still_missing}"));
    if let Err(error) = run_display_switch_extend() {
        diagnostics::log(format!("recover:display_switch_failed:{error}"));
        return Err(error);
    }
    attempted.push("DisplaySwitch /extend");

    let still_missing = match settle_poll(working_layout, RECOVER_SETTLE_DEADLINE, "display_switch")
    {
        Ok(SettleOutcome::Settled(snapshot, layout)) => {
            diagnostics::log("recover:resolved:display_switch");
            return finish_recovery(snapshot, layout);
        }
        Ok(SettleOutcome::StillMissing(description)) => description,
        Err(error) => {
            return Err(error);
        }
    };

    // (d) Out of options: undo everything the recovery touched and name what was tried.
    diagnostics::log(format!("recover:still_missing:{still_missing}"));
    Err(ManagerError::Backend(format!(
        "cannot attach display {still_missing}: it did not come back after {}. reconnect it or attach it once from Windows Display settings",
        attempted.join(", then ")
    )))
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
mod recovery_verification_tests {
    use super::*;
    use monarch::{OutputConfig, Position, Resolution};
    use windows::Win32::Devices::Display::DISPLAYCONFIG_PATH_INFO;

    #[test]
    fn an_inactive_database_path_does_not_prove_recovery_succeeded() {
        let desired = Layout {
            outputs: vec![OutputConfig {
                display_id: DisplayId {
                    adapter_luid: 1,
                    target_id: 2,
                    edid_hash: None,
                },
                enabled: true,
                primary: true,
                position: Position { x: 0, y: 0 },
                resolution: Resolution {
                    width: 1920,
                    height: 1080,
                },
                refresh_rate_mhz: 60_000,
            }],
        };
        let mut path = DISPLAYCONFIG_PATH_INFO::default();
        path.targetInfo.adapterId.LowPart = 1;
        path.targetInfo.id = 2;
        let mut raw = RawTopologySnapshot {
            paths: vec![path],
            modes: vec![],
        };
        assert_eq!(enabled_outputs_missing_from_raw(&desired, &raw).len(), 1);
        raw.paths[0].flags = DISPLAYCONFIG_PATH_ACTIVE_FLAG;
        assert!(enabled_outputs_missing_from_raw(&desired, &raw).is_empty());
    }
}
