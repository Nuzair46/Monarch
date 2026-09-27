use super::apply::{
    active_color_state_signature, apply_layout_against_snapshot, capture_sdr_gamma_ramps,
    gamma_ramp_looks_identity, reapply_color_calibration_for_active_with_cached_sdr, GammaRampKey,
    GammaRampWords,
};
use super::enumerate::{
    query_active_only_topology, query_active_topology, query_connected_topology,
};
use super::win32_types::TopologySnapshot;
use crate::diagnostics;
use monarch::history::GeometryHistory;
use monarch::{DisplayBackend, DisplayInfo, DisplaySnapshot, Layout, ManagerError};
use std::collections::HashMap;
use std::sync::Mutex;

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
        let connected = query_connected_topology()?;
        let desired = monarch::identity::resolve_layout(&layout, &connected.layout)?;
        let observed = apply_layout_against_snapshot(&desired, &connected)?;
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
    fn get_display_capabilities(
        &self,
    ) -> Result<Vec<monarch::capabilities::DisplayCapabilities>, ManagerError> {
        super::capabilities::current()
    }
    fn validate_layout(&self, layout: &Layout) -> Result<(), ManagerError> {
        let connected = query_connected_topology()?;
        monarch::capabilities::validate(layout, &super::capabilities::discover(&connected))?;
        let (paths, modes) = super::apply::plan_layout(layout, &connected)?;
        use windows::Win32::Devices::Display::*;
        let status = unsafe {
            SetDisplayConfig(
                Some(&paths),
                Some(&modes),
                SDC_VALIDATE | SDC_USE_SUPPLIED_DISPLAY_CONFIG,
            )
        };
        if status != 0 {
            return Err(ManagerError::Validation(format!("Windows rejected the requested display combination ({status}); select compatible resolution/refresh or Extend")));
        }
        Ok(())
    }

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
        let gamma = super::apply::capture_active_gamma_ramps(&previous);
        let result = monarch::transaction::apply_with_recovery(
            || self.apply_layout_inner(layout),
            || restore_captured_state(&previous),
        );
        if let Ok(observed) = query_active_only_topology() {
            super::apply::best_effort_reload_color_calibration();
            super::apply::best_effort_restore_gamma_ramps(&observed, &gamma);
        }
        result
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

fn restore_captured_state(previous: &TopologySnapshot) -> Result<(), ManagerError> {
    use windows::Win32::Devices::Display::*;
    let connected = query_connected_topology()?;
    let desired = monarch::identity::resolve_layout(&previous.layout, &connected.layout)?;
    let same_endpoints = previous
        .layout
        .outputs
        .iter()
        .zip(&desired.outputs)
        .all(|(a, b)| !a.enabled || a.display_id.endpoint() == b.display_id.endpoint());
    let flags = SDC_USE_SUPPLIED_DISPLAY_CONFIG;
    let validation = if same_endpoints {
        unsafe {
            SetDisplayConfig(
                Some(&previous.raw.paths),
                Some(&previous.raw.modes),
                SDC_VALIDATE | flags,
            )
        }
    } else {
        -1
    };
    let status = if validation == 0 {
        unsafe {
            SetDisplayConfig(
                Some(&previous.raw.paths),
                Some(&previous.raw.modes),
                SDC_APPLY | flags | SDC_SAVE_TO_DATABASE,
            )
        }
    } else {
        validation
    };
    if status != 0 {
        apply_layout_against_snapshot(&desired, &connected)?;
    } else {
        let refreshed = super::apply::wait_for_requested_outputs(&desired)?;
        super::apply::restore_preferences(&desired, &refreshed)?;
    }
    super::apply::wait_for_verified_layout(&desired).map(|_| ())
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
            std::thread::sleep(std::time::Duration::from_millis(250));
            query_active_only_topology()
        }
    }
}

#[cfg(test)]
mod hardware_tests {
    use super::*;

    struct RestoreOnDrop(TopologySnapshot);

    impl Drop for RestoreOnDrop {
        fn drop(&mut self) {
            if let Err(error) = restore_captured_state(&self.0) {
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
