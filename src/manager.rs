use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};

use crate::backend::DisplayBackend;
use crate::model::{
    AppConfig, AppSettings, DisplayId, DisplayInfo, Layout, Profile,
    DEFAULT_DISPLAY_TOGGLE_SHORTCUT_BASE, DEFAULT_PROFILE_SHORTCUT_BASE,
};
use crate::store::ConfigStore;
use crate::ManagerError;

#[derive(Clone, Debug)]
struct PendingConfirmation {
    previous_layout: Layout,
    applied_at: Instant,
    timeout: Duration,
}

impl PendingConfirmation {
    fn new(previous_layout: Layout, timeout: Duration) -> Self {
        Self {
            previous_layout,
            applied_at: Instant::now(),
            timeout,
        }
    }

    fn expired(&self) -> bool {
        self.applied_at.elapsed() >= self.timeout
    }

    fn remaining(&self) -> Duration {
        self.timeout
            .checked_sub(self.applied_at.elapsed())
            .unwrap_or(Duration::ZERO)
    }
}

#[derive(Debug)]
pub struct MonarchDisplayManager<B, S> {
    backend: B,
    store: S,
    config: AppConfig,
    pending_confirmation: Option<PendingConfirmation>,
    confirmation_timeout: Duration,
}

impl<B, S> MonarchDisplayManager<B, S>
where
    B: DisplayBackend,
    S: ConfigStore,
{
    pub fn new(backend: B, store: S) -> Result<Self, ManagerError> {
        let mut config = store.load()?;
        let mut should_persist = false;
        if config
            .settings
            .profile_shortcut_base
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .is_none()
        {
            config.settings.profile_shortcut_base = Some(DEFAULT_PROFILE_SHORTCUT_BASE.to_string());
            should_persist = true;
        }
        if config
            .settings
            .display_toggle_shortcut_base
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .is_none()
        {
            config.settings.display_toggle_shortcut_base =
                Some(DEFAULT_DISPLAY_TOGGLE_SHORTCUT_BASE.to_string());
            should_persist = true;
        }
        let confirmation_timeout = Duration::from_secs(config.settings.revert_timeout_secs.max(1));

        if config.last_known_good_layout.is_none() || config.last_restorable_layout.is_none() {
            let current_layout = backend.get_layout()?;
            if config.last_known_good_layout.is_none() {
                config.last_known_good_layout = Some(current_layout.clone());
            }
            if config.last_restorable_layout.is_none() {
                config.last_restorable_layout = Some(current_layout);
            }
            should_persist = true;
        }
        if should_persist {
            store.save(&config)?;
        }

        Ok(Self {
            backend,
            store,
            config,
            pending_confirmation: None,
            confirmation_timeout,
        })
    }

    pub fn set_confirmation_timeout(&mut self, timeout: Duration) {
        self.confirmation_timeout = timeout;
    }

    pub fn list_displays(&self) -> Result<Vec<DisplayInfo>, ManagerError> {
        self.backend.list_displays()
    }

    pub fn get_layout(&self) -> Result<Layout, ManagerError> {
        self.backend.get_layout()
    }

    pub fn color_state_signature(&self) -> Result<Option<String>, ManagerError> {
        self.backend.color_state_signature()
    }

    pub fn reapply_color_calibration(&self) -> Result<(), ManagerError> {
        self.backend.reapply_color_calibration()
    }

    pub fn has_pending_confirmation(&self) -> bool {
        self.pending_confirmation.is_some()
    }

    pub fn pending_confirmation_remaining(&self) -> Option<Duration> {
        self.pending_confirmation
            .as_ref()
            .map(PendingConfirmation::remaining)
    }

    pub fn apply_layout(&mut self, layout: Layout) -> Result<(), ManagerError> {
        self.ensure_no_pending_confirmation()?;
        let mut layout = layout;
        layout.ensure_valid()?;
        normalize_primary(&mut layout);

        let current_layout = self.backend.get_layout()?;
        self.config.last_known_good_layout = Some(current_layout.clone());
        self.config.last_restorable_layout = Some(current_layout.clone());
        self.persist_config()?;

        self.backend.apply_layout(layout)?;
        self.pending_confirmation = Some(PendingConfirmation::new(
            current_layout,
            self.confirmation_timeout,
        ));
        Ok(())
    }

    pub fn confirm_current_layout(&mut self) -> Result<(), ManagerError> {
        if self.pending_confirmation.is_none() {
            return Err(ManagerError::NoPendingConfirmation);
        }

        let current_layout = self.backend.get_layout()?;
        self.pending_confirmation = None;
        self.config.last_known_good_layout = Some(current_layout);
        self.persist_config()
    }

    pub fn rollback_pending(&mut self) -> Result<(), ManagerError> {
        let pending = self
            .pending_confirmation
            .take()
            .ok_or(ManagerError::NoPendingConfirmation)?;

        self.backend.apply_layout(pending.previous_layout.clone())?;
        self.config.last_known_good_layout = Some(pending.previous_layout);
        self.persist_config()
    }

    pub fn rollback_if_confirmation_expired(&mut self) -> Result<bool, ManagerError> {
        let expired = self
            .pending_confirmation
            .as_ref()
            .map(PendingConfirmation::expired)
            .unwrap_or(false);

        if expired {
            self.rollback_pending()?;
            return Ok(true);
        }

        Ok(false)
    }

    pub fn toggle_display(&mut self, display_id: &DisplayId) -> Result<(), ManagerError> {
        let mut layout = self.backend.get_layout()?;
        let resolved_display_id = resolve_display_id_for_layout_action(display_id, &layout)
            .unwrap_or_else(|| display_id.clone());
        let index = layout
            .find_output_index(&resolved_display_id)
            .ok_or_else(|| {
                ManagerError::NotFound(format!(
                    "display ({}, {})",
                    display_id.adapter_luid, display_id.target_id
                ))
            })?;

        let currently_enabled = layout.outputs[index].enabled;
        if currently_enabled && layout.enabled_output_count() == 1 {
            return Err(ManagerError::Validation(
                "cannot disable the last active display".to_string(),
            ));
        }

        layout.outputs[index].enabled = !currently_enabled;
        if !layout.outputs[index].enabled {
            layout.outputs[index].primary = false;
        }

        normalize_primary(&mut layout);
        self.apply_layout(layout)
    }

    pub fn save_profile(&mut self, name: impl Into<String>) -> Result<(), ManagerError> {
        self.ensure_no_pending_confirmation()?;

        let name = name.into();
        let name = name.trim();
        if name.is_empty() {
            return Err(ManagerError::Validation(
                "profile name cannot be empty".to_string(),
            ));
        }

        let layout = self.backend.get_layout()?;
        let profile = Profile {
            name: name.to_string(),
            layout,
        };

        if let Some(existing) = self
            .config
            .profiles
            .iter_mut()
            .find(|candidate| candidate.name == profile.name)
        {
            *existing = profile;
        } else {
            self.config.profiles.push(profile);
            self.config.profiles.sort_by(|a, b| a.name.cmp(&b.name));
        }

        self.persist_config()
    }

    pub fn list_profiles(&self) -> Vec<Profile> {
        self.config.profiles.clone()
    }

    pub fn apply_profile(&mut self, name: &str) -> Result<(), ManagerError> {
        self.ensure_no_pending_confirmation()?;

        let profile = self
            .config
            .profiles
            .iter()
            .find(|profile| profile.name == name)
            .cloned()
            .ok_or_else(|| ManagerError::NotFound(format!("profile '{name}'")))?;

        let mut target_layout = profile.layout;
        target_layout.ensure_valid()?;
        normalize_primary(&mut target_layout);

        let mut current_layout = self.backend.get_layout()?;
        normalize_primary(&mut current_layout);
        target_layout = remap_layout_display_ids(&target_layout, &current_layout);
        ensure_all_outputs_resolve(&target_layout, &current_layout)?;

        if current_layout == target_layout {
            return Ok(());
        }

        self.apply_layout(target_layout)
    }

    pub fn delete_profile(&mut self, name: &str) -> Result<(), ManagerError> {
        let before = self.config.profiles.len();
        self.config.profiles.retain(|profile| profile.name != name);
        if self.config.profiles.len() == before {
            return Err(ManagerError::NotFound(format!("profile '{name}'")));
        }
        self.persist_config()
    }

    pub fn restore_last_layout(&mut self) -> Result<(), ManagerError> {
        self.ensure_no_pending_confirmation()?;

        let target_layout = self
            .config
            .last_restorable_layout
            .clone()
            .or_else(|| self.config.last_known_good_layout.clone())
            .ok_or_else(|| ManagerError::NotFound("last restorable layout".to_string()))?;

        let current_layout = self.backend.get_layout()?;
        let mut remapped_target_layout = target_layout;
        remapped_target_layout.ensure_valid()?;
        normalize_primary(&mut remapped_target_layout);
        let remapped_target_layout =
            remap_layout_display_ids(&remapped_target_layout, &current_layout);
        ensure_all_outputs_resolve(&remapped_target_layout, &current_layout)?;
        self.backend.apply_layout(remapped_target_layout.clone())?;
        self.pending_confirmation = None;
        self.config.last_restorable_layout = Some(current_layout);
        self.config.last_known_good_layout = Some(remapped_target_layout);
        self.persist_config()
    }

    pub fn settings(&self) -> &AppSettings {
        &self.config.settings
    }

    pub fn update_settings(&mut self, settings: AppSettings) -> Result<(), ManagerError> {
        let revert_timeout_secs = settings.revert_timeout_secs.max(1);
        let startup_profile_name = settings
            .startup_profile_name
            .as_deref()
            .map(str::trim)
            .filter(|name| !name.is_empty())
            .map(str::to_string);
        let profile_shortcut_base = settings
            .profile_shortcut_base
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string)
            .unwrap_or_else(|| DEFAULT_PROFILE_SHORTCUT_BASE.to_string());
        let display_toggle_shortcut_base = settings
            .display_toggle_shortcut_base
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string)
            .unwrap_or_else(|| DEFAULT_DISPLAY_TOGGLE_SHORTCUT_BASE.to_string());
        if profile_shortcut_base.eq_ignore_ascii_case(&display_toggle_shortcut_base) {
            return Err(ManagerError::Validation(
                "profile and monitor shortcut bases must be different".to_string(),
            ));
        }
        let profile_shortcuts = settings
            .profile_shortcuts
            .into_iter()
            .filter_map(|(name, shortcut)| {
                let name = name.trim();
                let shortcut = shortcut.trim();
                if name.is_empty() || shortcut.is_empty() {
                    return None;
                }
                Some((name.to_string(), shortcut.to_string()))
            })
            .collect();
        let display_toggle_shortcuts = settings
            .display_toggle_shortcuts
            .into_iter()
            .filter_map(|(display_key, shortcut)| {
                let display_key = display_key.trim();
                let shortcut = shortcut.trim();
                if display_key.is_empty() || shortcut.is_empty() {
                    return None;
                }
                Some((display_key.to_string(), shortcut.to_string()))
            })
            .collect();
        self.confirmation_timeout = Duration::from_secs(revert_timeout_secs);
        self.config.settings = AppSettings {
            revert_timeout_secs,
            start_with_windows: settings.start_with_windows,
            startup_profile_name,
            global_shortcuts_enabled: settings.global_shortcuts_enabled,
            profile_shortcut_base: Some(profile_shortcut_base),
            display_toggle_shortcut_base: Some(display_toggle_shortcut_base),
            profile_shortcuts,
            display_toggle_shortcuts,
        };
        self.persist_config()
    }

    pub fn config(&self) -> &AppConfig {
        &self.config
    }

    fn ensure_no_pending_confirmation(&self) -> Result<(), ManagerError> {
        if self.pending_confirmation.is_some() {
            return Err(ManagerError::ConfirmationPending);
        }
        Ok(())
    }

    fn persist_config(&self) -> Result<(), ManagerError> {
        self.store.save(&self.config)
    }
}

fn normalize_primary(layout: &mut Layout) {
    let mut primary_found = false;

    for output in &mut layout.outputs {
        if !output.enabled {
            output.primary = false;
            continue;
        }

        if output.primary && !primary_found {
            primary_found = true;
            continue;
        }

        output.primary = false;
    }

    if !primary_found {
        if let Some(output) = layout.outputs.iter_mut().find(|output| output.enabled) {
            output.primary = true;
        }
    }

    if let Some(primary) = layout
        .outputs
        .iter()
        .find(|output| output.enabled && output.primary)
    {
        if primary.position.x != 0 || primary.position.y != 0 {
            let offset_x = primary.position.x;
            let offset_y = primary.position.y;
            for output in &mut layout.outputs {
                output.position.x -= offset_x;
                output.position.y -= offset_y;
            }
        }
    }
}

fn resolve_display_id_for_layout_action(
    requested: &DisplayId,
    layout: &Layout,
) -> Option<DisplayId> {
    if layout.find_output_index(requested).is_some() {
        return Some(requested.clone());
    }

    if let Some(edid_hash) = requested.edid_hash {
        let mut matches = layout
            .outputs
            .iter()
            .filter(|output| output.display_id.edid_hash == Some(edid_hash));
        let first = matches.next()?;
        if matches.next().is_none() {
            return Some(first.display_id.clone());
        }
    }

    let mut matches = layout
        .outputs
        .iter()
        .filter(|output| output.display_id.target_id == requested.target_id);
    let first = matches.next()?;
    if matches.next().is_none() {
        return Some(first.display_id.clone());
    }

    None
}

fn remap_layout_display_ids(desired: &Layout, current: &Layout) -> Layout {
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

    let mut current_by_edid: HashMap<u64, Vec<&crate::model::OutputConfig>> = HashMap::new();
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
            // Deterministic fallback for legacy profiles created before EDID hashes were
            // persisted: only remap by target id when there is exactly one unused candidate.
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

fn ensure_all_outputs_resolve(desired: &Layout, current: &Layout) -> Result<(), ManagerError> {
    let current_ids: HashSet<&DisplayId> = current
        .outputs
        .iter()
        .map(|output| &output.display_id)
        .collect();

    let unresolved = desired
        .outputs
        .iter()
        .find(|output| !current_ids.contains(&output.display_id));

    if let Some(output) = unresolved {
        return Err(ManagerError::Validation(format!(
            "profile/layout references an unknown display (target_id={}, edid_hash={}). re-save the profile on this system",
            output.display_id.target_id,
            output
                .display_id
                .edid_hash
                .map(|value| format!("{value:016x}"))
                .unwrap_or_else(|| "none".to_string())
        )));
    }

    Ok(())
}

fn unique_unused_candidates<'a>(
    candidates: Vec<&'a crate::model::OutputConfig>,
    used: &HashSet<DisplayId>,
) -> Vec<&'a crate::model::OutputConfig> {
    candidates
        .into_iter()
        .filter(|candidate| !used.contains(&candidate.display_id))
        .collect()
}

fn unique_unused_candidates_by_target_id<'a>(
    target_id: u32,
    current_outputs: &'a [crate::model::OutputConfig],
    used: &HashSet<DisplayId>,
) -> Vec<&'a crate::model::OutputConfig> {
    current_outputs
        .iter()
        .filter(|candidate| candidate.display_id.target_id == target_id)
        .filter(|candidate| !used.contains(&candidate.display_id))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        model::{OutputConfig, Position, Resolution},
        MemoryConfigStore, MockBackend,
    };

    fn sample_display_id(target_id: u32) -> DisplayId {
        sample_display_id_on_adapter(1, target_id)
    }

    fn sample_display_id_on_adapter(adapter_luid: u64, target_id: u32) -> DisplayId {
        DisplayId {
            adapter_luid,
            target_id,
            edid_hash: Some(target_id as u64),
        }
    }

    fn sample_layout() -> Layout {
        sample_layout_on_adapter(1)
    }

    fn sample_layout_on_adapter(adapter_luid: u64) -> Layout {
        Layout {
            outputs: vec![
                OutputConfig {
                    display_id: sample_display_id_on_adapter(adapter_luid, 1),
                    enabled: true,
                    position: Position { x: 0, y: 0 },
                    resolution: Resolution {
                        width: 1920,
                        height: 1080,
                    },
                    refresh_rate_mhz: 60_000,
                    primary: true,
                },
                OutputConfig {
                    display_id: sample_display_id_on_adapter(adapter_luid, 2),
                    enabled: true,
                    position: Position { x: 1920, y: 0 },
                    resolution: Resolution {
                        width: 2560,
                        height: 1440,
                    },
                    refresh_rate_mhz: 144_000,
                    primary: false,
                },
            ],
        }
    }

    fn sample_displays() -> Vec<DisplayInfo> {
        sample_displays_on_adapter(1)
    }

    fn sample_displays_on_adapter(adapter_luid: u64) -> Vec<DisplayInfo> {
        vec![
            DisplayInfo {
                id: sample_display_id_on_adapter(adapter_luid, 1),
                friendly_name: "Primary".to_string(),
                is_active: true,
                is_primary: true,
                resolution: Resolution {
                    width: 1920,
                    height: 1080,
                },
                refresh_rate_mhz: 60_000,
            },
            DisplayInfo {
                id: sample_display_id_on_adapter(adapter_luid, 2),
                friendly_name: "Secondary".to_string(),
                is_active: true,
                is_primary: false,
                resolution: Resolution {
                    width: 2560,
                    height: 1440,
                },
                refresh_rate_mhz: 144_000,
            },
        ]
    }

    fn build_manager() -> (
        MonarchDisplayManager<MockBackend, MemoryConfigStore>,
        MockBackend,
        MemoryConfigStore,
    ) {
        let backend = MockBackend::new(sample_displays(), sample_layout()).unwrap();
        let store = MemoryConfigStore::default();
        let manager = MonarchDisplayManager::new(backend.clone(), store.clone()).unwrap();
        (manager, backend, store)
    }

    #[test]
    fn toggle_display_creates_pending_confirmation() {
        let (mut manager, backend, _) = build_manager();
        manager.toggle_display(&sample_display_id(2)).unwrap();

        let layout = backend.current_layout().unwrap();
        assert_eq!(layout.enabled_output_count(), 1);
        assert!(manager.has_pending_confirmation());
        assert!(manager.pending_confirmation_remaining().is_some());
    }

    #[test]
    fn cannot_disable_last_active_display() {
        let (mut manager, _, _) = build_manager();

        manager.toggle_display(&sample_display_id(2)).unwrap();
        manager.confirm_current_layout().unwrap();

        let err = manager.toggle_display(&sample_display_id(1)).unwrap_err();
        assert!(matches!(err, ManagerError::Validation(_)));
    }

    #[test]
    fn rollback_restores_previous_layout() {
        let (mut manager, backend, store) = build_manager();
        let original = backend.current_layout().unwrap();

        manager.toggle_display(&sample_display_id(2)).unwrap();
        manager.rollback_pending().unwrap();

        assert_eq!(backend.current_layout().unwrap(), original);
        assert_eq!(
            store.snapshot().unwrap().last_known_good_layout,
            Some(original)
        );
    }

    #[test]
    fn detaching_primary_reassigns_primary_and_rebases_origin() {
        let (mut manager, backend, _) = build_manager();

        manager.toggle_display(&sample_display_id(1)).unwrap();

        let layout = backend.current_layout().unwrap();
        let primary = layout
            .outputs
            .iter()
            .find(|output| output.enabled && output.primary)
            .expect("expected new primary after detaching original primary");
        assert_eq!(primary.display_id, sample_display_id(2));
        assert_eq!(primary.position, Position { x: 0, y: 0 });

        let detached_former_primary = layout
            .outputs
            .iter()
            .find(|output| output.display_id == sample_display_id(1))
            .expect("expected detached former primary output");
        assert!(!detached_former_primary.enabled);
        assert_eq!(
            detached_former_primary.position,
            Position { x: -1920, y: 0 }
        );
    }

    #[test]
    fn reattaching_primary_after_detach_preserves_non_overlapping_positions() {
        let (mut manager, backend, _) = build_manager();

        manager.toggle_display(&sample_display_id(1)).unwrap();
        manager.confirm_current_layout().unwrap();
        manager.toggle_display(&sample_display_id(1)).unwrap();

        let layout = backend.current_layout().unwrap();
        let enabled: Vec<_> = layout
            .outputs
            .iter()
            .filter(|output| output.enabled)
            .collect();
        assert_eq!(enabled.len(), 2);
        assert!(enabled
            .iter()
            .any(|output| output.position == Position { x: 0, y: 0 }));
        assert!(enabled
            .iter()
            .any(|output| output.position == Position { x: -1920, y: 0 }));
    }

    #[test]
    fn can_apply_again_after_manual_rollback() {
        let (mut manager, backend, _) = build_manager();

        manager.toggle_display(&sample_display_id(2)).unwrap();
        assert!(manager.has_pending_confirmation());

        manager.rollback_pending().unwrap();
        assert!(!manager.has_pending_confirmation());

        manager.toggle_display(&sample_display_id(2)).unwrap();
        assert!(manager.has_pending_confirmation());
        assert_eq!(backend.current_layout().unwrap().enabled_output_count(), 1);
    }

    #[test]
    fn save_and_apply_profile_round_trip() {
        let (mut manager, backend, _) = build_manager();

        manager.save_profile("dual").unwrap();
        manager.toggle_display(&sample_display_id(2)).unwrap();
        manager.confirm_current_layout().unwrap();

        manager.apply_profile("dual").unwrap();
        let layout = backend.current_layout().unwrap();
        assert_eq!(layout.enabled_output_count(), 2);
        assert!(manager.has_pending_confirmation());
    }

    #[test]
    fn applying_matching_profile_is_a_noop() {
        let (mut manager, backend, _) = build_manager();

        manager.save_profile("current").unwrap();
        let before = backend.current_layout().unwrap();

        manager.apply_profile("current").unwrap();

        assert_eq!(backend.current_layout().unwrap(), before);
        assert!(!manager.has_pending_confirmation());
    }

    #[test]
    fn apply_profile_remaps_display_ids_after_adapter_luid_change() {
        let backend =
            MockBackend::new(sample_displays_on_adapter(9), sample_layout_on_adapter(9)).unwrap();
        let profile_layout = sample_layout_on_adapter(1);
        let store = MemoryConfigStore::new(AppConfig {
            profiles: vec![Profile {
                name: "dual".to_string(),
                layout: profile_layout,
            }],
            ..AppConfig::default()
        });
        let mut manager = MonarchDisplayManager::new(backend.clone(), store).unwrap();

        manager.apply_profile("dual").unwrap();

        let applied = backend.current_layout().unwrap();
        assert!(applied
            .outputs
            .iter()
            .all(|output| output.display_id.adapter_luid == 9));
        assert!(!manager.has_pending_confirmation());
    }

    #[test]
    fn toggle_display_remaps_stale_adapter_luid_by_target_id() {
        let backend =
            MockBackend::new(sample_displays_on_adapter(9), sample_layout_on_adapter(9)).unwrap();
        let store = MemoryConfigStore::default();
        let mut manager = MonarchDisplayManager::new(backend.clone(), store).unwrap();

        manager
            .toggle_display(&sample_display_id_on_adapter(1, 2))
            .unwrap();

        let layout = backend.current_layout().unwrap();
        let output = layout
            .outputs
            .iter()
            .find(|output| output.display_id == sample_display_id_on_adapter(9, 2))
            .expect("expected remapped target");
        assert!(!output.enabled);
    }

    #[test]
    fn delete_profile_removes_saved_profile() {
        let (mut manager, _, _) = build_manager();
        manager.save_profile("dual").unwrap();
        manager.delete_profile("dual").unwrap();
        assert!(manager.list_profiles().is_empty());
    }

    #[test]
    fn restore_last_layout_restores_previous_confirmed_layout() {
        let (mut manager, backend, _) = build_manager();
        let original = backend.current_layout().unwrap();

        manager.toggle_display(&sample_display_id(2)).unwrap();
        manager.confirm_current_layout().unwrap();
        assert_eq!(backend.current_layout().unwrap().enabled_output_count(), 1);

        manager.restore_last_layout().unwrap();
        assert_eq!(backend.current_layout().unwrap(), original);
    }

    #[test]
    fn expired_confirmation_triggers_auto_rollback() {
        let (mut manager, backend, _) = build_manager();
        let original = backend.current_layout().unwrap();

        manager.set_confirmation_timeout(Duration::ZERO);
        manager.toggle_display(&sample_display_id(2)).unwrap();

        let rolled_back = manager.rollback_if_confirmation_expired().unwrap();
        assert!(rolled_back);
        assert_eq!(backend.current_layout().unwrap(), original);
        assert!(!manager.has_pending_confirmation());
    }
}
