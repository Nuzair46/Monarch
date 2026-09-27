use super::apply::{
    active_color_state_signature, apply_layout_against_snapshot, capture_sdr_gamma_ramps,
    gamma_ramp_looks_identity, reapply_color_calibration_for_active_with_cached_sdr,
    run_display_switch_extend, try_topology_extend, GammaRampKey, GammaRampWords,
};
use super::enumerate::{
    query_active_only_topology, query_active_topology, query_connected_topology,
};
use super::recovery::recover_layout;
use super::win32_types::{RawTopologySnapshot, TopologySnapshot};
use crate::diagnostics;
use monarch::history::GeometryHistory;
use monarch::{DisplayBackend, DisplayInfo, DisplaySnapshot, Layout, ManagerError};
use std::collections::{HashMap, HashSet};
use std::sync::Mutex;
const DISPLAYCONFIG_PATH_ACTIVE_FLAG: u32 = 1;

#[derive(Default)]
struct BackendCache {
    remembered: GeometryHistory,
    generation: u64,
    sdr_gamma_cache: HashMap<GammaRampKey, GammaRampWords>,
}

#[derive(Default)]
pub struct WindowsDisplayBackend {
    cache: Mutex<BackendCache>,
}

fn inventory_path() -> std::path::PathBuf {
    monarch::FileConfigStore::default_config_path().with_file_name("monitor_geometry.json")
}

fn load_geometry() -> GeometryHistory {
    std::fs::read(inventory_path())
        .ok()
        .and_then(|bytes| serde_json::from_slice::<GeometryHistory>(&bytes).ok())
        .filter(GeometryHistory::is_valid)
        .unwrap_or_default()
}

impl WindowsDisplayBackend {
    pub fn new() -> Result<Self, ManagerError> {
        let backend = Self::default();
        backend.cache.lock().unwrap().remembered = load_geometry();
        backend.refresh_active()?;
        let native = query_active_only_topology()?;
        backend.cache.lock().unwrap().sdr_gamma_cache = capture_sdr_gamma_ramps(&native);
        Ok(backend)
    }

    pub fn invalidate_cache(&self) -> Result<(), ManagerError> {
        // Every snapshot enumerates live routes. Only typed geometry and gamma
        // preferences survive a notification, so there is no route cache to clear.
        Ok(())
    }

    pub fn prepare_attach_targets(&self, _desired: &Layout) -> Result<(), ManagerError> {
        self.refresh_active().map(|_| ())
    }

    fn refresh_active(&self) -> Result<DisplaySnapshot, ManagerError> {
        let mut fresh = query_active_topology()?;
        let mut cache = self
            .cache
            .lock()
            .map_err(|_| ManagerError::Backend("windows backend cache poisoned".into()))?;
        cache.remembered.complete_inventory(&mut fresh.layout);
        for display in &mut fresh.displays {
            if let Some(output) = fresh
                .layout
                .outputs
                .iter()
                .find(|o| o.display_id == display.id && !o.enabled)
            {
                display.resolution = output.resolution.clone();
                display.refresh_rate_mhz = output.refresh_rate_mhz;
            }
        }
        // Stable connection/monitor keys, not changing friendly names, determine default slots.
        fresh.displays.sort_by(|a, b| {
            a.id.identity
                .edid_serial
                .cmp(&b.id.identity.edid_serial)
                .then(a.id.identity.device_path.cmp(&b.id.identity.device_path))
                .then(a.id.edid_hash.cmp(&b.id.edid_hash))
                .then(a.id.endpoint().cmp(&b.id.endpoint()))
        });
        let before = cache.remembered.clone();
        cache.remembered.remember(&fresh.layout);
        cache.generation += 1;
        let snapshot = DisplaySnapshot {
            generation: cache.generation,
            layout: fresh.layout,
            displays: fresh.displays,
        };
        let changed = before != cache.remembered;
        let remembered = cache.remembered.clone();
        drop(cache);
        if changed {
            if let Ok(bytes) = serde_json::to_vec(&remembered) {
                if let Err(error) = monarch::store::atomic_write(&inventory_path(), &bytes) {
                    diagnostics::log(format!("geometry_cache:save_failed:{error}"));
                }
            }
        }
        Ok(snapshot)
    }

    pub fn color_state_signature(&self) -> Result<Option<String>, ManagerError> {
        Ok(Some(active_color_state_signature(
            &query_active_only_topology()?,
        )))
    }

    pub fn reapply_color_calibration(&self) -> Result<(), ManagerError> {
        let cached = self
            .cache
            .lock()
            .map_err(|_| ManagerError::Backend("windows backend cache poisoned".into()))?
            .sdr_gamma_cache
            .clone();
        reapply_color_calibration_for_active_with_cached_sdr(&cached)
    }

    fn apply_layout_inner(&self, layout: Layout) -> Result<(), ManagerError> {
        let active = query_active_only_topology()?;
        let inventory = self.refresh_active()?;
        let desired = monarch::identity::resolve_layout(&layout, &inventory.layout)?;
        let (observed, _) = if desired_enables_inactive_output(&desired, &active.layout) {
            recover_apply_with_topology_extend(&desired)?
        } else {
            match apply_layout_against_snapshot(&desired, &active) {
                Ok(observed) => (observed, desired),
                Err(error) => {
                    diagnostics::log(format!("apply:recovery:{error}"));
                    recover_apply_with_topology_extend(&desired)?
                }
            }
        };
        merge_sdr_gamma_cache(
            &mut self
                .cache
                .lock()
                .map_err(|_| ManagerError::Backend("windows backend cache poisoned".into()))?
                .sdr_gamma_cache,
            capture_sdr_gamma_ramps(&observed),
        );
        self.refresh_active()?;
        Ok(())
    }
}

impl DisplayBackend for WindowsDisplayBackend {
    fn snapshot(&self) -> Result<DisplaySnapshot, ManagerError> {
        self.refresh_active()
    }
    fn list_displays(&self) -> Result<Vec<DisplayInfo>, ManagerError> {
        Ok(self.snapshot()?.displays)
    }
    fn get_layout(&self) -> Result<Layout, ManagerError> {
        Ok(self.snapshot()?.layout)
    }
    fn apply_layout(&self, layout: Layout) -> Result<(), ManagerError> {
        layout.ensure_supported()?;
        let previous = capture_pre_recovery_state()?;
        previous.layout.ensure_supported()?;
        let _wallpaper = super::wallpaper::WallpaperState::capture(&previous);
        match self.apply_layout_inner(layout) {
            Ok(()) => Ok(()),
            Err(error) => match restore_pre_extend_topology(&previous) {
                Ok(()) => Err(ManagerError::ApplyRestored(error.to_string())),
                Err(rollback) => Err(ManagerError::RecoveryRequired(format!(
                    "{error}; restoring previous layout failed: {rollback}"
                ))),
            },
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

fn raw_path_connectors(raw: &RawTopologySnapshot) -> HashSet<(u64, u32)> {
    raw.paths
        .iter()
        .map(|p| {
            (
                super::win32_types::luid_to_u64(
                    p.targetInfo.adapterId.HighPart,
                    p.targetInfo.adapterId.LowPart,
                ),
                p.targetInfo.id,
            )
        })
        .collect()
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
            && !active_layout.outputs.iter().any(|active| {
                active.enabled && active.display_id.endpoint() == output.display_id.endpoint()
            })
    })
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
        if output.rotation.is_none() {
            output.rotation = active.rotation;
        }
    }
}

/// Best-effort undo of a recovery that did not pan out. Both the explicit attach and the extend
/// change (and persist) the topology, so leaving them in place would silently rewrite the user's
/// setup on a failed attach. Re-applying the pre-recovery layout works because its enabled set
/// only covers the previously active outputs, and apply's `unwrap_or(false)` disables everything
/// the recovery added.
fn restore_pre_extend_topology(previous: &TopologySnapshot) -> Result<(), ManagerError> {
    query_active_only_topology()
        .and_then(|active| {
            let desired = monarch::identity::resolve_layout(&previous.layout, &active.layout)?;
            apply_layout_against_snapshot(&desired, &active)
        })
        .or_else(|initial| {
            let connected = query_connected_topology()?;
            let desired = monarch::identity::resolve_layout(&previous.layout, &connected.layout)?;
            let recovered = recover_layout(&desired, &connected).map_err(|error| {
                ManagerError::Backend(format!("{initial}; rollback reconnect failed: {error}"))
            })?;
            apply_layout_against_snapshot(&desired, &recovered)
        })
        .map(|_| ())
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
        let snapshot = match query_active_only_topology() {
            Ok(snapshot) => snapshot,
            Err(error) if std::time::Instant::now() < deadline_at => {
                diagnostics::log(format!("recover:settle_query:{label}:{error}"));
                continue;
            }
            Err(error) => return Err(error),
        };
        let layout = match monarch::identity::resolve_layout(working_layout, &snapshot.layout) {
            Ok(layout) => layout,
            Err(error) if std::time::Instant::now() < deadline_at => {
                diagnostics::log(format!("recover:settle_identity:{label}:{error}"));
                continue;
            }
            Err(error) => return Ok(SettleOutcome::StillMissing(error.to_string())),
        };
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
) -> Result<(TopologySnapshot, Layout), ManagerError> {
    // Every recovery step actually attempted, so the final error can name them honestly.
    let mut attempted: Vec<&str> = Vec::new();

    // Select a complete one-source-per-target assignment from fresh alternatives.
    // Do not splice QDC_ALL_PATHS routes directly into the cached topology.
    attempted.push("an explicit topology assignment");
    let explicit = query_connected_topology().and_then(|connected| {
        let desired = monarch::identity::resolve_layout(working_layout, &connected.layout)?;
        let recovered = recover_layout(&desired, &connected)?;
        finish_recovery(recovered, desired)
    });
    match explicit {
        Ok(result) => return Ok(result),
        Err(error) => diagnostics::log(format!("recover:explicit_assignment_failed:{error}")),
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

#[cfg(test)]
mod hardware_tests {
    use super::*;

    struct RestoreOnDrop(TopologySnapshot);

    impl Drop for RestoreOnDrop {
        fn drop(&mut self) {
            if let Err(error) = restore_pre_extend_topology(&self.0) {
                eprintln!("Hardware test cleanup failed: {error}");
            }
        }
    }

    #[test]
    #[ignore = "Changes real displays. Run explicitly on a local console with at least three active monitors."]
    fn restores_two_detached_monitors_after_cold_start_with_incomplete_cache() {
        let original = query_active_only_topology().expect("local console display inventory");
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

        // A new backend must discover physically connected but inactive displays
        // from fresh routes, including after a resume/cache invalidation notification.
        let backend = WindowsDisplayBackend::new().unwrap();
        let inventory = backend.list_displays().unwrap();
        assert!(original
            .layout
            .outputs
            .iter()
            .all(|o| inventory.iter().any(|d| d.id == o.display_id)));
        backend.invalidate_cache().unwrap();
        backend.apply_layout(original.layout.clone()).unwrap();
        let restored = query_active_only_topology().unwrap();
        monarch::verification::verify_applied_layout(&original.layout, &restored.layout).unwrap();
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
