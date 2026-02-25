use std::time::{Duration, Instant};

use crate::backend::DisplayBackend;
use crate::model::{AppConfig, AppSettings, DisplayId, DisplayInfo, Layout, Profile};
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
        let confirmation_timeout = Duration::from_secs(config.settings.revert_timeout_secs.max(1));

        if config.last_known_good_layout.is_none() || config.last_restorable_layout.is_none() {
            let current_layout = backend.get_layout()?;
            if config.last_known_good_layout.is_none() {
                config.last_known_good_layout = Some(current_layout.clone());
            }
            if config.last_restorable_layout.is_none() {
                config.last_restorable_layout = Some(current_layout);
            }
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
        let index = layout.find_output_index(display_id).ok_or_else(|| {
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

        target_layout.ensure_valid()?;

        let current_layout = self.backend.get_layout()?;
        self.backend.apply_layout(target_layout.clone())?;
        self.pending_confirmation = None;
        self.config.last_restorable_layout = Some(current_layout);
        self.config.last_known_good_layout = Some(target_layout);
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
        self.confirmation_timeout = Duration::from_secs(revert_timeout_secs);
        self.config.settings = AppSettings {
            revert_timeout_secs,
            start_with_windows: settings.start_with_windows,
            startup_profile_name,
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        model::{OutputConfig, Position, Resolution},
        MemoryConfigStore, MockBackend,
    };

    fn sample_display_id(target_id: u32) -> DisplayId {
        DisplayId {
            adapter_luid: 1,
            target_id,
            edid_hash: Some(target_id as u64),
        }
    }

    fn sample_layout() -> Layout {
        Layout {
            outputs: vec![
                OutputConfig {
                    display_id: sample_display_id(1),
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
                    display_id: sample_display_id(2),
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
        vec![
            DisplayInfo {
                id: sample_display_id(1),
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
                id: sample_display_id(2),
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
